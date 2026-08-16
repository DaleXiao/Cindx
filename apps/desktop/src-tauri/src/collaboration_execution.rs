use super::*;

pub(crate) fn collaboration_run_should_interrupt(control: &Arc<AgentRunControl>) -> bool {
    agent_run_should_stop(control) || control.has_pending_steer()
}

pub(crate) fn collaboration_stage_should_interrupt(
    control: &Arc<AgentRunControl>,
    stage_class: RunStageClass,
) -> bool {
    collaboration_run_should_interrupt(control) || control.stage_should_stop(stage_class)
}

pub(crate) const COLLABORATION_TERMINATION_SCOPE_KEY: &str = "termination_scope";
pub(crate) const COLLABORATION_TERMINATION_RUN: &str = "run";
pub(crate) const COLLABORATION_TERMINATION_STEER: &str = "steer";
pub(crate) const COLLABORATION_TERMINATION_STAGE: &str = "stage";
pub(crate) const COLLABORATION_TERMINATION_ATTEMPT: &str = "attempt";
pub(crate) const CONDUCTOR_NO_PROGRESS_DEADLINE_CODE: &str = "conductor_no_progress_deadline";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CollaborationCallLimits {
    pub(crate) recovery_window: Option<Duration>,
    pub(crate) no_progress_timeout: Option<Duration>,
    pub(crate) provider_activity_is_progress: bool,
    pub(crate) objective_epoch: Option<u64>,
}

pub(crate) fn collaboration_no_progress_should_cancel(
    response_started: bool,
    elapsed: Duration,
    timeout: Option<Duration>,
) -> bool {
    !response_started && timeout.is_some_and(|timeout| elapsed >= timeout)
}

fn collaboration_control_failure(
    cancellation: Option<&Arc<AgentRunControl>>,
    stage_class: RunStageClass,
    attempt_deadline_reached: bool,
    objective_epoch: Option<u64>,
) -> Option<(AgentFailure, &'static str)> {
    let control = cancellation?;
    if let Some(reason) = control.stop_reason() {
        return Some((
            AgentFailure::from_stop_reason(
                reason,
                format!("Run stopped during model call: {}", reason.code()),
            ),
            COLLABORATION_TERMINATION_RUN,
        ));
    }
    if control.has_pending_steer() {
        return Some((
            AgentFailure::cancelled(
                "user_steer",
                "model request interrupted by queued user steering",
            ),
            COLLABORATION_TERMINATION_STEER,
        ));
    }
    if objective_epoch
        .is_some_and(|expected_epoch| !control.preparation_epoch_is_current(expected_epoch))
    {
        return Some((
            AgentFailure::cancelled(
                "user_steer",
                "model request superseded by applied user steering",
            ),
            COLLABORATION_TERMINATION_STEER,
        ));
    }
    if control.stage_should_stop(stage_class) {
        return Some((
            AgentFailure::budget(
                RunStopReason::StageBudgetExhausted.code(),
                "collaboration stage deadline exhausted",
            ),
            COLLABORATION_TERMINATION_STAGE,
        ));
    }
    attempt_deadline_reached.then(|| {
        (
            AgentFailure::new(
                CONDUCTOR_NO_PROGRESS_DEADLINE_CODE,
                "conductor response did not start before the failover deadline",
                AgentFailureClass::ProviderTransient,
                true,
            ),
            COLLABORATION_TERMINATION_ATTEMPT,
        )
    })
}

#[cfg(test)]
pub(crate) fn collaboration_model_failure(
    error: &model_provider::ModelError,
    cancellation: Option<&Arc<AgentRunControl>>,
    stage_class: RunStageClass,
) -> AgentFailure {
    collaboration_control_failure(cancellation, stage_class, false, None)
        .map(|(failure, _)| failure)
        .unwrap_or_else(|| AgentFailure::from_model_error(error))
}


pub(crate) fn collaboration_candidate_models(
    config: &ProviderConfig,
    candidates: usize,
) -> Vec<String> {
    let limit = candidates.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS);
    let mut models = Vec::new();
    for role in [ModelRole::Planner, ModelRole::Executor] {
        let model = config.model_for_role(&role);
        if !model.trim().is_empty() && !models.iter().any(|existing| existing == &model) {
            models.push(model);
        }
        if models.len() >= limit {
            break;
        }
    }
    models
}


pub(crate) fn collaboration_role_hints(
    config: &ProviderConfig,
    worker_models: &[String],
) -> ConductorRoleHints {
    let fallback = |preferred: String, index: usize| {
        if worker_models.iter().any(|model| model == &preferred) {
            preferred
        } else {
            worker_models
                .get(index)
                .or_else(|| worker_models.first())
                .cloned()
                .unwrap_or(preferred)
        }
    };
    let planner = fallback(config.model_for_role(&ModelRole::Planner), 0);
    let executor = fallback(config.model_for_role(&ModelRole::Executor), 1);
    let reviewer = fallback(
        config.model_for_role(&ModelRole::Reviewer),
        worker_models.len().saturating_sub(1),
    );
    ConductorRoleHints {
        planner: planner.clone(),
        executor,
        reviewer,
        synthesizer: fallback(config.model_for_role(&ModelRole::Summarizer), 0),
    }
}

fn collaboration_failure_completion(
    failure: AgentFailure,
    termination_scope: Option<&str>,
    partial_content: Option<String>,
    latency_ms: u64,
    mut usage: Metadata,
) -> CollaborationCompletion {
    if let Some(scope) = termination_scope {
        usage.insert(
            COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
            scope.to_string(),
        );
    }
    CollaborationCompletion {
        content: None,
        partial_content,
        error: Some(failure.message.clone()),
        failure: Some(failure),
        latency_ms,
        usage,
        evidence: Vec::new(),
    }
}


#[allow(clippy::too_many_arguments)]
pub(crate) fn collaboration_stage_started_metadata(
    collaboration_id: &str,
    stage: &str,
    role: &ModelRole,
    model: &str,
    request_id: &str,
    attribution: AgentModelAttribution,
    stage_metadata: &Metadata,
) -> Result<Metadata, String> {
    let mut metadata = [
        ("collaboration_id".to_string(), collaboration_id.to_string()),
        ("request_id".to_string(), request_id.to_string()),
        ("stage".to_string(), stage.to_string()),
        ("role".to_string(), role_label(role).to_string()),
        ("model".to_string(), model.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    for (key, value) in stage_metadata {
        metadata.insert(key.clone(), value.clone());
    }
    attribution
        .insert_into(&mut metadata, model, role_label(role), stage)
        .map_err(|error| error.to_string())?;
    Ok(metadata)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_collaboration_stage_started(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: &ModelRole,
    model: &str,
    request_id: &str,
    attribution: AgentModelAttribution,
    stage_metadata: &Metadata,
) -> Result<(), String> {
    let metadata = collaboration_stage_started_metadata(
        collaboration_id,
        stage,
        role,
        model,
        request_id,
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
        EventKind::ModelRequestStarted,
        format!("Collaboration {stage} started"),
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn complete_collaboration_model_with_control(
    config: ProviderConfig,
    role: ModelRole,
    model: String,
    system_prompt: String,
    prompt: String,
    cancellation: Option<Arc<AgentRunControl>>,
    on_delta: impl FnMut(&str),
) -> CollaborationCompletion {
    let stage = role_label(&role).to_string();
    complete_collaboration_model_for_stage_with_control(
        config,
        stage,
        role,
        model,
        system_prompt,
        prompt,
        cancellation,
        on_delta,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_collaboration_model_for_stage_with_control(
    config: ProviderConfig,
    stage: String,
    role: ModelRole,
    model: String,
    system_prompt: String,
    prompt: String,
    cancellation: Option<Arc<AgentRunControl>>,
    on_delta: impl FnMut(&str),
) -> CollaborationCompletion {
    complete_collaboration_model_for_stage_with_recovery_control(
        config,
        stage,
        role,
        model,
        system_prompt,
        prompt,
        cancellation,
        CollaborationCallLimits::default(),
        None,
        on_delta,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_collaboration_model_for_stage_with_recovery_control(
    config: ProviderConfig,
    stage: String,
    role: ModelRole,
    model: String,
    system_prompt: String,
    prompt: String,
    cancellation: Option<Arc<AgentRunControl>>,
    limits: CollaborationCallLimits,
    resource_checkpoint: Option<&AgentResourceCheckpoint<'_>>,
    mut on_delta: impl FnMut(&str),
) -> CollaborationCompletion {
    let started_at_ms = current_time_millis();
    let role_name = role_label(&role).to_string();
    let inferred_stage_class = RunStageClass::from_label(&stage);
    let stage_class = if inferred_stage_class == RunStageClass::Other {
        RunStageClass::from_label(&role_name)
    } else {
        inferred_stage_class
    };
    let objective_epoch = limits
        .objective_epoch
        .or_else(|| cancellation.as_ref().map(|control| control.steer_epoch()));
    if let Some(control) = cancellation.as_ref() {
        let expected_epoch = objective_epoch.unwrap_or_else(|| control.steer_epoch());
        match control.begin_stage_model_call_at(expected_epoch, &stage, stage_class) {
            Ok(Some(_)) => {}
            Ok(None) => {
                let mut completion = CollaborationCompletion::failed_with(AgentFailure::cancelled(
                    "user_steer",
                    "collaboration model call superseded by user steering",
                ));
                completion.usage.insert(
                    COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
                    COLLABORATION_TERMINATION_STEER.to_string(),
                );
                return completion;
            }
            Err(reason) => {
                let scope = if reason == RunStopReason::StageBudgetExhausted {
                    COLLABORATION_TERMINATION_STAGE
                } else {
                    COLLABORATION_TERMINATION_RUN
                };
                let mut completion =
                    CollaborationCompletion::failed_with(AgentFailure::from_stop_reason(
                        reason,
                        format!("Run stopped before model call: {}", reason.code()),
                    ));
                completion.usage.insert(
                    COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
                    scope.to_string(),
                );
                return completion;
            }
        }
    }
    let timeout_seconds = cancellation.as_ref().map_or(180, |control| {
        limits.recovery_window.map_or_else(
            || control.stage_model_call_timeout_seconds(stage_class),
            |window| {
                control
                    .stage_model_call_timeout_with_recovery(stage_class, 1, window)
                    .as_secs()
                    .max(1)
            },
        )
    });
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds,
    });
    let mut partial_output = String::new();
    let mut stream_progress = ModelStreamProgress::new();
    let mut first_delta_at_ms = None;
    let response_started = AtomicBool::new(false);
    let attempt_deadline_reached = AtomicBool::new(false);
    let attempt_started_at = Instant::now();
    let provider_activity_progress_at = std::cell::Cell::new(Instant::now());
    let model_request = ModelRequest {
        role,
        messages: vec![
            Message {
                role: MessageRole::System,
                content: system_prompt,
                metadata: Metadata::new(),
            },
            Message {
                role: MessageRole::User,
                content: prompt,
                metadata: Metadata::new(),
            },
        ],
        tools: Vec::new(),
        mode: ModelCallMode::Streaming,
        metadata: [(
            "max_output_tokens".to_string(),
            COLLABORATION_MAX_OUTPUT_TOKENS.to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let model_attempt = if let Some(control) = cancellation.as_ref() {
        let expected_epoch = objective_epoch.unwrap_or_else(|| control.steer_epoch());
        match crate::model_resource_runtime::ControlledModelAttempt::reserve_at(
            control,
            expected_epoch,
            &model,
            &model_request,
            stage_class,
        ) {
            Ok(Some(attempt)) => Some(attempt),
            Ok(None) => {
                control.finish_model_call_at(expected_epoch);
                let mut completion = CollaborationCompletion::failed_with(AgentFailure::cancelled(
                    "user_steer",
                    "collaboration model call superseded by user steering",
                ));
                completion.usage.insert(
                    COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
                    COLLABORATION_TERMINATION_STEER.to_string(),
                );
                return completion;
            }
            Err(reason) => {
                control.finish_model_call_at(expected_epoch);
                let scope = if reason == RunStopReason::StageBudgetExhausted {
                    COLLABORATION_TERMINATION_STAGE
                } else {
                    COLLABORATION_TERMINATION_RUN
                };
                let mut completion =
                    CollaborationCompletion::failed_with(AgentFailure::from_stop_reason(
                        reason,
                        format!("Run stopped before provider dispatch: {}", reason.code()),
                    ));
                completion.usage.insert(
                    COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
                    scope.to_string(),
                );
                return completion;
            }
        }
    } else {
        None
    };
    if let (Some(control), Some(checkpoint)) = (cancellation.as_ref(), resource_checkpoint) {
        if let Err(error) = checkpoint(control) {
            if let Some(attempt) = model_attempt {
                let _ = attempt.settle_unknown();
            }
            control.finish_model_call_at(
                objective_epoch.expect("controlled collaboration call should capture its epoch"),
            );
            return CollaborationCompletion::failed_with(AgentFailure::internal(
                "resource_checkpoint_failed",
                format!(
                    "collaboration resource checkpoint failed before provider dispatch: {error}"
                ),
            ));
        }
    }
    let response = provider.complete_streaming_cancellable_with_activity(
        model_request,
        |delta| {
            if !delta.is_empty() {
                response_started.store(true, Ordering::Release);
                first_delta_at_ms.get_or_insert_with(current_time_millis);
                partial_output.push_str(delta);
            }
            if let Some(control) = cancellation.as_ref() {
                stream_progress.observe(
                    control,
                    objective_epoch.unwrap_or_else(|| control.steer_epoch()),
                    "model_stream",
                    &stage,
                    &partial_output,
                );
            }
            on_delta(delta);
        },
        || {
            if !limits.provider_activity_is_progress {
                return;
            }
            response_started.store(true, Ordering::Release);
            let now = Instant::now();
            if now.saturating_duration_since(provider_activity_progress_at.get())
                < Duration::from_millis(500)
            {
                return;
            }
            provider_activity_progress_at.set(now);
            if let Some(control) = cancellation.as_ref() {
                control.mark_progress_at(
                    objective_epoch.unwrap_or_else(|| control.steer_epoch()),
                    "model_stream",
                    &stage,
                );
            }
        },
        || {
            if cancellation.as_ref().is_some_and(|control| {
                collaboration_stage_should_interrupt(control, stage_class)
                    || objective_epoch.is_some_and(|expected_epoch| {
                        !control.preparation_epoch_is_current(expected_epoch)
                    })
            }) {
                return true;
            }
            if collaboration_no_progress_should_cancel(
                response_started.load(Ordering::Acquire),
                attempt_started_at.elapsed(),
                limits.no_progress_timeout,
            ) {
                attempt_deadline_reached.store(true, Ordering::Release);
                return true;
            }
            false
        },
    );
    if let Some(attempt) = model_attempt {
        match &response {
            Ok(response) => {
                let _ = attempt.settle_response(response);
            }
            Err(_) => {
                let _ = attempt.settle_unknown();
            }
        }
    }
    if let Some(control) = cancellation.as_ref() {
        if let Some(checkpoint) = resource_checkpoint {
            if let Err(error) = checkpoint(control) {
                eprintln!("collaboration resource settlement checkpoint unavailable: {error}");
            }
        }
        control.finish_model_call_at(
            objective_epoch.expect("controlled collaboration call should capture its epoch"),
        );
    }
    let latency_ms = current_time_millis().saturating_sub(started_at_ms);
    let attempt_deadline_reached = attempt_deadline_reached.load(Ordering::Acquire);
    match response {
        Ok(response) => {
            let mut usage = collaboration_usage_metadata(&response.metadata, &model);
            if let Some(first_delta_at_ms) = first_delta_at_ms {
                usage.insert(
                    "first_token_latency_ms".to_string(),
                    first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
                );
            }
            let parsed = no_tool_collaboration_content(response, &role_name);
            if let Some((failure, scope)) = collaboration_control_failure(
                cancellation.as_ref(),
                stage_class,
                attempt_deadline_reached,
                objective_epoch,
            ) {
                let partial = match parsed {
                    Ok(content) => Some(content),
                    Err(_) => (!partial_output.trim().is_empty()).then_some(partial_output),
                };
                return collaboration_failure_completion(
                    failure,
                    Some(scope),
                    partial,
                    latency_ms,
                    usage,
                );
            }
            let content = match parsed {
                Ok(content) => content,
                Err(failure) => {
                    if let Some(control) = cancellation.as_ref() {
                        control.record_observation_at(
                            objective_epoch.unwrap_or_else(|| control.steer_epoch()),
                            "model_protocol_error",
                            &role_name,
                            &failure.message,
                        );
                    }
                    return collaboration_failure_completion(
                        failure,
                        None,
                        (!partial_output.trim().is_empty()).then_some(partial_output),
                        latency_ms,
                        usage,
                    );
                }
            };
            if let Some(control) = cancellation.as_ref() {
                let objective_epoch = objective_epoch.unwrap_or_else(|| control.steer_epoch());
                control.record_partial_output_at(objective_epoch, &content);
                control.record_observation_at(objective_epoch, "model_result", &stage, &content);
                let quality = match stage_class {
                    RunStageClass::Reviewer => ResultQuality::Verified,
                    RunStageClass::Synthesizer | RunStageClass::Finalizer => {
                        ResultQuality::Synthesized
                    }
                    RunStageClass::Candidate | RunStageClass::Worker => ResultQuality::Substantive,
                    _ => ResultQuality::Draft,
                };
                control.record_best_known_result_at(
                    objective_epoch,
                    &stage,
                    &content,
                    quality,
                    0,
                    false,
                    matches!(
                        stage_class,
                        RunStageClass::Synthesizer | RunStageClass::Finalizer
                    ),
                );
            }
            CollaborationCompletion {
                content: Some(content),
                partial_content: None,
                error: None,
                failure: None,
                latency_ms,
                usage,
                evidence: Vec::new(),
            }
        }
        Err(error) => {
            let terminal = collaboration_control_failure(
                cancellation.as_ref(),
                stage_class,
                attempt_deadline_reached,
                objective_epoch,
            );
            let (failure, termination_scope) = terminal
                .map(|(failure, scope)| (failure, Some(scope)))
                .unwrap_or_else(|| (AgentFailure::from_model_error(&error), None));
            if let Some(control) = cancellation.as_ref() {
                control.record_observation_at(
                    objective_epoch.unwrap_or_else(|| control.steer_epoch()),
                    "provider_failure",
                    error.class.label(),
                    &error.message,
                );
            }
            let mut usage = Metadata::new();
            usage.insert(
                "provider_failure_class".to_string(),
                error.class.label().to_string(),
            );
            usage.insert(
                "provider_failure_retryable".to_string(),
                error.retryable.to_string(),
            );
            if let Some(status_code) = error.status_code {
                usage.insert("provider_status_code".to_string(), status_code.to_string());
            }
            collaboration_failure_completion(
                failure,
                termination_scope,
                (!partial_output.trim().is_empty()).then_some(partial_output),
                latency_ms,
                usage,
            )
        }
    }
}

fn collaboration_usage_metadata(response_metadata: &Metadata, configured_model: &str) -> Metadata {
    let mut usage = Metadata::from([("model".to_string(), configured_model.to_string())]);
    for key in [
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "usage_source",
        "usage_estimated",
        "provider_response_id",
        "provider_response_model",
        "provider_system_fingerprint",
        "request_payload_sha256",
        "response_semantic_sha256",
        "provider_receipt_status",
    ] {
        if let Some(value) = response_metadata.get(key) {
            usage.insert(key.to_string(), value.clone());
        }
    }
    usage
}

fn no_tool_collaboration_content(
    response: model_provider::ModelResponse,
    role_name: &str,
) -> Result<String, AgentFailure> {
    match response.assessment().disposition {
        model_provider::ModelResponseDisposition::ToolCalls => {
            return Err(AgentFailure::model_output(
                "unexpected_tool_calls",
                format!(
                    "collaboration {role_name} attempted {} tool call(s) in a no-tool stage",
                    response.tool_calls.len()
                ),
            ));
        }
        model_provider::ModelResponseDisposition::IncompleteOutput => {
            return Err(AgentFailure::model_output(
                "incomplete_output",
                format!(
                    "collaboration {role_name} response reached its output limit before completion"
                ),
            ));
        }
        model_provider::ModelResponseDisposition::Filtered => {
            return Err(AgentFailure::model_output(
                "filtered_output",
                format!("collaboration {role_name} response was blocked by the provider filter"),
            ));
        }
        model_provider::ModelResponseDisposition::Empty => {
            return Err(AgentFailure::model_output(
                "empty_output",
                format!("collaboration {role_name} returned an empty response"),
            ));
        }
        model_provider::ModelResponseDisposition::Usable => {}
    }
    Ok(response.message.content)
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    #[test]
    fn collaboration_usage_binds_the_configured_model_with_provider_receipts() {
        let response = Metadata::from([
            ("provider_response_id".to_string(), "response-1".to_string()),
            ("request_payload_sha256".to_string(), "a".repeat(64)),
            ("response_semantic_sha256".to_string(), "b".repeat(64)),
            (
                "provider_receipt_status".to_string(),
                "observed".to_string(),
            ),
        ]);

        let usage = collaboration_usage_metadata(&response, "configured-model");

        assert_eq!(usage["model"], "configured-model");
        assert_eq!(usage["provider_response_id"], "response-1");
        assert_eq!(usage["request_payload_sha256"], "a".repeat(64));
        assert_eq!(usage["response_semantic_sha256"], "b".repeat(64));
    }


    fn response_with_tool_call() -> model_provider::ModelResponse {
        model_provider::ModelResponse {
            message: agent_core::Message {
                role: agent_core::MessageRole::Assistant,
                content: String::new(),
                metadata: agent_core::Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: vec![model_provider::ModelToolCall {
                id: "call-dsml-0".to_string(),
                name: "shell_run".to_string(),
                arguments_json: r#"{"command":"pwd"}"#.to_string(),
            }],
            metadata: agent_core::Metadata::new(),
        }
    }

    #[test]
    fn no_tool_collaboration_stage_rejects_tool_calls() {
        let error = no_tool_collaboration_content(response_with_tool_call(), "synthesizer")
            .expect_err("tool calls must not be accepted as final content");

        assert_eq!(error.class, agent_runtime::AgentFailureClass::ModelOutput);
        assert_eq!(error.code, "unexpected_tool_calls");
        assert_eq!(
            error.message,
            "collaboration synthesizer attempted 1 tool call(s) in a no-tool stage"
        );
    }
}
