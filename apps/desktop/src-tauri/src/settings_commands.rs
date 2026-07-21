use super::*;

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
pub(crate) fn save_provider_config(
    state: tauri::State<'_, AppState>,
    input: ProviderConfigInput,
) -> Result<Phase4State, String> {
    let config = {
        let mut config = state
            .provider_config
            .lock()
            .map_err(|error| format!("provider config lock poisoned: {error}"))?;
        apply_provider_config_input(&mut config, input);
        save_provider_config_to_disk(&config).map_err(|error| error.to_string())?;
        config.clone()
    };
    if let Ok(root) = active_workspace_root(&state) {
        invalidate_workspace_knowledge_cache(&state, &root)?;
    }
    invalidate_tool_registry_cache(&state)?;

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase4_task_id(),
        EventKind::TaskStatusChanged,
        "Provider config saved",
        [
            ("provider".to_string(), "openai-compatible".to_string()),
            ("base_url".to_string(), config.base_url.clone()),
            (
                "executor_model".to_string(),
                config.model_for_role(&ModelRole::Executor),
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
}

#[tauri::command]
pub(crate) fn set_prompt_evolution_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<Phase4State, String> {
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
        let mut config = clone_provider_config(&state)?;
        let base_url = normalized_config_value(&input.base_url);
        let api_key = normalized_config_value(&input.api_key);
        if !base_url.is_empty() {
            config.base_url = base_url;
        }
        if !api_key.is_empty() {
            config.api_key = api_key;
        }

        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url,
            api_key: config.api_key,
            model: config.model,
            embedding_model: config.embedding_model,
            timeout_seconds: 30,
        });
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

#[tauri::command]
pub(crate) async fn validate_image_endpoint(
    input: ImageEndpointValidationInput,
) -> Result<ImageEndpointValidationState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let base_url = if input.image_endpoint.trim().is_empty() {
            normalized_config_value(&input.base_url)
        } else {
            normalized_config_value(&input.image_endpoint)
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
                ("provider".to_string(), "openai-compatible".to_string()),
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
        let _ = stream_app.emit(
            "model-stream-delta",
            ModelStreamDelta {
                task_id: stream_task_id.clone(),
                request_id: stream_request_id.clone(),
                session_id: None,
                delta: delta.to_string(),
                done: false,
                reset: false,
                error: None,
            },
        );
    });

    match result {
        Ok(response) => {
            let latency_ms = current_time_millis().saturating_sub(started_at_ms);
            let _ = app.emit(
                "model-stream-delta",
                ModelStreamDelta {
                    task_id: task_id.0.clone(),
                    request_id: request_id.clone(),
                    session_id: None,
                    delta: String::new(),
                    done: true,
                    reset: false,
                    error: None,
                },
            );

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
                    ("provider".to_string(), "openai-compatible".to_string()),
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
            let _ = app.emit(
                "model-stream-delta",
                ModelStreamDelta {
                    task_id: task_id.0.clone(),
                    request_id,
                    session_id: None,
                    delta: String::new(),
                    done: true,
                    reset: false,
                    error: Some(message.clone()),
                },
            );
            record_phase4_error(&state, &message)?;
            phase4_state_with_error(&state, &config, &message)
        }
    }
}
