use super::*;

impl OpenAiCompatibleProvider {
    pub(super) fn prepare_streaming_model_request(
        &self,
        request: &ModelRequest,
    ) -> Result<PreparedStreamingModelRequest, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }

        let max_output_tokens = request
            .metadata
            .get("max_output_tokens")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0);
        let estimated_prompt_tokens = estimate_request_tokens(&request.messages, &request.tools);
        let request_body =
            crate::request_builder::build_chat_request_json_with_tools_output_limit_vision_and_images(
            &self.config.model,
            &request.messages,
            true,
            &request.tools,
            max_output_tokens,
            self.config.supports_vision_content(),
            &mut |path| self.image_cache.resolve(path),
        )?;
        Ok(PreparedStreamingModelRequest::encoded(
            request_body,
            estimated_prompt_tokens,
        ))
    }

    pub(super) fn complete_prepared_streaming_model_request(
        &self,
        request: &PreparedStreamingModelRequest,
        on_delta: &mut dyn FnMut(&str),
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_prepared_streaming_model_request_with_activity(
            request,
            on_delta,
            &mut || {},
            should_cancel,
        )
    }

    pub(super) fn complete_prepared_streaming_model_request_with_activity(
        &self,
        request: &PreparedStreamingModelRequest,
        mut on_delta: &mut dyn FnMut(&str),
        mut on_activity: &mut dyn FnMut(),
        mut should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }
        let (request_body, estimated_prompt_tokens, request_payload_sha256) = request
            .encoded_parts()
            .ok_or_else(|| ModelError::new("prepared model request did not include a body"))?;
        let request_body = request_body.clone();
        let idle_timeout = Duration::from_secs(self.config.timeout_seconds.max(1));
        let hard_timeout =
            Duration::from_secs(streaming_hard_timeout_seconds(self.config.timeout_seconds));
        let deadline = Instant::now() + hard_timeout;
        let http_request = http_request(
            &self.config.chat_completions_url(),
            &self.config.api_key,
            Some(request_body),
            hard_timeout,
            true,
        )?;
        let mut response = run_http(async {
            let response = await_http(
                http_request.send(),
                deadline,
                hard_timeout,
                "model stream request",
                &mut should_cancel,
            )
            .await?;
            consume_streaming_response(
                response,
                &self.config.model,
                &self.config.base_url,
                idle_timeout,
                hard_timeout,
                deadline,
                &mut on_delta,
                &mut on_activity,
                &mut should_cancel,
            )
            .await
        })?;
        crate::provider_receipt::attach_request_payload_sha256(
            &mut response.metadata,
            request_payload_sha256,
        );
        normalize_model_usage(&mut response, estimated_prompt_tokens);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::MessageRole;
    use base64::Engine;
    use std::fs;
    use std::sync::Arc;

    #[test]
    fn prepared_streaming_body_matches_canonical_json_and_shares_bytes() {
        let root = std::env::temp_dir().join(format!(
            "cindx-provider-prepared-vision-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let path = root.join(".cindx/vision.png");
        fs::create_dir_all(path.parent().expect("fixture should have a parent"))
            .expect("fixture directory should write");
        fs::write(&path, [0x89, b'P', b'N', b'G']).expect("image fixture should write");
        let request = ModelRequest {
            role: ModelRole::Executor,
            messages: vec![Message {
                role: MessageRole::User,
                content: "Inspect this screenshot".to_string(),
                metadata: [("image_paths".to_string(), path.display().to_string())]
                    .into_iter()
                    .collect(),
            }],
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: [("max_output_tokens".to_string(), "4096".to_string())]
                .into_iter()
                .collect(),
        };
        let expected = build_chat_request_json_with_tools_output_limit_and_vision(
            "qwen-vl-max",
            &request.messages,
            true,
            &request.tools,
            Some(4096),
            true,
        )
        .expect("canonical request should encode");
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "https://example.test/v1".to_string(),
            api_key: "secret".to_string(),
            model: "qwen-vl-max".to_string(),
            embedding_model: String::new(),
            timeout_seconds: 30,
        });

        let prepared = provider
            .prepare_streaming_request(&request)
            .expect("streaming request should prepare");
        let first = prepared
            .encoded_parts()
            .map(|(body, _, _)| body)
            .expect("openai-compatible request should have encoded bytes")
            .clone();
        let second = prepared
            .encoded_parts()
            .map(|(body, _, _)| body)
            .expect("prepared body should remain reusable")
            .clone();
        let repeated = provider
            .prepare_streaming_request(&request)
            .expect("unchanged image should prepare from cache");
        let cache_stats = provider.image_cache.stats();
        let debug = format!("{prepared:?}");
        let first_digest = prepared.encoded_parts().unwrap().2.to_string();
        let repeated_digest = repeated.encoded_parts().unwrap().2.to_string();
        let _ = fs::remove_dir_all(root);

        assert!(prepared.deferred_request().is_none());
        assert_eq!(first.as_ref(), expected.as_bytes());
        assert_eq!(
            repeated.encoded_parts().unwrap().0.as_ref(),
            expected.as_bytes()
        );
        assert_eq!(
            first.as_ptr(),
            second.as_ptr(),
            "Bytes clones must share storage"
        );
        assert_eq!(cache_stats.hits, 1);
        assert_eq!(cache_stats.reads, 1);
        assert_eq!(cache_stats.encodes, 1);
        assert_eq!(first_digest, repeated_digest);
        assert_eq!(
            first_digest,
            crate::provider_receipt::request_payload_sha256(expected.as_bytes())
        );
        assert!(debug.contains("request_body_bytes"));
        assert!(!debug.contains("Inspect this screenshot"));
        assert!(!debug.contains("base64"));
        assert!(!debug.contains("secret"));
        println!(
            "{{\"schema\":\"cindx.prepared-image-request-scaling.v1\",\"cache_hits\":{},\"image_reads\":{},\"image_encodes\":{},\"bytes_share_storage\":true}}",
            cache_stats.hits, cache_stats.reads, cache_stats.encodes
        );
    }

    #[test]
    fn prepared_snapshot_stays_stable_while_next_prepare_observes_replacement() {
        let root = std::env::temp_dir().join(format!(
            "cindx-provider-prepared-replacement-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let path = root.join(".cindx/vision.png");
        let replacement = root.join(".cindx/replacement.png");
        fs::create_dir_all(path.parent().expect("fixture should have a parent"))
            .expect("fixture directory should write");
        fs::write(&path, [1, 2, 3, 4]).expect("first image should write");
        let request = ModelRequest {
            role: ModelRole::Executor,
            messages: vec![Message {
                role: MessageRole::User,
                content: "Inspect replacement".to_string(),
                metadata: [("image_paths".to_string(), path.display().to_string())]
                    .into_iter()
                    .collect(),
            }],
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: Metadata::new(),
        };
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "https://example.test/v1".to_string(),
            api_key: "secret".to_string(),
            model: "qwen-vl-max".to_string(),
            embedding_model: String::new(),
            timeout_seconds: 30,
        });

        let prepared = provider
            .prepare_streaming_request(&request)
            .expect("first request should prepare");
        let original = prepared.encoded_parts().unwrap().0.clone();
        let original_digest = prepared.encoded_parts().unwrap().2.to_string();
        let original_ptr = original.as_ptr();
        fs::write(&replacement, [9, 8, 7, 6]).expect("replacement should write");
        fs::remove_file(&path).expect("first image should remove");
        fs::rename(&replacement, &path).expect("replacement should publish");

        let next = provider
            .prepare_streaming_request(&request)
            .expect("replacement request should prepare");
        let next_body = next.encoded_parts().unwrap().0;
        let next_digest = next.encoded_parts().unwrap().2;
        let stable = prepared.encoded_parts().unwrap().0;
        let stats = provider.image_cache.stats();
        let _ = fs::remove_dir_all(root);

        assert_eq!(stable.as_ptr(), original_ptr);
        assert_eq!(stable.as_ref(), original.as_ref());
        assert_ne!(next_body.as_ref(), original.as_ref());
        assert_ne!(next_digest, original_digest);
        assert_eq!(prepared.encoded_parts().unwrap().2, original_digest);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.reads, 2);
        assert_eq!(stats.encodes, 2);
        assert_eq!(stats.invalidations, 1);
    }

    #[test]
    fn text_only_request_does_not_resolve_image_paths() {
        let mut resolve_count = 0usize;
        let body = build_chat_request_json_with_tools_output_limit_vision_and_images(
            "text-only",
            &[Message {
                role: MessageRole::User,
                content: "Inspect if supported".to_string(),
                metadata: [(
                    "image_paths".to_string(),
                    "/missing/.cindx/vision.png".to_string(),
                )]
                .into_iter()
                .collect(),
            }],
            false,
            &[],
            None,
            false,
            &mut |_| {
                resolve_count += 1;
                Some(Arc::<str>::from("data:image/png;base64,unused"))
            },
        )
        .expect("text request should encode");

        assert_eq!(resolve_count, 0);
        assert!(!body.contains("image_url"));
        assert!(body.contains("selected model does not support vision"));
    }

    #[test]
    fn vision_request_resolves_each_selected_image_once() {
        let mut resolve_count = 0usize;
        let body = build_chat_request_json_with_tools_output_limit_vision_and_images(
            "vision-model",
            &[Message {
                role: MessageRole::User,
                content: "Compare both".to_string(),
                metadata: [(
                    "image_paths".to_string(),
                    "/tmp/.cindx/one.png\n/tmp/.cindx/two.png".to_string(),
                )]
                .into_iter()
                .collect(),
            }],
            true,
            &[],
            None,
            true,
            &mut |path| {
                resolve_count += 1;
                Some(Arc::<str>::from(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(path)
                )))
            },
        )
        .expect("vision request should encode");

        assert_eq!(resolve_count, 2);
        assert_eq!(body.matches("\"type\":\"image_url\"").count(), 2);
    }

    #[test]
    fn request_json_keeps_oversized_image_limit() {
        let root = std::env::temp_dir().join(format!(
            "cindx-provider-oversized-vision-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let path = root.join(".cindx/oversized.png");
        fs::create_dir_all(path.parent().expect("fixture should have a parent"))
            .expect("fixture directory should write");
        let file = fs::File::create(&path).expect("image fixture should create");
        file.set_len(24 * 1024 * 1024 + 1)
            .expect("sparse oversized fixture should size");
        let body = build_chat_request_json(
            "qwen-vl-max",
            &[Message {
                role: MessageRole::User,
                content: "Inspect this screenshot".to_string(),
                metadata: [("image_paths".to_string(), path.display().to_string())]
                    .into_iter()
                    .collect(),
            }],
            false,
        )
        .expect("request should encode while omitting oversized image");
        let _ = fs::remove_dir_all(root);

        assert!(!body.contains("\"type\":\"image_url\""));
        assert!(body.contains("Inspect this screenshot"));
    }
}
