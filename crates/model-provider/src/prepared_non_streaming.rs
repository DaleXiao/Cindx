use crate::provider_receipt::{attach_request_payload_sha256, request_payload_sha256};
use crate::request_builder::build_chat_request_json_with_tools_output_limit_vision_and_images;
use crate::{
    estimate_request_tokens, execute_http_bytes_cancellable, normalize_model_usage,
    parse_model_response, parse_provider_error, ModelCallMode, ModelError, ModelRequest,
    ModelResponse, OpenAiCompatibleProvider, MAX_MODEL_RESPONSE_BYTES,
};
use bytes::Bytes;
use std::fmt;

pub struct PreparedNonStreamingModelRequest {
    request_body: Bytes,
    request_payload_sha256: String,
    estimated_prompt_tokens: u64,
}

impl PreparedNonStreamingModelRequest {
    fn encoded(request_body: String, estimated_prompt_tokens: u64) -> Self {
        let request_body = Bytes::from(request_body);
        let request_payload_sha256 = request_payload_sha256(request_body.as_ref());
        Self {
            request_body,
            request_payload_sha256,
            estimated_prompt_tokens,
        }
    }

    fn into_encoded_parts(self) -> (Bytes, u64, String) {
        (
            self.request_body,
            self.estimated_prompt_tokens,
            self.request_payload_sha256,
        )
    }

    /// Returns the digest and size of the exact encoded payload, without the body.
    pub fn payload_receipt(&self) -> (&str, usize) {
        (&self.request_payload_sha256, self.request_body.len())
    }
}

impl fmt::Debug for PreparedNonStreamingModelRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "PreparedNonStreamingModelRequest {{ request_body_bytes: {} }}",
            self.request_body.len()
        )
    }
}

impl OpenAiCompatibleProvider {
    pub fn prepare_non_streaming_request(
        &self,
        request: &ModelRequest,
    ) -> Result<PreparedNonStreamingModelRequest, ModelError> {
        if request.mode != ModelCallMode::NonStreaming {
            return Err(ModelError::new(
                "non-streaming request preparation requires non-streaming mode",
            ));
        }
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }

        let max_output_tokens = request
            .metadata
            .get("max_output_tokens")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0);
        let generation_temperature =
            crate::request_builder::generation_temperature_from_metadata(&request.metadata);
        let reasoning_effort = request
            .metadata
            .get(agent_core::REASONING_EFFORT_KEY)
            .map(String::as_str);
        let thinking_budget_override = request
            .metadata
            .get(agent_core::THINKING_BUDGET_KEY)
            .and_then(|value| value.parse::<u32>().ok());
        let estimated_prompt_tokens = estimate_request_tokens(&request.messages, &request.tools);
        let request_body = build_chat_request_json_with_tools_output_limit_vision_and_images(
            &self.config.model,
            &request.messages,
            false,
            &request.tools,
            max_output_tokens,
            self.config.supports_vision_content(),
            &mut |path| self.image_cache.resolve(path),
            generation_temperature,
            reasoning_effort,
            thinking_budget_override,
        )?;
        Ok(PreparedNonStreamingModelRequest::encoded(
            request_body,
            estimated_prompt_tokens,
        ))
    }

    pub fn complete_prepared_non_streaming_request(
        &self,
        request: PreparedNonStreamingModelRequest,
    ) -> Result<ModelResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }
        let (request_body, estimated_prompt_tokens, request_payload_sha256) =
            request.into_encoded_parts();
        let output = execute_http_bytes_cancellable(
            &self.config.chat_completions_url(),
            &self.config.api_key,
            Some(request_body),
            self.config.timeout_seconds,
            MAX_MODEL_RESPONSE_BYTES,
            &mut || false,
        )?;

        let stdout = String::from_utf8_lossy(&output.body).to_string();
        if !output.status.is_success() {
            let provider_error = parse_provider_error(&stdout).unwrap_or_default();
            return Err(ModelError::with_status(
                output.status.as_u16(),
                if provider_error.is_empty() {
                    format!("model request failed with status {}", output.status)
                } else {
                    provider_error
                },
            ));
        }

        let mut response = parse_model_response(&stdout)?;
        response
            .metadata
            .insert("model".to_string(), self.config.model.clone());
        attach_request_payload_sha256(&mut response.metadata, &request_payload_sha256);
        normalize_model_usage(&mut response, estimated_prompt_tokens);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpenAiCompatibleConfig;
    use agent_core::{Message, MessageRole, Metadata, ModelRole};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn serve_non_streaming_response(response_body: &str) -> (String, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");
        let address = listener
            .local_addr()
            .expect("loopback address should resolve");
        let response_body = response_body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("prepared request should connect");
            let mut request = Vec::new();
            let (body_start, body_bytes) = loop {
                let mut chunk = [0_u8; 4096];
                let read = stream
                    .read(&mut chunk)
                    .expect("prepared request should read");
                assert!(
                    read > 0,
                    "prepared request closed before its body completed"
                );
                request.extend_from_slice(&chunk[..read]);
                let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                else {
                    continue;
                };
                let body_start = header_end + 4;
                let headers = std::str::from_utf8(&request[..header_end])
                    .expect("prepared request headers should be UTF-8");
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .expect("prepared request should include Content-Length");
                if request.len() >= body_start + content_length {
                    break (body_start, content_length);
                }
            };
            assert_eq!(
                request.len(),
                body_start + body_bytes,
                "prepared request should contain one exact body"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .expect("loopback response should write");
            request[body_start..].to_vec()
        });
        (format!("http://{address}/v1"), handle)
    }

    fn non_streaming_request(content: &str) -> ModelRequest {
        ModelRequest {
            role: ModelRole::Executor,
            messages: vec![Message {
                role: MessageRole::User,
                content: content.to_string(),
                metadata: Metadata::new(),
            }],
            tools: Vec::new(),
            mode: ModelCallMode::NonStreaming,
            metadata: [("max_output_tokens".to_string(), "128".to_string())]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn prepared_non_streaming_request_dispatches_exact_snapshot_and_cached_receipt() {
        let response_body = r#"{"id":"prepared-response","model":"receipt-model","system_fingerprint":"fp-1","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":1,"total_tokens":8}}"#;
        let (base_url, server) = serve_non_streaming_response(response_body);
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "private-api-key".to_string(),
            model: "receipt-model".to_string(),
            embedding_model: String::new(),
            timeout_seconds: 5,
        });
        let mut request = non_streaming_request("frozen request body");
        let prepared = provider
            .prepare_non_streaming_request(&request)
            .expect("non-streaming request should prepare");
        let (cached_digest, cached_bytes) = prepared.payload_receipt();
        let cached_digest = cached_digest.to_string();
        let debug = format!("{prepared:?}");

        request.messages[0].content = "mutated after preparation".to_string();
        request
            .metadata
            .insert("max_output_tokens".to_string(), "4096".to_string());
        let response = provider
            .complete_prepared_non_streaming_request(prepared)
            .expect("prepared non-streaming request should complete");
        let captured_body = server.join().expect("loopback server should finish");
        let captured_json: serde_json::Value =
            serde_json::from_slice(&captured_body).expect("captured body should be JSON");

        assert_eq!(cached_bytes, captured_body.len());
        assert_eq!(cached_digest, request_payload_sha256(&captured_body));
        assert_eq!(captured_json["stream"], false);
        assert_eq!(captured_json["max_tokens"], 128);
        assert!(String::from_utf8_lossy(&captured_body).contains("frozen request body"));
        assert!(!String::from_utf8_lossy(&captured_body).contains("mutated after preparation"));
        assert_eq!(response.metadata["request_payload_sha256"], cached_digest);
        assert_eq!(
            response.metadata["provider_response_id"],
            "prepared-response"
        );
        assert_eq!(
            response.metadata["provider_response_model"],
            "receipt-model"
        );
        assert_eq!(response.metadata["provider_receipt_status"], "observed");
        assert!(debug.contains("request_body_bytes"));
        assert!(!debug.contains("frozen request body"));
        assert!(!debug.contains("private-api-key"));
    }

    #[test]
    fn prepared_non_streaming_request_carries_generation_temperature() {
        let response_body = r#"{"id":"temperature-response","model":"receipt-model","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#;
        let (base_url, server) = serve_non_streaming_response(response_body);
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "private-api-key".to_string(),
            model: "receipt-model".to_string(),
            embedding_model: String::new(),
            timeout_seconds: 5,
        });
        let mut request = non_streaming_request("temperature probe");
        request.metadata.insert(
            crate::GENERATION_TEMPERATURE_KEY.to_string(),
            "0".to_string(),
        );
        let prepared = provider
            .prepare_non_streaming_request(&request)
            .expect("non-streaming request should prepare");
        provider
            .complete_prepared_non_streaming_request(prepared)
            .expect("prepared request should complete");
        let captured_body = server.join().expect("loopback server should finish");
        let captured_json: serde_json::Value =
            serde_json::from_slice(&captured_body).expect("captured body should be JSON");
        assert_eq!(captured_json["temperature"], 0);
    }

    #[test]
    fn non_streaming_preparation_rejects_streaming_mode_without_dispatch() {
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "https://example.test/v1".to_string(),
            api_key: "secret".to_string(),
            model: "receipt-model".to_string(),
            embedding_model: String::new(),
            timeout_seconds: 5,
        });
        let mut request = non_streaming_request("wrong mode");
        request.mode = ModelCallMode::Streaming;

        let error = provider
            .prepare_non_streaming_request(&request)
            .expect_err("streaming mode must fail before dispatch");

        assert!(error.message.contains("requires non-streaming mode"));
    }
}
