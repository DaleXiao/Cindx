use super::*;
use crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA;

fn record_worker_goal_delta(
    control: Option<&AgentRunControl>,
    objective_epoch: u64,
    goal_delta: Option<&agent_runtime::AgentGoalDelta>,
) -> bool {
    control
        .zip(goal_delta)
        .is_some_and(|(control, delta)| control.record_goal_delta_at(objective_epoch, delta))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_collaboration_worker_with_tools(
    app: tauri::AppHandle,
    config: ProviderConfig,
    task_id: TaskId,
    workspace_root: PathBuf,
    run_context: Metadata,
    collaboration_id: String,
    stage: String,
    role: ModelRole,
    model: String,
    prompt: String,
    allow_tools: bool,
    max_model_turns: usize,
    max_tool_calls: usize,
    max_output_tokens: u64,
    cancellation: Option<Arc<AgentRunControl>>,
    branch_cancellation: Option<Arc<AtomicBool>>,
) -> CollaborationCompletion {
    let started_at_ms = current_time_millis();
    let objective_epoch = run_context_steer_epoch(&run_context);
    let state = app.state::<AppState>();
    let registry = if allow_tools {
        match tool_registry_for_state(&state, &workspace_root) {
            Ok(registry) => Some(registry),
            Err(error) => return CollaborationCompletion::failed(error),
        }
    } else {
        None
    };
    let tools = if let Some(registry) = registry.as_ref() {
        evidence_worker_tools(
            &registry
                .exposure_plan(&prompt, config.context_window_tokens)
                .inline,
        )
    } else {
        Vec::new()
    };
    let evidence_turn_limit = max_model_turns.max(1);
    let mut worker = IsolatedWorkerRuntime::new(
        task_id,
        prompt.clone(),
        tools,
        evidence_turn_limit,
        max_tool_calls,
    );
    if allow_tools {
        worker.require_substantive_evidence();
    }
    let mut trusted_context = agent_runtime_context_for_run(&run_context).unwrap_or_default();
    if !trusted_context.is_empty() {
        trusted_context.push('\n');
    }
    trusted_context.push_str(&format!(
        "Collaboration: {collaboration_id}\nWorker stage: {stage}\nWorker role: {}\n{} Evidence budget: {evidence_turn_limit} tool-capable model rounds and {max_tool_calls} total tool calls. Batch independent reads, prefer decisive file reads or searches over repeated directory listings, and stop gathering evidence as soon as the assigned question is answerable. Return concise conclusions for downstream workers; do not claim workspace changes.",
        role_label(&role),
        if allow_tools {
            "This worker has an isolated transcript and may use only exposed read-only evidence tools."
        } else {
            "This is a bounded worker with no tools. Reason only from the supplied request and recent context, then finish in one response."
        }
    ));
    let mut worker_context = run_context.clone();
    let evidence_source = stage.clone();
    worker_context.insert("collaboration_id".to_string(), collaboration_id.clone());
    worker_context.insert("stage".to_string(), stage.clone());
    worker_context.insert("role".to_string(), role_label(&role).to_string());
    worker_context.insert(
        "worker_runtime".to_string(),
        "isolated_evidence_v1".to_string(),
    );
    let mut evidence = Vec::new();
    let mut first_delta_at_ms = None;
    let stage_class = collaboration_worker_stage_class(&stage, &role);

    loop {
        if branch_cancellation
            .as_ref()
            .is_some_and(|cancelled| cancelled.load(Ordering::SeqCst))
        {
            let failure = AgentFailure::cancelled(
                "branch_quorum_cancelled",
                "collaboration branch cancelled after quorum",
            );
            return CollaborationCompletion::failed_worker(
                failure,
                None,
                current_time_millis().saturating_sub(started_at_ms),
                worker.completion_usage("isolated_evidence_v2"),
                evidence,
            );
        }
        if cancellation.as_ref().is_some_and(|control| {
            collaboration_run_should_interrupt(control)
                || !control.preparation_epoch_is_current(objective_epoch)
        }) {
            let message = if cancellation.as_ref().is_some_and(|control| {
                control.has_pending_steer()
                    || !control.preparation_epoch_is_current(objective_epoch)
            }) {
                COLLABORATION_STEER_INTERRUPTED
            } else {
                MODEL_REQUEST_CANCELLED
            };
            let failure = AgentFailure::cancelled("collaboration_interrupted", message);
            return CollaborationCompletion::failed_worker(
                failure,
                None,
                current_time_millis().saturating_sub(started_at_ms),
                worker.completion_usage("isolated_evidence_v2"),
                evidence,
            );
        }

        let timeout_seconds = cancellation
            .as_ref()
            .map(|control| control.stage_model_call_timeout_seconds(stage_class))
            .unwrap_or(180);
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            model: model.clone(),
            embedding_model: config.model_for_role(&ModelRole::Embedder),
            timeout_seconds,
        });

        let max_output_tokens = bounded_max_output_tokens(
            config.context_window_tokens,
            max_output_tokens.clamp(256, COLLABORATION_MAX_OUTPUT_TOKENS),
        );
        let prepared_turn = match worker.prepare_model_turn(
            Some(&config.agent_system_prompt),
            Some(&trusted_context),
            config.context_window_tokens,
            max_output_tokens,
        ) {
            Ok(prepared_turn) => prepared_turn,
            Err(failed) => {
                if let (Some(control), Some(partial_answer)) =
                    (cancellation.as_ref(), failed.partial_content.as_ref())
                {
                    control.record_partial_output_at(objective_epoch, partial_answer);
                }
                return CollaborationCompletion::failed_worker(
                    failed.failure,
                    failed.partial_content,
                    current_time_millis().saturating_sub(started_at_ms),
                    worker.completion_usage("isolated_evidence_v2"),
                    evidence,
                );
            }
        };
        if let Some(control) = cancellation.as_ref() {
            match control.begin_stage_model_call_at(objective_epoch, &stage, stage_class) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return CollaborationCompletion::failed_worker(
                        AgentFailure::cancelled(
                            "user_steer",
                            "worker model call superseded by user steering",
                        ),
                        None,
                        current_time_millis().saturating_sub(started_at_ms),
                        worker.completion_usage("isolated_evidence_v2"),
                        evidence,
                    )
                }
                Err(reason) => {
                    let failure = AgentFailure::from_stop_reason(
                        reason,
                        format!("Run stopped before worker model call: {}", reason.code()),
                    );
                    return CollaborationCompletion::failed_worker(
                        failure,
                        None,
                        current_time_millis().saturating_sub(started_at_ms),
                        worker.completion_usage("isolated_evidence_v2"),
                        evidence,
                    );
                }
            }
        }
        let mut request = prepared_turn.turn.request;
        request.role = role.clone();
        request
            .metadata
            .insert("collaboration_worker".to_string(), "true".to_string());
        request.metadata.insert(
            "max_output_tokens".to_string(),
            max_output_tokens.to_string(),
        );
        let model_attempt = if let Some(control) = cancellation.as_ref() {
            match crate::model_resource_runtime::ControlledModelAttempt::reserve_at(
                control,
                objective_epoch,
                &model,
                &request,
                stage_class,
            ) {
                Ok(Some(attempt)) => Some(attempt),
                Ok(None) => {
                    control.finish_model_call_at(objective_epoch);
                    return CollaborationCompletion::failed_worker(
                        AgentFailure::cancelled(
                            "user_steer",
                            "worker provider dispatch superseded by user steering",
                        ),
                        None,
                        current_time_millis().saturating_sub(started_at_ms),
                        worker.completion_usage("isolated_evidence_v2"),
                        evidence,
                    );
                }
                Err(reason) => {
                    control.finish_model_call_at(objective_epoch);
                    return CollaborationCompletion::failed_worker(
                        AgentFailure::from_stop_reason(
                            reason,
                            format!("worker stopped before provider dispatch: {}", reason.code()),
                        ),
                        None,
                        current_time_millis().saturating_sub(started_at_ms),
                        worker.completion_usage("isolated_evidence_v2"),
                        evidence,
                    );
                }
            }
        } else {
            None
        };
        if let Some(control) = cancellation.as_ref() {
            if let Err(error) = crate::agent_resource_snapshot::checkpoint_agent_run_resources(
                &state,
                &worker_context,
                control,
            ) {
                if let Some(attempt) = model_attempt {
                    let _ = attempt.settle_unknown();
                }
                control.finish_model_call_at(objective_epoch);
                return CollaborationCompletion::failed_worker(
                    AgentFailure::internal(
                        "resource_checkpoint_failed",
                        format!(
                            "worker resource checkpoint failed before provider dispatch: {error}"
                        ),
                    ),
                    None,
                    current_time_millis().saturating_sub(started_at_ms),
                    worker.completion_usage("isolated_evidence_v2"),
                    evidence,
                );
            }
        }
        let mut partial_output = String::new();
        let mut stream_progress = ModelStreamProgress::new();
        let response = provider.complete_streaming_cancellable(
            request,
            |delta| {
                if !delta.is_empty() {
                    first_delta_at_ms.get_or_insert_with(current_time_millis);
                    partial_output.push_str(delta);
                }
                if let Some(control) = cancellation.as_ref() {
                    stream_progress.observe(
                        control,
                        objective_epoch,
                        "model_stream",
                        &stage,
                        &partial_output,
                    );
                }
            },
            || {
                cancellation.as_ref().is_some_and(|control| {
                    collaboration_stage_should_interrupt(control, stage_class)
                        || !control.preparation_epoch_is_current(objective_epoch)
                }) || branch_cancellation
                    .as_ref()
                    .is_some_and(|cancelled| cancelled.load(Ordering::SeqCst))
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
            if let Err(error) = crate::agent_resource_snapshot::checkpoint_agent_run_resources(
                &state,
                &worker_context,
                control,
            ) {
                eprintln!("worker resource settlement checkpoint unavailable: {error}");
            }
            control.finish_model_call_at(objective_epoch);
        }
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let failure = AgentFailure::from_model_error(&error);
                if let Some(control) = cancellation.as_ref() {
                    control.record_observation_at(
                        objective_epoch,
                        "provider_failure",
                        error.class.label(),
                        &error.message,
                    );
                }
                worker.insert_usage("provider_failure_class", error.class.label());
                worker.insert_usage("provider_failure_retryable", error.retryable.to_string());
                if let Some(status_code) = error.status_code {
                    worker.insert_usage("provider_status_code", status_code.to_string());
                }
                return CollaborationCompletion::failed_worker(
                    failure,
                    (!partial_output.trim().is_empty()).then_some(partial_output),
                    current_time_millis().saturating_sub(started_at_ms),
                    worker.completion_usage("isolated_evidence_v2"),
                    evidence,
                );
            }
        };
        if let Some(control) = cancellation.as_ref() {
            match control.record_agent_turn_at(objective_epoch, &stage) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return CollaborationCompletion::failed_worker(
                        AgentFailure::cancelled(
                            "user_steer",
                            "worker model turn superseded by user steering",
                        ),
                        None,
                        current_time_millis().saturating_sub(started_at_ms),
                        worker.completion_usage("isolated_evidence_v2"),
                        evidence,
                    )
                }
                Err(reason) => {
                    let failure = AgentFailure::from_stop_reason(
                        reason,
                        format!("Run stopped after worker model turn: {}", reason.code()),
                    );
                    return CollaborationCompletion::failed_worker(
                        failure,
                        None,
                        current_time_millis().saturating_sub(started_at_ms),
                        worker.completion_usage("isolated_evidence_v2"),
                        evidence,
                    );
                }
            }
        }
        if let Some(control) = cancellation.as_ref() {
            if let Some(evidence) = model_response_checkpoint_evidence(&response) {
                control.record_observation_at(objective_epoch, "model_result", &stage, &evidence);
            }
        }

        if let Some(first_delta_at_ms) = first_delta_at_ms {
            worker.insert_usage(
                "first_token_latency_ms",
                first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
            );
        }

        match worker.advance_model_response(response) {
            WorkerAdvance::Completed { answer } => {
                if let Some(control) = cancellation.as_ref() {
                    control.record_partial_output_at(objective_epoch, &answer);
                    control.record_best_known_result_at(
                        objective_epoch,
                        &stage,
                        &answer,
                        if evidence.is_empty() {
                            ResultQuality::Substantive
                        } else {
                            ResultQuality::Grounded
                        },
                        evidence.len(),
                        false,
                        false,
                    );
                }
                return CollaborationCompletion::completed_worker(
                    answer,
                    current_time_millis().saturating_sub(started_at_ms),
                    worker.completion_usage("isolated_evidence_v2"),
                    evidence,
                );
            }
            WorkerAdvance::Failed(failed) => {
                if let (Some(control), Some(partial_answer)) =
                    (cancellation.as_ref(), failed.partial_content.as_ref())
                {
                    control.record_partial_output_at(objective_epoch, partial_answer);
                }
                return CollaborationCompletion::failed_worker(
                    failed.failure,
                    failed.partial_content,
                    current_time_millis().saturating_sub(started_at_ms),
                    worker.completion_usage("isolated_evidence_v2"),
                    evidence,
                );
            }
            WorkerAdvance::Retry => continue,
            WorkerAdvance::ToolCalls { calls } => {
                for call in calls {
                    let tool_call_id = call.call_id.0.clone();
                    let mut invocation = worker.tool_invocation(&call);
                    invocation.proposed_by_model = "collaboration-worker".to_string();
                    invocation
                        .metadata
                        .insert("collaboration_worker".to_string(), "true".to_string());
                    {
                        let mut store = match state.store.lock() {
                            Ok(store) => store,
                            Err(error) => {
                                return CollaborationCompletion::failed(format!(
                                    "store lock poisoned: {error}"
                                ))
                            }
                        };
                        if let Err(error) = append_tool_proposed_event(
                            &mut store,
                            &invocation,
                            Some(&worker_context),
                        ) {
                            return CollaborationCompletion::failed(error.to_string());
                        }
                    }

                    let admission = worker.admit_tool_call(&call);
                    let (status, observation) = if let WorkerToolAdmission::Denied {
                        kind,
                        reason,
                    } = admission
                    {
                        let observation =
                            observation_from_tool_result(&call.tool_name, "failed", reason);
                        evidence.push(CollaborationEvidence {
                            evidence_schema: COLLABORATION_TOOL_EVIDENCE_SCHEMA.to_string(),
                            steer_epoch: Some(objective_epoch),
                            collaboration_id: collaboration_id.clone(),
                            source_step: evidence_source.clone(),
                            tool_call_id: tool_call_id.clone(),
                            tool_name: call.tool_name.clone(),
                            request: call.input.clone(),
                            status: "failed".to_string(),
                            output: truncate_for_collaboration(reason, 2_000),
                        });
                        let mut store = match state.store.lock() {
                            Ok(store) => store,
                            Err(error) => {
                                return CollaborationCompletion::failed(format!(
                                    "store lock poisoned: {error}"
                                ))
                            }
                        };
                        if let Err(error) = append_tool_finished_event(
                            &mut store,
                            worker.task_id(),
                            &call.call_id.0,
                            &call.tool_name,
                            "failed",
                            &observation,
                            [("failure_code".to_string(), kind.code().to_string())]
                                .into_iter()
                                .collect(),
                            Some(&worker_context),
                        ) {
                            return CollaborationCompletion::failed(error.to_string());
                        }
                        (ToolOutcomeStatus::Failed, observation)
                    } else {
                        let result = match registry.as_ref() {
                            Some(registry) => {
                                if let Err(error) =
                                    registry.permissionless_read_tool(&invocation)
                                {
                                    Ok(ToolResult::text(
                                        invocation.id.clone(),
                                        ToolOutcomeStatus::Denied,
                                        error.message,
                                        Metadata::new(),
                                    ))
                                } else if let Some(control) = cancellation.as_ref() {
                                    match execute_agent_tool_invocation_for_objective_epoch(
                                        &state,
                                        registry,
                                        invocation,
                                        &workspace_root,
                                        &worker_context,
                                        control,
                                        objective_epoch,
                                    ) {
                                        Ok(AgentToolInvocationOutcome::Completed(result)) => {
                                            Ok(*result)
                                        }
                                        Ok(AgentToolInvocationOutcome::RestartAfterSteer) => {
                                            let failure = AgentFailure::cancelled(
                                                "collaboration_interrupted",
                                                COLLABORATION_STEER_INTERRUPTED,
                                            );
                                            return CollaborationCompletion::failed_worker(
                                                failure,
                                                None,
                                                current_time_millis()
                                                    .saturating_sub(started_at_ms),
                                                worker.completion_usage("isolated_evidence_v2"),
                                                evidence,
                                            );
                                        }
                                        Err(error) => Err(error),
                                    }
                                } else {
                                    execute_agent_tool_invocation(
                                        &state,
                                        registry,
                                        invocation,
                                        &workspace_root,
                                        &worker_context,
                                    )
                                }
                            }
                            None => Err(
                                "collaboration worker tool registry is unavailable".to_string(),
                            ),
                        };
                        match result {
                            Ok(result) => {
                                let status = result.status.clone();
                                evidence.push(CollaborationEvidence {
                                    evidence_schema: COLLABORATION_TOOL_EVIDENCE_SCHEMA.to_string(),
                                    steer_epoch: Some(objective_epoch),
                                    collaboration_id: collaboration_id.clone(),
                                    source_step: evidence_source.clone(),
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: call.tool_name.clone(),
                                    request: call.input.clone(),
                                    status: tool_outcome_label(&result.status).to_string(),
                                    output: truncate_for_collaboration(&result.output, 2_000),
                                });
                                (
                                    status,
                                    observation_from_agent_tool_result(&call.tool_name, &result),
                                )
                            }
                            Err(error) => {
                                evidence.push(CollaborationEvidence {
                                    evidence_schema: COLLABORATION_TOOL_EVIDENCE_SCHEMA.to_string(),
                                    steer_epoch: Some(objective_epoch),
                                    collaboration_id: collaboration_id.clone(),
                                    source_step: evidence_source.clone(),
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: call.tool_name.clone(),
                                    request: call.input.clone(),
                                    status: "failed".to_string(),
                                    output: truncate_for_collaboration(&error, 2_000),
                                });
                                (
                                    ToolOutcomeStatus::Failed,
                                    observation_from_tool_result(&call.tool_name, "failed", &error),
                                )
                            }
                        }
                    };
                    let goal_delta = worker.apply_tool_observation(&call, &status, &observation);
                    record_worker_goal_delta(
                        cancellation.as_deref(),
                        objective_epoch,
                        goal_delta.as_ref(),
                    );
                }
            }
        }
    }
}

pub(crate) fn collaboration_worker_stage_class(stage: &str, role: &ModelRole) -> RunStageClass {
    let role_stage_class = RunStageClass::from_label(role_label(role));
    let inferred_stage_class = RunStageClass::from_label(stage);
    if role_stage_class.is_terminal() {
        role_stage_class
    } else if inferred_stage_class != RunStageClass::Other {
        inferred_stage_class
    } else {
        RunStageClass::Worker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collaboration_worker_records_only_current_goal_delta() {
        let mut worker = IsolatedWorkerRuntime::new(
            TaskId("worker-goal-delta".to_string()),
            "inspect the workspace",
            vec![ToolSpec::builtin(
                "file.read",
                "file",
                "Read a file",
                ToolRisk::ReadOnly,
                r#"{"type":"object"}"#,
            )],
            1,
            1,
        );
        assert_eq!(worker.require_substantive_evidence(), 1);
        let request = AgentToolRequest {
            call_id: agent_core::ToolCallId("worker-read".to_string()),
            tool_name: "file.read".to_string(),
            input: r#"{"path":"README.md"}"#.to_string(),
        };
        let delta = worker
            .apply_tool_observation(
                &request,
                &ToolOutcomeStatus::Succeeded,
                "README.md\nproject evidence",
            )
            .expect("substantive worker evidence should emit a goal delta");

        let control = AgentRunControl::new("auto");
        assert!(record_worker_goal_delta(Some(&control), 0, Some(&delta)));
        assert!(!record_worker_goal_delta(Some(&control), 0, Some(&delta)));

        let stale_control = AgentRunControl::new("auto");
        assert!(!record_worker_goal_delta(
            Some(&stale_control),
            1,
            Some(&delta),
        ));
        assert!(!record_worker_goal_delta(None, 0, Some(&delta)));
    }
}
