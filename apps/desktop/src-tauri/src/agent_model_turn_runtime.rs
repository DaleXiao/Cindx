use super::*;
use crate::agent_failure_terminal_runtime::{
    commit_agent_failure_terminal, AgentFailureTerminalOutcome,
};
use model_provider::ModelError;

fn prepared_streaming_request_once<'a>(
    provider: &dyn StreamingModelProvider,
    request: &ModelRequest,
    prepared: &'a mut Option<PreparedStreamingModelRequest>,
) -> Result<&'a PreparedStreamingModelRequest, ModelError> {
    if prepared.is_none() {
        *prepared = Some(provider.prepare_streaming_request(request)?);
    }
    Ok(prepared
        .as_ref()
        .expect("prepared request slot must be set"))
}

pub(crate) struct AgentModelTurnResponse {
    pub response: ModelResponse,
    pub request_id: String,
    pub streamed_output: bool,
    pub epoch_lease: agent_runtime::RunEpochLease,
}

pub(crate) enum AgentModelTurnOutcome {
    Response(AgentModelTurnResponse),
    RestartAfterSteer,
    Finished(Box<AgentState>),
}

fn finished_agent_turn(state: AgentState) -> AgentModelTurnOutcome {
    AgentModelTurnOutcome::Finished(Box::new(state))
}

fn stop_parent_run_for_model_call_failure(cancellation: &AgentRunControl, reason: RunStopReason) {
    cancellation.request_stop(reason);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_agent_model_turn(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    provider: &dyn StreamingModelProvider,
    agent_model: &str,
    tools: &[ToolSpec],
    request: ModelRequest,
    context_governor: &ContextGovernorReport,
    terminal_commit: bool,
) -> Result<AgentModelTurnOutcome, String> {
    let session_id = run_context.get("session_id").map(String::as_str);
    let epoch_lease = match cancellation.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        agent_runtime::RunEpochLeaseOutcome::RestartAfterSteer => {
            return Ok(AgentModelTurnOutcome::RestartAfterSteer)
        }
        agent_runtime::RunEpochLeaseOutcome::Stopped(_) => {
            return Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                runtime,
                prompt,
                run_context,
                collaboration,
                cancellation,
            )?))
        }
        agent_runtime::RunEpochLeaseOutcome::TerminalCommitted => {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            return Ok(finished_agent_turn(
                agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string())?,
            ));
        }
    };
    let model_call = if collaboration.is_some() || terminal_commit {
        cancellation.begin_stage_model_call_at(
            epoch_lease.epoch(),
            "executor",
            RunStageClass::Finalizer,
        )
    } else {
        cancellation.begin_model_call_at(epoch_lease.epoch(), "executor")
    };
    match model_call {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(AgentModelTurnOutcome::RestartAfterSteer),
        Err(reason) => {
            stop_parent_run_for_model_call_failure(cancellation, reason);
            return Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                runtime,
                prompt,
                run_context,
                collaboration,
                cancellation,
            )?));
        }
    }
    if !cancellation.execution_epoch_lease_is_current(epoch_lease) {
        cancellation.finish_model_call_at(epoch_lease.epoch());
        if agent_run_should_stop(cancellation) {
            return Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                runtime,
                prompt,
                run_context,
                collaboration,
                cancellation,
            )?));
        }
        return Ok(AgentModelTurnOutcome::RestartAfterSteer);
    }

    let request_id = unique_id("agent-model");
    let started_at_ms = current_time_millis();
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        if agent_task_is_cancelled(&mut store, session_id).map_err(|error| error.to_string())? {
            let agent_state = agent_state_for_session(&store, None, session_id)
                .map_err(|error| error.to_string())?;
            drop(store);
            cancellation.finish_model_call_at(epoch_lease.epoch());
            return Ok(finished_agent_turn(agent_state));
        }
        let mut metadata = [
            ("request_id".to_string(), request_id.clone()),
            ("turn".to_string(), runtime.turn.to_string()),
            ("model".to_string(), agent_model.to_string()),
            ("tool_count".to_string(), tools.len().to_string()),
            ("prompt".to_string(), prompt.to_string()),
            (
                "context_governor_applied".to_string(),
                context_governor.applied.to_string(),
            ),
            (
                "context_original_tokens".to_string(),
                context_governor.estimated_original_tokens.to_string(),
            ),
            (
                "context_projected_tokens".to_string(),
                context_governor.estimated_projected_tokens.to_string(),
            ),
            (
                "context_input_budget_tokens".to_string(),
                context_governor.input_budget_tokens.to_string(),
            ),
            (
                "context_omitted_messages".to_string(),
                context_governor.omitted_messages.to_string(),
            ),
            ("terminal_commit".to_string(), terminal_commit.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        if let Some(collaboration) = collaboration {
            metadata.insert("collaboration_id".to_string(), collaboration.id.clone());
            metadata.insert("stage".to_string(), "executor".to_string());
            metadata.insert("role".to_string(), "executor".to_string());
            metadata.insert("policy".to_string(), collaboration.policy.clone());
        }
        append_event(
            &mut store,
            &runtime.task_id,
            EventKind::ModelRequestStarted,
            "Agent model turn started",
            metadata_with_context(metadata, run_context),
        )
        .map_err(|error| error.to_string())?;
    }

    let visible_stream = collaboration.is_none();
    let mut streamed_output = false;
    let mut partial_stream = String::new();
    let mut stream_progress = ModelStreamProgress::new();
    let mut first_delta_at_ms = None;
    let mut transport_attempt = 0usize;
    let resource_stage = if collaboration.is_some() || terminal_commit {
        RunStageClass::Finalizer
    } else {
        RunStageClass::Other
    };
    let mut prepared_request = None;
    let mut response = loop {
        transport_attempt += 1;
        partial_stream.clear();
        stream_progress.reset();
        let model_attempt = match crate::model_resource_runtime::ControlledModelAttempt::reserve_at(
            cancellation,
            epoch_lease.epoch(),
            agent_model,
            &request,
            resource_stage,
        ) {
            Ok(Some(attempt)) => attempt,
            Ok(None) => {
                cancellation.finish_model_call_at(epoch_lease.epoch());
                return Ok(AgentModelTurnOutcome::RestartAfterSteer);
            }
            Err(_) => {
                cancellation.finish_model_call_at(epoch_lease.epoch());
                return Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    runtime,
                    prompt,
                    run_context,
                    collaboration,
                    cancellation,
                )?));
            }
        };
        if let Err(error) = crate::agent_resource_snapshot::checkpoint_agent_run_resources(
            state,
            run_context,
            cancellation,
        ) {
            let _ = model_attempt.settle_unknown();
            cancellation.finish_model_call_at(epoch_lease.epoch());
            return Err(format!(
                "agent resource checkpoint failed before provider dispatch: {error}"
            ));
        }
        let mut on_delta = |delta: &str| {
            if !delta.is_empty() {
                first_delta_at_ms.get_or_insert_with(current_time_millis);
                partial_stream.push_str(delta);
                stream_progress.observe(
                    cancellation,
                    epoch_lease.epoch(),
                    "model_stream",
                    "executor",
                    &partial_stream,
                );
            }
            if visible_stream && !delta.is_empty() {
                streamed_output = true;
                emit_agent_stream_delta(app, &request_id, session_id, delta, false, false, None);
            }
        };
        let mut should_cancel = || {
            agent_run_should_stop(cancellation)
                || !cancellation.execution_epoch_lease_is_current(epoch_lease)
        };
        let result = prepared_streaming_request_once(provider, &request, &mut prepared_request)
            .and_then(|prepared| {
                provider.complete_prepared_streaming_cancellable(
                    prepared,
                    &mut on_delta,
                    &mut should_cancel,
                )
            });
        match result {
            Ok(response) => {
                let _ = model_attempt.settle_response(&response);
                if let Err(error) = crate::agent_resource_snapshot::checkpoint_agent_run_resources(
                    state,
                    run_context,
                    cancellation,
                ) {
                    eprintln!("agent resource settlement checkpoint unavailable: {error}");
                }
                break response;
            }
            Err(error) => {
                let _ = model_attempt.settle_unknown();
                if let Err(checkpoint_error) =
                    crate::agent_resource_snapshot::checkpoint_agent_run_resources(
                        state,
                        run_context,
                        cancellation,
                    )
                {
                    eprintln!(
                        "agent resource settlement checkpoint unavailable: {checkpoint_error}"
                    );
                }
                if !cancellation.execution_epoch_lease_is_current(epoch_lease)
                    && !agent_run_should_stop(cancellation)
                {
                    if visible_stream && streamed_output {
                        emit_agent_stream_delta(
                            app,
                            &request_id,
                            session_id,
                            "",
                            false,
                            true,
                            None,
                        );
                    }
                    cancellation.finish_model_call_at(epoch_lease.epoch());
                    return Ok(AgentModelTurnOutcome::RestartAfterSteer);
                }
                if error.is_cancelled()
                    && !cancellation.execution_epoch_lease_is_current(epoch_lease)
                    && !agent_run_should_stop(cancellation)
                {
                    if visible_stream && streamed_output {
                        emit_agent_stream_delta(
                            app,
                            &request_id,
                            session_id,
                            "",
                            false,
                            true,
                            None,
                        );
                    }
                    cancellation.finish_model_call_at(epoch_lease.epoch());
                    return Ok(AgentModelTurnOutcome::RestartAfterSteer);
                }
                if error.is_cancelled() || agent_run_should_stop(cancellation) {
                    emit_agent_stream_delta(app, &request_id, session_id, "", true, true, None);
                    cancellation.finish_model_call_at(epoch_lease.epoch());
                    return Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                        app,
                        state,
                        workspace_root,
                        runtime,
                        prompt,
                        run_context,
                        collaboration,
                        cancellation,
                    )?));
                }
                if transport_attempt < MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS
                    && error.is_retryable()
                    && matches!(
                        cancellation.begin_repair_attempt_at(
                            epoch_lease.epoch(),
                            "provider_transport_retry",
                        ),
                        Ok(Some(_))
                    )
                {
                    if visible_stream && streamed_output {
                        emit_agent_stream_delta(
                            app,
                            &request_id,
                            session_id,
                            "",
                            false,
                            true,
                            None,
                        );
                        streamed_output = false;
                    }
                    cancellation.mark_progress_at(
                        epoch_lease.epoch(),
                        "model_retry",
                        "Transient provider failure",
                    );
                    append_agent_progress_event(
                        state,
                        &runtime.task_id,
                        run_context,
                        &format!(
                            "Retrying model request ({}/{})",
                            transport_attempt + 1,
                            MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS
                        ),
                    )?;
                    std::thread::sleep(model_transport_retry_delay(transport_attempt));
                    continue;
                }
                if !partial_stream.trim().is_empty() {
                    cancellation.record_partial_output_at(epoch_lease.epoch(), &partial_stream);
                }
                cancellation.finish_model_call_at(epoch_lease.epoch());
                cancellation.record_observation_at(
                    epoch_lease.epoch(),
                    "provider_failure",
                    error.class.label(),
                    &error.message,
                );
                if let Some(reason) = exhausted_model_transport_error_stop_reason(&error) {
                    cancellation.request_stop(reason);
                    return Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                        app,
                        state,
                        workspace_root,
                        runtime,
                        prompt,
                        run_context,
                        collaboration,
                        cancellation,
                    )?));
                }
                let failure = AgentFailure::from_model_error(&error);
                return match commit_agent_failure_terminal(
                    state,
                    runtime,
                    run_context,
                    cancellation,
                    epoch_lease,
                    &failure,
                    format!("Agent model call failed: {error}"),
                )? {
                    AgentFailureTerminalOutcome::Committed(agent_state) => {
                        Ok(finished_agent_turn(agent_state))
                    }
                    AgentFailureTerminalOutcome::RestartAfterSteer => {
                        if visible_stream && streamed_output {
                            emit_agent_stream_delta(
                                app,
                                &request_id,
                                session_id,
                                "",
                                false,
                                true,
                                None,
                            );
                        }
                        Ok(AgentModelTurnOutcome::RestartAfterSteer)
                    }
                    AgentFailureTerminalOutcome::Stopped => {
                        Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                            app,
                            state,
                            workspace_root,
                            runtime,
                            prompt,
                            run_context,
                            collaboration,
                            cancellation,
                        )?))
                    }
                };
            }
        }
    };

    cancellation.finish_model_call_at(epoch_lease.epoch());
    if !cancellation.execution_epoch_lease_is_current(epoch_lease)
        && !agent_run_should_stop(cancellation)
    {
        if visible_stream && streamed_output {
            emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
        }
        return Ok(AgentModelTurnOutcome::RestartAfterSteer);
    }
    match cancellation.record_agent_turn_at(epoch_lease.epoch(), "executor") {
        Ok(Some(_)) => {}
        Ok(None) => {
            if visible_stream && streamed_output {
                emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
            }
            return Ok(AgentModelTurnOutcome::RestartAfterSteer);
        }
        Err(_) => {
            if !partial_stream.trim().is_empty() {
                cancellation.record_partial_output_at(epoch_lease.epoch(), &partial_stream);
            }
            return Ok(finished_agent_turn(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                runtime,
                prompt,
                run_context,
                collaboration,
                cancellation,
            )?));
        }
    }

    let raw_response_content = response.message.content.clone();
    response.message.content = sanitize_assistant_content(&raw_response_content);
    let reasoning_markup_removed = response.message.content != raw_response_content.trim();
    if visible_stream && streamed_output && reasoning_markup_removed {
        emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
        streamed_output = false;
    }
    if !response.message.content.trim().is_empty() {
        cancellation.record_partial_output_at(epoch_lease.epoch(), &response.message.content);
    }
    if let Some(evidence) = model_response_checkpoint_evidence(&response) {
        cancellation.record_observation_at(
            epoch_lease.epoch(),
            "model_result",
            "executor",
            &evidence,
        );
    }
    if collaboration.is_some()
        && response.tool_calls.is_empty()
        && !response.message.content.trim().is_empty()
    {
        response
            .message
            .metadata
            .insert("internal".to_string(), "true".to_string());
        response.message.metadata.insert(
            "collaboration_stage".to_string(),
            "executor_draft".to_string(),
        );
    }
    let latency_ms = current_time_millis().saturating_sub(started_at_ms);
    let output_length = response.message.content.len();
    let tool_call_count = response.tool_calls.len();
    if visible_stream && streamed_output && tool_call_count > 0 {
        emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
        streamed_output = false;
    }
    let progress = cancellation.progress();
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        if agent_task_is_cancelled(&mut store, session_id).map_err(|error| error.to_string())? {
            return Ok(finished_agent_turn(
                agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string())?,
            ));
        }
        let mut metadata = Metadata::new();
        metadata.insert("request_id".to_string(), request_id.clone());
        metadata.insert("model".to_string(), agent_model.to_string());
        metadata.insert("latency_ms".to_string(), latency_ms.to_string());
        if let Some(first_delta_at_ms) = first_delta_at_ms {
            metadata.insert(
                "first_token_latency_ms".to_string(),
                first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
            );
        }
        metadata.insert("output_length".to_string(), output_length.to_string());
        metadata.insert("tool_calls".to_string(), tool_call_count.to_string());
        metadata.insert(
            "transport_attempts".to_string(),
            transport_attempt.to_string(),
        );
        metadata.insert(
            "context_projected_tokens".to_string(),
            context_governor.estimated_projected_tokens.to_string(),
        );
        metadata.insert(
            "run_checkpoints".to_string(),
            progress.checkpoints.to_string(),
        );
        metadata.insert(
            "run_observations".to_string(),
            progress.observations.to_string(),
        );
        metadata.insert(
            "run_budget_extensions".to_string(),
            progress.budget_extensions.to_string(),
        );
        metadata.insert(
            "run_model_call_limit".to_string(),
            progress.model_call_limit.to_string(),
        );
        metadata.insert(
            "run_tool_call_limit".to_string(),
            progress.tool_call_limit.to_string(),
        );
        metadata.insert(
            "run_agent_turns".to_string(),
            progress.agent_turns.to_string(),
        );
        metadata.insert(
            "run_agent_turn_limit".to_string(),
            progress.agent_turn_limit.to_string(),
        );
        metadata.insert(
            "run_repair_attempts".to_string(),
            progress.repair_attempts.to_string(),
        );
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
            if let Some(value) = response.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
        if let Some(raw_tool_calls_json) = response.raw_tool_calls_json.clone() {
            metadata.insert("raw_tool_calls_json".to_string(), raw_tool_calls_json);
        }
        if let Some(collaboration) = collaboration {
            metadata.insert("collaboration_id".to_string(), collaboration.id.clone());
            metadata.insert("stage".to_string(), "executor".to_string());
            metadata.insert("role".to_string(), "executor".to_string());
            metadata.insert("policy".to_string(), collaboration.policy.clone());
        }
        append_event(
            &mut store,
            &runtime.task_id,
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            metadata_with_context(metadata, run_context),
        )
        .map_err(|error| error.to_string())?;
    }

    Ok(AgentModelTurnOutcome::Response(AgentModelTurnResponse {
        response,
        request_id,
        streamed_output,
        epoch_lease,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingPreparedProvider {
        prepares: AtomicUsize,
        dispatches: AtomicUsize,
        prepare_failures: AtomicUsize,
    }

    impl StreamingModelProvider for CountingPreparedProvider {
        fn prepare_streaming_request(
            &self,
            request: &ModelRequest,
        ) -> Result<PreparedStreamingModelRequest, ModelError> {
            self.prepares.fetch_add(1, Ordering::Relaxed);
            if self
                .prepare_failures
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(ModelError::new("service temporarily unavailable"));
            }
            Ok(PreparedStreamingModelRequest::deferred(request.clone()))
        }

        fn complete_streaming_cancellable(
            &self,
            request: ModelRequest,
            _on_delta: &mut dyn FnMut(&str),
            _should_cancel: &mut dyn FnMut() -> bool,
        ) -> Result<ModelResponse, ModelError> {
            self.dispatches.fetch_add(1, Ordering::Relaxed);
            Ok(ModelResponse {
                message: Message {
                    role: MessageRole::Assistant,
                    content: "ok".to_string(),
                    metadata: Metadata::new(),
                },
                raw_tool_calls_json: None,
                tool_calls: Vec::new(),
                metadata: request.metadata,
            })
        }
    }

    #[test]
    fn transport_attempts_prepare_model_request_once() {
        let provider = CountingPreparedProvider {
            prepares: AtomicUsize::new(0),
            dispatches: AtomicUsize::new(0),
            prepare_failures: AtomicUsize::new(0),
        };
        let request = ModelRequest {
            role: ModelRole::Executor,
            messages: vec![Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: Metadata::new(),
        };
        let mut prepared = None;

        for _ in 0..MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS {
            let prepared_request =
                prepared_streaming_request_once(&provider, &request, &mut prepared)
                    .expect("request should prepare");
            provider
                .complete_prepared_streaming_cancellable(prepared_request, &mut |_| {}, &mut || {
                    false
                })
                .expect("prepared request should dispatch");
        }

        assert_eq!(provider.prepares.load(Ordering::Relaxed), 1);
        assert_eq!(
            provider.dispatches.load(Ordering::Relaxed),
            MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS
        );
        println!(
            "{{\"schema\":\"cindx.model-transport-prepare-scaling.v1\",\"prepares\":{},\"dispatches\":{}}}",
            provider.prepares.load(Ordering::Relaxed),
            provider.dispatches.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn retryable_prepare_error_is_not_cached() {
        let provider = CountingPreparedProvider {
            prepares: AtomicUsize::new(0),
            dispatches: AtomicUsize::new(0),
            prepare_failures: AtomicUsize::new(1),
        };
        let request = ModelRequest {
            role: ModelRole::Executor,
            messages: Vec::new(),
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: Metadata::new(),
        };
        let mut prepared = None;

        let error = prepared_streaming_request_once(&provider, &request, &mut prepared)
            .expect_err("first preparation should fail transiently");
        assert!(error.is_retryable());
        assert!(prepared.is_none());

        prepared_streaming_request_once(&provider, &request, &mut prepared)
            .expect("second preparation should retry and succeed");
        prepared_streaming_request_once(&provider, &request, &mut prepared)
            .expect("successful preparation should be reused");
        assert_eq!(provider.prepares.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn finalizer_stage_exhaustion_becomes_a_recoverable_run_stop() {
        let control = AgentRunControl::new("fast");
        let finalizer_calls = control.budget().finalizer_model_call_reserve();
        for _ in 0..finalizer_calls {
            control
                .begin_stage_model_call("executor", RunStageClass::Finalizer)
                .expect("reserved finalizer call should start");
            control.finish_model_call();
        }
        let reason = control
            .begin_stage_model_call("executor", RunStageClass::Finalizer)
            .expect_err("finalizer stage should be bounded");

        stop_parent_run_for_model_call_failure(&control, reason);

        assert_eq!(
            control.stop_reason(),
            Some(RunStopReason::StageBudgetExhausted)
        );
    }
}
