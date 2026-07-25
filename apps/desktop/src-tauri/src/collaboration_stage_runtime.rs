use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_collaboration_stage_finished(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: &ModelRole,
    model: &str,
    request_id: &str,
    completion: &CollaborationCompletion,
    stage_metadata: &Metadata,
) -> Result<(), String> {
    let mut metadata = [
        ("collaboration_id".to_string(), collaboration_id.to_string()),
        ("request_id".to_string(), request_id.to_string()),
        ("stage".to_string(), stage.to_string()),
        ("role".to_string(), role_label(role).to_string()),
        ("model".to_string(), model.to_string()),
        ("latency_ms".to_string(), completion.latency_ms.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    metadata.insert(
        "evidence_count".to_string(),
        completion.evidence.len().to_string(),
    );
    metadata.insert(
        "evidence_tools".to_string(),
        completion
            .evidence
            .iter()
            .map(|entry| entry.tool_name.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(","),
    );
    for (key, value) in stage_metadata {
        metadata.insert(key.clone(), value.clone());
    }
    if let Some(failure) = completion.failure.as_ref() {
        metadata.insert("failure_code".to_string(), failure.code.clone());
        metadata.insert(
            "failure_class".to_string(),
            failure.class.label().to_string(),
        );
        metadata.insert(
            "failure_retryable".to_string(),
            failure.retryable.to_string(),
        );
    }
    if let Some(partial) = completion.partial_content.as_ref() {
        metadata.insert("partial_output".to_string(), partial.clone());
    }
    let summary = if let Some(content) = completion.content.as_ref() {
        metadata.insert("output".to_string(), content.clone());
        metadata.insert("status".to_string(), "completed".to_string());
        for (key, value) in &completion.usage {
            metadata.insert(key.clone(), value.clone());
        }
        format!("Collaboration {stage} finished")
    } else {
        metadata.insert("status".to_string(), "degraded".to_string());
        metadata.insert(
            "error".to_string(),
            completion
                .error
                .clone()
                .unwrap_or_else(|| "unknown collaboration failure".to_string()),
        );
        format!("Collaboration {stage} unavailable")
    };
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::ModelRequestFinished,
        summary,
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_collaboration_stage(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: ModelRole,
    model: &str,
    prompt: String,
) -> Result<String, String> {
    run_collaboration_stage_with_delta(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        stage,
        role,
        model,
        prompt,
        |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_collaboration_stage_with_delta(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: ModelRole,
    model: &str,
    prompt: String,
    on_delta: impl FnMut(&str),
) -> Result<String, String> {
    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
    if let Some(control) = cancellation.as_ref() {
        if control.has_pending_steer() {
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        if agent_run_should_stop(control) {
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
    }
    let request_id = unique_id("collaboration-model");
    record_collaboration_stage_started(
        state,
        task_id,
        run_context,
        collaboration_id,
        stage,
        &role,
        model,
        &request_id,
        &Metadata::new(),
    )?;
    let completion = complete_collaboration_model_for_stage_with_control(
        config.clone(),
        stage.to_string(),
        role.clone(),
        model.to_string(),
        collaboration_system_prompt_for_run(&config.agent_system_prompt, run_context),
        prompt,
        cancellation.clone(),
        on_delta,
    );
    record_collaboration_stage_finished(
        state,
        task_id,
        run_context,
        collaboration_id,
        stage,
        &role,
        model,
        &request_id,
        &completion,
        &Metadata::new(),
    )?;
    if cancellation
        .as_ref()
        .is_some_and(|control| control.has_pending_steer())
    {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    completion.content.ok_or_else(|| {
        completion
            .error
            .unwrap_or_else(|| "collaboration model returned no content".to_string())
    })
}
