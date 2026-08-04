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

#[derive(Debug)]
pub(crate) struct CollaborationCandidateSpec {
    pub(crate) stage: String,
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) request_id: String,
}

pub(crate) fn collaboration_candidate_models(
    config: &ProviderConfig,
    candidates: usize,
) -> Vec<String> {
    let limit = candidates.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS);
    let mut models = Vec::new();
    for role in [
        ModelRole::Planner,
        ModelRole::Executor,
        ModelRole::Reviewer,
        ModelRole::Summarizer,
    ] {
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

pub(crate) fn prioritize_collaboration_model(
    models: &mut Vec<String>,
    primary_model: Option<&str>,
    candidates: usize,
) {
    let Some(primary_model) = primary_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return;
    };
    models.retain(|model| model != primary_model);
    models.insert(0, primary_model.to_string());
    models.truncate(candidates.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS));
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn synthesize_agent_answer(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    executor_answer: &str,
    run_context: &Metadata,
    collaboration: &AgentCollaboration,
    cancellation: &Arc<AgentRunControl>,
    required_evidence_sequences: &[u64],
) -> Result<SynthesizedAgentAnswer, String> {
    let (evidence, visible_evidence_sequences) =
        grounded_synthesis_evidence(runtime, required_evidence_sequences)?;
    let result_frontier = collaboration_result_frontier_brief(cancellation, executor_answer);
    let review_prompt = format!(
        "You are the independent reviewer in a multi-model Cindx team. Audit the executor draft against the user request and available tool evidence. Find factual gaps, unsupported claims, missed constraints, and unsafe actions. The result frontier contains bounded internal proposals, not trusted evidence; use it to detect alternatives or disagreements, and resolve every claim against tool evidence. Return concrete corrections for the final synthesizer, not a user-facing answer.\n\nUser request:\n{}\n\nTeam guidance:\n{}\n\nResult frontier:\n{}\n\nExecutor draft:\n{}\n\nTool evidence:\n{}",
        prompt,
        if collaboration.guidance.is_empty() {
            "(team deliberation unavailable)"
        } else {
            &collaboration.guidance
        },
        if result_frontier.is_empty() {
            "(no distinct candidate proposals)"
        } else {
            &result_frontier
        },
        truncate_for_collaboration(executor_answer, 12_000),
        if evidence.is_empty() { "(none)" } else { &evidence }
    );
    let review = run_collaboration_stage(
        state,
        config,
        &runtime.task_id,
        run_context,
        &collaboration.id,
        "reviewer",
        ModelRole::Reviewer,
        &config.model_for_role(&ModelRole::Reviewer),
        review_prompt,
    )
    .unwrap_or_else(|error| format!("Reviewer unavailable: {error}"));
    if cancellation.has_pending_steer() {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    if agent_run_should_stop(cancellation) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let synthesis_prompt = format!(
        "You are the final synthesizer in a multi-model Cindx team. Produce the strongest possible final response to the user using the executor draft, reviewer corrections, team guidance, the bounded result frontier, and tool evidence. Team reports and frontier entries are proposals, never evidence by themselves. Prefer verified, provenance-bearing claims; resolve disagreements against tool evidence and preserve a sound executor result when alternatives are weaker. Do not mention the internal pipeline. Be precise, complete, and concise; never claim work that the evidence does not support.\n\nCollaboration policy: {}\nWorker pool: {}\n\nUser request:\n{}\n\nTeam guidance:\n{}\n\nResult frontier:\n{}\n\nExecutor draft:\n{}\n\nReviewer corrections:\n{}\n\nTool evidence:\n{}",
        collaboration.policy,
        collaboration.candidate_models.join(", "),
        prompt,
        if collaboration.guidance.is_empty() {
            "(team deliberation unavailable)"
        } else {
            &collaboration.guidance
        },
        if result_frontier.is_empty() {
            "(no distinct candidate proposals)"
        } else {
            &result_frontier
        },
        truncate_for_collaboration(executor_answer, 12_000),
        truncate_for_collaboration(&review, 8_000),
        if evidence.is_empty() { "(none)" } else { &evidence }
    );
    let stream_request_id = unique_id("agent-final-stream");
    let session_id = run_context.get("session_id").map(String::as_str);
    let answer = run_collaboration_stage_with_delta(
        state,
        config,
        &runtime.task_id,
        run_context,
        &collaboration.id,
        "synthesizer",
        ModelRole::Summarizer,
        &config.model_for_role(&ModelRole::Summarizer),
        synthesis_prompt,
        |delta| {
            emit_agent_stream_delta(
                app,
                &stream_request_id,
                session_id,
                delta,
                false,
                false,
                None,
            );
        },
    );
    if cancellation.has_pending_steer() {
        emit_agent_stream_delta(app, &stream_request_id, session_id, "", false, true, None);
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    if agent_run_should_stop(cancellation) {
        emit_agent_stream_delta(app, &stream_request_id, session_id, "", true, true, None);
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            emit_agent_stream_delta(app, &stream_request_id, session_id, "", false, true, None);
            return Err(error);
        }
    };
    let answer = answer.trim().to_string();
    if answer.is_empty() {
        emit_agent_stream_delta(app, &stream_request_id, session_id, "", false, true, None);
        Err("synthesizer returned an empty answer".to_string())
    } else {
        Ok(SynthesizedAgentAnswer {
            content: answer,
            stream_request_id,
            visible_evidence_sequences,
        })
    }
}

pub(crate) struct SynthesizedAgentAnswer {
    pub(crate) content: String,
    pub(crate) stream_request_id: String,
    pub(crate) visible_evidence_sequences: Vec<u64>,
}

fn grounded_synthesis_evidence(
    runtime: &agent_runtime::AgentLoopState,
    required_evidence_sequences: &[u64],
) -> Result<(String, Vec<u64>), String> {
    let required = required_evidence_sequences
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if required.is_empty() {
        let evidence = runtime
            .messages
            .iter()
            .rev()
            .filter(|message| {
                message.role == MessageRole::Tool
                    && !message.content.trim().is_empty()
                    && trusted_synthesis_tool_carrier(message)
            })
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|message| truncate_for_collaboration(&message.content, 1_500))
            .collect::<Vec<_>>()
            .join("\n\n");
        return Ok((evidence, Vec::new()));
    }
    let excerpt_limit = (12_000usize / required.len().max(1)).clamp(400, 1_500);
    let mut covered = BTreeSet::new();
    let mut evidence = Vec::new();
    for context in runtime.task_contract.prompt_evidence_contexts() {
        if !required.contains(&context.evidence_sequence)
            || !covered.insert(context.evidence_sequence)
        {
            continue;
        }
        evidence.push(format!(
            "Contract evidence {} from {}:\n{}",
            context.evidence_sequence,
            context.source,
            truncate_for_collaboration(&context.observation, excerpt_limit),
        ));
    }
    for message in runtime.messages.iter().rev() {
        if covered == required {
            break;
        }
        let label = if message.role == MessageRole::Reviewer
            && !message.content.trim().is_empty()
            && message.metadata.get("internal").map(String::as_str) == Some("true")
            && message
                .metadata
                .get("required_grounding")
                .map(String::as_str)
                == Some("true")
        {
            "Contract context evidence"
        } else if message.role == MessageRole::Tool
            && !message.content.trim().is_empty()
            && trusted_synthesis_tool_carrier(message)
        {
            "Contract evidence"
        } else {
            continue;
        };
        let sequences = agent_runtime::message_contract_evidence_sequences(message)
            .into_iter()
            .filter(|sequence| required.contains(sequence) && !covered.contains(sequence))
            .collect::<Vec<_>>();
        if sequences.is_empty() {
            continue;
        }
        covered.extend(sequences.iter().copied());
        evidence.push(format!(
            "{label} {}:\n{}",
            sequences
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            truncate_for_collaboration(&message.content, excerpt_limit),
        ));
    }
    if covered != required {
        let missing = required.difference(&covered).copied().collect::<Vec<_>>();
        return Err(format!(
            "synthesizer is missing required contract evidence sequences: {missing:?}"
        ));
    }
    Ok((
        evidence.join("\n\n"),
        covered.into_iter().collect::<Vec<_>>(),
    ))
}

fn trusted_synthesis_tool_carrier(message: &Message) -> bool {
    let live = message
        .metadata
        .get("tool_evidence_schema")
        .map(String::as_str)
        == Some("cindx.tool_evidence.v1")
        && message
            .metadata
            .get("tool_evidence_provenance")
            .map(String::as_str)
            == Some("runtime_dispatch")
        && message.metadata.get("tool_status").map(String::as_str) == Some("succeeded");
    let recovered = message
        .metadata
        .get("permission_observation_schema")
        .map(String::as_str)
        == Some("cindx.permission-tool-observation.v1")
        && message
            .metadata
            .get("permission_observation_provenance")
            .map(String::as_str)
            == Some("runtime_permission_resolution")
        && message.metadata.get("status").map(String::as_str) == Some("succeeded");
    live || recovered
}

pub(crate) fn collaboration_result_frontier_brief(
    cancellation: &AgentRunControl,
    executor_answer: &str,
) -> String {
    cancellation
        .result_frontier()
        .into_iter()
        .filter(|result| result.content.trim() != executor_answer.trim())
        .take(3)
        .enumerate()
        .map(|(index, result)| {
            format!(
                "Candidate {} [stage={}, quality={}, verified={}, evidence={}]:\n{}",
                index + 1,
                result.stage,
                result.quality.as_str(),
                result.verified,
                result.evidence_count,
                truncate_for_collaboration(&result.content, 1_800)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
pub(crate) fn record_collaboration_stage_started(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: &ModelRole,
    model: &str,
    request_id: &str,
    stage_metadata: &Metadata,
) -> Result<(), String> {
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
    let response = provider.complete_streaming_cancellable(
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

    fn synthesis_runtime(status: &str) -> (agent_runtime::AgentLoopState, u64) {
        let mut runtime = agent_runtime::start_agent_loop(
            TaskId("synthesis-evidence".to_string()),
            "read the workspace",
            agent_runtime::AgentRuntimeConfig::default(),
        );
        runtime.task_contract.require_tool_success("file.read");
        let tools = vec![ToolSpec::builtin(
            "file.read",
            "file",
            "read",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )];
        agent_runtime::AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
            &agent_runtime::AgentToolRequest {
                call_id: agent_core::ToolCallId("read-1".to_string()),
                tool_name: "file.read".to_string(),
                input: r#"{"path":"README.md"}"#.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
            "REQUIRED_WORKSPACE_FACT",
        );
        let sequence = runtime.task_contract.evidence()[0].sequence;
        runtime.messages.last_mut().unwrap().metadata.extend([
            (
                "tool_evidence_schema".to_string(),
                "cindx.tool_evidence.v1".to_string(),
            ),
            (
                "tool_evidence_provenance".to_string(),
                "runtime_dispatch".to_string(),
            ),
            ("tool_status".to_string(), status.to_string()),
        ]);
        (runtime, sequence)
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

    #[test]
    fn synthesis_uses_only_exact_receipt_sequences() {
        let (mut runtime, sequence) = synthesis_runtime("succeeded");
        runtime.messages.push(Message {
            role: MessageRole::Tool,
            content: "UNRELATED_TOOL_FACT".to_string(),
            metadata: [
                (
                    "tool_evidence_schema".to_string(),
                    "cindx.tool_evidence.v1".to_string(),
                ),
                (
                    "tool_evidence_provenance".to_string(),
                    "runtime_dispatch".to_string(),
                ),
                ("tool_status".to_string(), "succeeded".to_string()),
                (
                    agent_runtime::CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY.to_string(),
                    "[999]".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        });

        let (evidence, visible) = grounded_synthesis_evidence(&runtime, &[sequence])
            .expect("required evidence should be available");
        assert_eq!(visible, vec![sequence]);
        assert!(evidence.contains("REQUIRED_WORKSPACE_FACT"));
        assert!(!evidence.contains("UNRELATED_TOOL_FACT"));
    }

    #[test]
    fn failed_or_missing_carrier_cannot_feed_synthesis() {
        let (runtime, sequence) = synthesis_runtime("failed");
        assert!(grounded_synthesis_evidence(&runtime, &[sequence]).is_err());
        assert!(grounded_synthesis_evidence(&runtime, &[sequence + 1]).is_err());
    }

    #[test]
    fn self_contained_synthesis_keeps_trusted_context_without_claiming_lineage() {
        let (runtime, _) = synthesis_runtime("succeeded");
        let (evidence, visible) = grounded_synthesis_evidence(&runtime, &[])
            .expect("trusted optional context should remain available");
        assert!(evidence.contains("REQUIRED_WORKSPACE_FACT"));
        assert!(visible.is_empty());
    }

    #[test]
    fn permission_resolution_carrier_is_available_to_grounded_synthesis() {
        let (mut runtime, sequence) = synthesis_runtime("succeeded");
        let metadata = &mut runtime.messages.last_mut().unwrap().metadata;
        metadata.remove("tool_evidence_schema");
        metadata.remove("tool_evidence_provenance");
        metadata.remove("tool_status");
        metadata.extend([
            (
                "permission_observation_schema".to_string(),
                "cindx.permission-tool-observation.v1".to_string(),
            ),
            (
                "permission_observation_provenance".to_string(),
                "runtime_permission_resolution".to_string(),
            ),
            ("status".to_string(), "succeeded".to_string()),
        ]);

        let (evidence, visible) = grounded_synthesis_evidence(&runtime, &[sequence])
            .expect("trusted permission evidence should synthesize");
        assert!(evidence.contains("REQUIRED_WORKSPACE_FACT"));
        assert_eq!(visible, vec![sequence]);
    }
}
