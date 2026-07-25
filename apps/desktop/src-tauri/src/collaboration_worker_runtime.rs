use super::*;

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
    cancellation: Option<Arc<AgentRunControl>>,
    branch_cancellation: Option<Arc<AtomicBool>>,
) -> CollaborationCompletion {
    let started_at_ms = current_time_millis();
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
    worker_context.insert("collaboration_id".to_string(), collaboration_id);
    worker_context.insert("stage".to_string(), stage.clone());
    worker_context.insert("role".to_string(), role_label(&role).to_string());
    worker_context.insert(
        "worker_runtime".to_string(),
        "isolated_evidence_v1".to_string(),
    );
    let mut evidence = Vec::new();
    let mut first_delta_at_ms = None;
    let stage_class = RunStageClass::Worker;

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
        if cancellation
            .as_ref()
            .is_some_and(collaboration_run_should_interrupt)
        {
            let message = if cancellation
                .as_ref()
                .is_some_and(|control| control.has_pending_steer())
            {
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
            COLLABORATION_MAX_OUTPUT_TOKENS,
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
                    control.record_partial_output(partial_answer);
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
            if let Err(reason) = control.begin_stage_model_call(&stage, stage_class) {
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
        let mut request = prepared_turn.turn.request;
        request.role = role.clone();
        request
            .metadata
            .insert("collaboration_worker".to_string(), "true".to_string());
        request.metadata.insert(
            "max_output_tokens".to_string(),
            max_output_tokens.to_string(),
        );
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
                    stream_progress.observe(control, "model_stream", &stage, &partial_output);
                }
            },
            || {
                cancellation.as_ref().is_some_and(|control| {
                    collaboration_stage_should_interrupt(control, stage_class)
                }) || branch_cancellation
                    .as_ref()
                    .is_some_and(|cancelled| cancelled.load(Ordering::SeqCst))
            },
        );
        if let Some(control) = cancellation.as_ref() {
            control.finish_model_call();
        }
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let failure = AgentFailure::from_model_error(&error);
                if let Some(control) = cancellation.as_ref() {
                    control.record_observation(
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
            if let Err(reason) = control.record_agent_turn(&stage) {
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
        if let Some(control) = cancellation.as_ref() {
            if let Some(evidence) = model_response_checkpoint_evidence(&response) {
                control.record_observation("model_result", &stage, &evidence);
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
                    control.record_partial_output(&answer);
                    control.record_best_known_result(
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
                    control.record_partial_output(partial_answer);
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
                        let result = registry.as_ref().ok_or_else(|| {
                            "collaboration worker tool registry is unavailable".to_string()
                        });
                        match result.and_then(|registry| {
                            execute_agent_tool_invocation(
                                &state,
                                registry,
                                invocation,
                                &workspace_root,
                                &worker_context,
                            )
                        }) {
                            Ok(result) => {
                                let status = result.status.clone();
                                evidence.push(CollaborationEvidence {
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
                    worker.apply_tool_observation(&call, &status, &observation);
                }
            }
        }
    }
}
