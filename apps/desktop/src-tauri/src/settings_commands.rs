use super::*;
use crate::desktop_event_sink::DesktopEventSink;

#[tauri::command]
pub(crate) fn get_phase3_state(state: tauri::State<'_, AppState>) -> Result<Phase3State, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase3_state(&store).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn get_permission_review_state(
    app: tauri::AppHandle,
) -> Result<PermissionReviewState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let project_sessions = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?
            .clone();
        let store = open_app_read_store()?;
        permission_review_state(&store, &project_sessions).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("permission review load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) fn request_mock_permission(
    state: tauri::State<'_, AppState>,
) -> Result<Phase3State, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    request_mock_permission_in_store(&mut store).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn resolve_permission(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
) -> Result<Phase3State, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    resolve_permission_in_store(&mut store, &request_id, &decision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_phase4_state(state: tauri::State<'_, AppState>) -> Result<Phase4State, String> {
    let config = clone_provider_config(&state)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase4_state(&mut store, &config, None).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_personalization_config() -> PersonalizationConfig {
    load_personalization_config()
}

#[tauri::command]
pub(crate) fn save_personalization_config(
    input: PersonalizationConfig,
) -> Result<PersonalizationConfig, String> {
    let config = normalized_personalization_config(input);
    save_personalization_config_to_disk(&config).map_err(|error| error.to_string())?;
    Ok(config)
}

#[tauri::command]
pub(crate) async fn save_provider_config(
    app: tauri::AppHandle,
    input: ProviderConfigInput,
) -> Result<Phase4State, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _update = state
            .provider_config_update
            .lock()
            .map_err(|error| format!("provider config update lock poisoned: {error}"))?;
        let mut config = state
            .provider_config
            .lock()
            .map_err(|error| format!("provider config lock poisoned: {error}"))?
            .clone();
        apply_provider_config_input(&mut config, input);
        if !config.is_ready() {
            return Err("Provider endpoint, API key, and Chat model are required".to_string());
        }
        if config.provider_id == PROVIDER_ALIBABA_CN
            && config.api_key.trim().starts_with("sk-sp-")
        {
            return Err(
                "Alibaba Coding Plan and Token Plan keys use dedicated endpoints and protocols; the built-in Alibaba profile currently supports standard Pay-as-you-go DashScope API keys"
                    .to_string(),
            );
        }
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            model: config.model_for_role(&ModelRole::Executor),
            embedding_model: config.model_for_role(&ModelRole::Embedder),
            timeout_seconds: 30,
        });
        let (available_models, chat_verified) = if config.provider_id == PROVIDER_AZURE_OPENAI {
            (verify_azure_provider(&mut config)?, true)
        } else {
            let models = provider
                .validate_credentials()
                .map_err(|error| format!("API key verification failed: {error}"))?;
            let chat_verified = models.is_empty();
            (models, chat_verified)
        };
        if provider_supports_model_discovery(&config.provider_id) {
            reconcile_provider_models(&mut config, &available_models)?;
        }
        if !chat_verified {
            OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: config.base_url.clone(),
                api_key: config.api_key.clone(),
                model: config.model_for_role(&ModelRole::Executor),
                embedding_model: config.model_for_role(&ModelRole::Embedder),
                timeout_seconds: 30,
            })
            .validate_chat_access()
            .map_err(|error| format!("Chat model verification failed: {error}"))?;
        }
        config.auth_verified_at_ms = Some(current_time_millis());
        save_provider_config_to_disk(&config).map_err(|error| error.to_string())?;
        *state
            .provider_config
            .lock()
            .map_err(|error| format!("provider config lock poisoned: {error}"))? = config.clone();
        state
            .workspace_knowledge_cache
            .lock()
            .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?
            .clear();
        invalidate_tool_registry_cache(&state)?;
        state
            .conductor_health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();

        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &phase4_task_id(),
            EventKind::TaskStatusChanged,
            "Provider verified and configured",
            [
                ("provider".to_string(), config.provider_id.clone()),
                ("base_url".to_string(), config.base_url.clone()),
                (
                    "executor_model".to_string(),
                    config.model_for_role(&ModelRole::Executor),
                ),
                (
                    "available_models".to_string(),
                    available_models.len().to_string(),
                ),
                (
                    "agent_system_prompt_length".to_string(),
                    config.agent_system_prompt.chars().count().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;

        phase4_state(&mut store, &config, None).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("provider verification task failed to join: {error}"))?
}

fn verify_azure_provider(config: &mut ProviderConfig) -> Result<Vec<String>, String> {
    let resource = config.provider_resource.trim();
    let candidates = [
        config.base_url.clone(),
        format!("https://{resource}.openai.azure.com/openai/v1"),
        format!("https://{resource}.services.ai.azure.com/openai/v1"),
    ];
    let mut attempted = Vec::new();
    let mut errors = Vec::new();
    for base_url in candidates {
        if base_url.trim().is_empty()
            || attempted
                .iter()
                .any(|candidate: &String| candidate.eq_ignore_ascii_case(&base_url))
        {
            continue;
        }
        attempted.push(base_url.clone());
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: base_url.clone(),
            api_key: config.api_key.clone(),
            model: config.model_for_role(&ModelRole::Executor),
            embedding_model: config.model_for_role(&ModelRole::Embedder),
            timeout_seconds: 30,
        });
        match provider.validate_credentials() {
            Ok(_) => {
                config.base_url = base_url;
                return Ok(Vec::new());
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    Err(format!(
        "Azure API key or deployment verification failed: {}",
        errors
            .last()
            .cloned()
            .unwrap_or_else(|| "no valid Azure v1 endpoint was available".to_string())
    ))
}

pub(crate) fn reconcile_provider_models(
    config: &mut ProviderConfig,
    available_models: &[String],
) -> Result<(), String> {
    if available_models.is_empty() {
        return Ok(());
    }
    let available = |model: &str| {
        available_models
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(model.trim()))
    };
    let first_available = |models: &[&str]| {
        models
            .iter()
            .find(|model| available(model))
            .map(|model| (*model).to_string())
    };
    let chat_models = provider_models_for_modality(&config.provider_id, "chat");
    if chat_models.is_empty() {
        if !available(&config.model_for_role(&ModelRole::Executor)) {
            return Err(
                "API key is valid, but the configured Chat model is unavailable".to_string(),
            );
        }
        return Ok(());
    }
    let multimodal_models = provider_models_for_modality(&config.provider_id, "imageInput");
    let chat_fallback = first_available(&multimodal_models)
        .or_else(|| first_available(&chat_models))
        .or_else(|| {
            available_models
                .iter()
                .find(|model| looks_like_chat_model(model))
                .cloned()
        })
        .ok_or_else(|| "API key is valid, but no compatible Chat model is available".to_string())?;
    let resolve_chat = |model: &str| {
        if available(model) {
            model.to_string()
        } else {
            chat_fallback.clone()
        }
    };
    config.model = resolve_chat(&config.model);
    config.conductor_model = resolve_chat(&config.conductor_model);
    config.planner_model = resolve_chat(&config.planner_model);
    config.executor_model = resolve_chat(&config.executor_model);
    config.reviewer_model = resolve_chat(&config.reviewer_model);
    config.summarizer_model = resolve_chat(&config.summarizer_model);
    if provider_discovers_modality(&config.provider_id, "embedding")
        && !available(&config.embedding_model)
    {
        config.embedding_model = first_available(&provider_models_for_modality(
            &config.provider_id,
            "embedding",
        ))
        .unwrap_or_default();
    }
    if provider_discovers_modality(&config.provider_id, "imageGeneration")
        && !config.image_model.is_empty()
        && !available(&config.image_model)
    {
        config.image_model = first_available(&provider_models_for_modality(
            &config.provider_id,
            "imageGeneration",
        ))
        .unwrap_or_default();
    }
    if provider_discovers_modality(&config.provider_id, "realtime")
        && !config.voice_model.is_empty()
        && !available(&config.voice_model)
    {
        config.voice_model = first_available(&provider_models_for_modality(
            &config.provider_id,
            "realtime",
        ))
        .unwrap_or_default();
    }
    Ok(())
}

fn looks_like_chat_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    !model.is_empty()
        && ![
            "embedding",
            "rerank",
            "image",
            "dall-e",
            "realtime",
            "transcribe",
            "whisper",
            "speech",
            "tts",
        ]
        .iter()
        .any(|marker| model.contains(marker))
}

#[tauri::command]
pub(crate) fn set_prompt_evolution_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<Phase4State, String> {
    let _update = state
        .provider_config_update
        .lock()
        .map_err(|error| format!("provider config update lock poisoned: {error}"))?;
    let config = {
        let mut config = state
            .provider_config
            .lock()
            .map_err(|error| format!("provider config lock poisoned: {error}"))?;
        config.prompt_evolution_enabled = enabled;
        save_provider_config_to_disk(&config).map_err(|error| error.to_string())?;
        config.clone()
    };
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase4_task_id(),
        EventKind::TaskStatusChanged,
        if enabled {
            "Prompt evolution enabled"
        } else {
            "Prompt evolution disabled"
        },
        [("enabled".to_string(), enabled.to_string())]
            .into_iter()
            .collect(),
    )
    .map_err(|error| error.to_string())?;
    phase4_state(&mut store, &config, None).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn list_provider_models(
    app: tauri::AppHandle,
    input: ProviderModelsInput,
) -> Result<ProviderModelsState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let config = clone_provider_config(&state)?;
        let provider =
            OpenAiCompatibleProvider::new(provider_config_for_model_list(&config, &input));
        let models = provider.list_models().map_err(|error| error.to_string())?;
        Ok(ProviderModelsState {
            models,
            fetched_at_ms: current_time_millis(),
            last_error: None,
        })
    })
    .await
    .map_err(|error| format!("model list task failed: {error}"))?
}

pub(crate) fn provider_config_for_model_list(
    saved: &ProviderConfig,
    input: &ProviderModelsInput,
) -> OpenAiCompatibleConfig {
    let provider_id = normalized_config_value(&input.provider_id);
    let provider_resource = normalized_config_value(&input.provider_resource);
    let base_url = normalized_config_value(&input.base_url);
    let saved_profile = saved.provider_profile();
    let draft_profile =
        if provider_id.is_empty() && provider_resource.is_empty() && base_url.is_empty() {
            saved_profile.clone()
        } else {
            resolve_provider_profile(&provider_id, &provider_resource, &base_url, "")
        };
    let input_api_key = normalized_config_value(&input.api_key);
    let api_key = if !input_api_key.is_empty() {
        input_api_key
    } else if same_provider_model_identity(&saved_profile, &draft_profile) {
        saved.api_key.clone()
    } else {
        String::new()
    };
    OpenAiCompatibleConfig {
        base_url: draft_profile.base_url,
        api_key,
        model: saved.model.clone(),
        embedding_model: saved.embedding_model.clone(),
        timeout_seconds: 30,
    }
}

#[tauri::command]
pub(crate) async fn validate_image_endpoint(
    input: ImageEndpointValidationInput,
) -> Result<ImageEndpointValidationState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let profile = resolve_provider_profile(
            &normalized_config_value(&input.provider_id),
            &normalized_config_value(&input.provider_resource),
            &normalized_config_value(&input.base_url),
            &normalized_config_value(&input.image_endpoint),
        );
        let base_url = if profile.image_endpoint.trim().is_empty() {
            profile.base_url
        } else {
            profile.image_endpoint
        };
        let config = OpenAiCompatibleImageConfig {
            base_url,
            api_key: String::new(),
            model: normalized_config_value(&input.image_model),
            timeout_seconds: 8,
        };
        let endpoint = config.images_url();
        let provider = OpenAiCompatibleImageProvider::new(config);
        let result = provider.validate_endpoint();
        Ok(ImageEndpointValidationState {
            endpoint,
            valid: result.is_ok(),
            last_error: result.err().map(|error| error.to_string()),
        })
    })
    .await
    .map_err(|error| format!("image endpoint validation task failed: {error}"))?
}

#[tauri::command]
pub(crate) fn send_model_prompt(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    prompt: String,
) -> Result<Phase4State, String> {
    let prompt = prompt.trim().to_string();
    let config = clone_provider_config(&state)?;
    let task_id = phase4_task_id();

    if prompt.is_empty() {
        return phase4_state_with_error(&state, &config, "prompt is empty");
    }

    if !config.is_ready() {
        record_phase4_error(&state, "Provider config is incomplete")?;
        return phase4_state_with_error(&state, &config, "Provider config is incomplete");
    }

    let request_id = unique_id("model");
    let model = config.model_for_role(&ModelRole::Executor);
    let started_at_ms = current_time_millis();

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_message_event(&mut store, &task_id, MessageRole::User, &prompt)
            .map_err(|error| error.to_string())?;
        append_event(
            &mut store,
            &task_id,
            EventKind::ModelRequestStarted,
            format!("Model request started for {model}"),
            [
                ("request_id".to_string(), request_id.clone()),
                ("provider".to_string(), config.provider_id.clone()),
                ("base_url".to_string(), config.base_url.clone()),
                ("model".to_string(), model.clone()),
                ("role".to_string(), "executor".to_string()),
                ("prompt_length".to_string(), prompt.len().to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: 180,
    });
    let mut request_metadata = Metadata::new();
    request_metadata.insert("request_id".to_string(), request_id.clone());
    let request = ModelRequest {
        role: ModelRole::Executor,
        messages: vec![Message {
            role: MessageRole::User,
            content: prompt,
            metadata: Metadata::new(),
        }],
        tools: Vec::new(),
        mode: ModelCallMode::Streaming,
        metadata: request_metadata,
    };
    let stream_task_id = task_id.0.clone();
    let stream_request_id = request_id.clone();
    let stream_app = app.clone();
    let result = provider.complete_streaming(request, |delta| {
        stream_app.emit_model_stream_delta(ModelStreamDelta {
            task_id: stream_task_id.clone(),
            request_id: stream_request_id.clone(),
            session_id: None,
            delta: delta.to_string(),
            done: false,
            reset: false,
            error: None,
        });
    });

    match result {
        Ok(response) => {
            let latency_ms = current_time_millis().saturating_sub(started_at_ms);
            app.emit_model_stream_delta(ModelStreamDelta {
                task_id: task_id.0.clone(),
                request_id: request_id.clone(),
                session_id: None,
                delta: String::new(),
                done: true,
                reset: false,
                error: None,
            });

            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                &task_id,
                EventKind::ModelRequestFinished,
                format!("Model response received from {model}"),
                [
                    ("request_id".to_string(), request_id),
                    ("provider".to_string(), config.provider_id.clone()),
                    ("model".to_string(), model),
                    ("latency_ms".to_string(), latency_ms.to_string()),
                    (
                        "output_length".to_string(),
                        response.message.content.len().to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            )
            .map_err(|error| error.to_string())?;
            append_message_event(
                &mut store,
                &task_id,
                MessageRole::Assistant,
                &response.message.content,
            )
            .map_err(|error| error.to_string())?;

            phase4_state(&mut store, &config, None).map_err(|error| error.to_string())
        }
        Err(error) => {
            let message = error.to_string();
            app.emit_model_stream_delta(ModelStreamDelta {
                task_id: task_id.0.clone(),
                request_id,
                session_id: None,
                delta: String::new(),
                done: true,
                reset: false,
                error: Some(message.clone()),
            });
            record_phase4_error(&state, &message)?;
            phase4_state_with_error(&state, &config, &message)
        }
    }
}
