use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CollaborationStageError {
    RunStopped,
    SteerInterrupted,
    AttemptDeadline,
    StageDeadline,
    ModelFailure(AgentFailure),
    DecisionRejected(String),
    Failed(String),
}

impl CollaborationStageError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::RunStopped => MODEL_REQUEST_CANCELLED.to_string(),
            Self::SteerInterrupted => COLLABORATION_STEER_INTERRUPTED.to_string(),
            Self::AttemptDeadline => {
                "conductor response did not start before the failover deadline".to_string()
            }
            Self::StageDeadline => "collaboration stage deadline exhausted".to_string(),
            Self::ModelFailure(failure) => failure.message,
            Self::DecisionRejected(error) => error,
            Self::Failed(error) => error,
        }
    }
}

pub(crate) fn collaboration_stage_terminal_presentation(
    completion: &CollaborationCompletion,
) -> (&'static str, &'static str) {
    if completion.content.is_some() {
        return ("completed", "finished");
    }
    match completion
        .usage
        .get(COLLABORATION_TERMINATION_SCOPE_KEY)
        .map(String::as_str)
    {
        Some(COLLABORATION_TERMINATION_STEER) => ("interrupted", "interrupted"),
        Some(COLLABORATION_TERMINATION_ATTEMPT) => ("degraded", "stalled"),
        Some(COLLABORATION_TERMINATION_STAGE) => ("degraded", "deadline exhausted"),
        _ => match completion.failure.as_ref() {
            Some(failure) if failure.class == AgentFailureClass::Cancelled => {
                ("interrupted", "interrupted")
            }
            Some(failure) if failure.code == RunStopReason::StageBudgetExhausted.code() => {
                ("degraded", "deadline exhausted")
            }
            _ => ("degraded", "unavailable"),
        },
    }
}

pub(crate) fn collaboration_stage_event_subject(stage: &str) -> String {
    crate::runtime_values::collaboration_stage_display_label(stage)
}

pub(crate) fn collaboration_stage_result(
    completion: CollaborationCompletion,
) -> Result<String, CollaborationStageError> {
    match completion
        .usage
        .get(COLLABORATION_TERMINATION_SCOPE_KEY)
        .map(String::as_str)
    {
        Some(COLLABORATION_TERMINATION_RUN) => return Err(CollaborationStageError::RunStopped),
        Some(COLLABORATION_TERMINATION_STEER) => {
            return Err(CollaborationStageError::SteerInterrupted)
        }
        Some(COLLABORATION_TERMINATION_ATTEMPT) => {
            return Err(CollaborationStageError::AttemptDeadline)
        }
        Some(COLLABORATION_TERMINATION_STAGE) => {
            return Err(CollaborationStageError::StageDeadline)
        }
        _ => {}
    }
    if completion
        .failure
        .as_ref()
        .is_some_and(|failure| failure.code == CONDUCTOR_NO_PROGRESS_DEADLINE_CODE)
    {
        return Err(CollaborationStageError::AttemptDeadline);
    }
    if completion
        .failure
        .as_ref()
        .is_some_and(|failure| failure.code == RunStopReason::StageBudgetExhausted.code())
    {
        return Err(CollaborationStageError::StageDeadline);
    }
    if let Some(content) = completion.content {
        return Ok(content);
    }
    if let Some(failure) = completion.failure {
        return Err(CollaborationStageError::ModelFailure(failure));
    }
    Err(CollaborationStageError::Failed(
        completion
            .error
            .unwrap_or_else(|| "collaboration model returned no content".to_string()),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collaboration_stage_finished_metadata(
    collaboration_id: &str,
    stage: &str,
    role: &ModelRole,
    model: &str,
    request_id: &str,
    completion: &CollaborationCompletion,
    attribution: AgentModelAttribution,
    stage_metadata: &Metadata,
) -> Result<(String, Metadata), String> {
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
    attribution
        .insert_into(&mut metadata, model, role_label(role), stage)
        .map_err(|error| error.to_string())?;
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
    for (key, value) in &completion.usage {
        metadata.insert(key.clone(), value.clone());
    }
    let (status, terminal_verb) = collaboration_stage_terminal_presentation(completion);
    let summary = if let Some(content) = completion.content.as_ref() {
        metadata.insert("output".to_string(), content.clone());
        metadata.insert("status".to_string(), status.to_string());
        format!("{} {terminal_verb}", collaboration_stage_event_subject(stage))
    } else {
        metadata.insert("status".to_string(), status.to_string());
        metadata.insert(
            "error".to_string(),
            completion
                .error
                .clone()
                .unwrap_or_else(|| "unknown collaboration failure".to_string()),
        );
        if let Some(failure) = completion
            .failure
            .as_ref()
            .filter(|failure| failure.class == AgentFailureClass::Cancelled)
        {
            metadata.insert("interruption_reason".to_string(), failure.code.clone());
        }
        format!("{} {terminal_verb}", collaboration_stage_event_subject(stage))
    };
    Ok((summary, metadata))
}

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
    attribution: AgentModelAttribution,
    stage_metadata: &Metadata,
) -> Result<(), String> {
    let (summary, metadata) = collaboration_stage_finished_metadata(
        collaboration_id,
        stage,
        role,
        model,
        request_id,
        completion,
        attribution,
        stage_metadata,
    )?;
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
    attribution: AgentModelAttribution,
) -> Result<String, String> {
    run_collaboration_stage_typed(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        stage,
        role,
        model,
        prompt,
        attribution,
        CollaborationCallLimits::default(),
        |_| {},
    )
    .map_err(CollaborationStageError::message)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_conductor_collaboration_stage(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: ModelRole,
    model: &str,
    prompt: String,
    limits: CollaborationCallLimits,
) -> Result<String, CollaborationStageError> {
    run_collaboration_stage_typed(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        stage,
        role,
        model,
        prompt,
        AgentModelAttribution::service(
            AgentService::Conductor,
            AgentStage::Plan,
            AgentModelProfile::Reasoning,
        ),
        limits,
        |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
fn run_collaboration_stage_typed(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: ModelRole,
    model: &str,
    prompt: String,
    attribution: AgentModelAttribution,
    mut limits: CollaborationCallLimits,
    on_delta: impl FnMut(&str),
) -> Result<String, CollaborationStageError> {
    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))
            .map_err(CollaborationStageError::Failed)?;
    if let Some(control) = cancellation.as_ref() {
        if agent_run_should_stop(control) {
            return Err(CollaborationStageError::RunStopped);
        }
        if control.has_pending_steer() {
            return Err(CollaborationStageError::SteerInterrupted);
        }
    }
    limits.objective_epoch = Some(run_context_steer_epoch(run_context));
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
        attribution,
        &Metadata::new(),
    )
    .map_err(CollaborationStageError::Failed)?;
    let resource_checkpoint = |control: &AgentRunControl| {
        crate::agent_resource_snapshot::checkpoint_agent_run_resources(state, run_context, control)
    };
    let completion = complete_collaboration_model_for_stage_with_recovery_control(
        config.clone(),
        stage.to_string(),
        role.clone(),
        model.to_string(),
        collaboration_system_prompt_for_run(&config.agent_system_prompt, run_context),
        prompt,
        cancellation.clone(),
        limits,
        Some(&resource_checkpoint),
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
        attribution,
        &Metadata::new(),
    )
    .map_err(CollaborationStageError::Failed)?;
    collaboration_stage_result(completion)
}
