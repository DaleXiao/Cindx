use crate::desktop_prelude::*;
use crate::{
    agent_completion_runtime::{
        finalize_agent_completion, AgentCompletionDelivery, AgentCompletionOutcome,
    },
    agent_failure_terminal_runtime::{resolve_loop_failure, AgentFailureLoopOutcome},
    agent_finalizer_runtime::terminal_runtime::{
        execute_terminal_finalizer, TerminalFinalizerContext, TerminalFinalizerOutcome,
    },
    agent_grounded_response_runtime::{
        advance_grounded_model_response, should_reset_rejected_completion_stream,
        GroundedModelResponse,
    },
    agent_model_turn_runtime::{
        execute_agent_model_turn, AgentModelTurnOutcome, AgentModelTurnPlan,
        AgentModelTurnResponse,
    },
    agent_query_commands::{
        append_agent_progress_event, emit_agent_stream_delta,
        finish_agent_run_for_control_stop_with_task_state,
    },
    agent_read_model::agent_state_for_session,
    agent_runtime_snapshot::{
        capture_persistable_agent_task_state, persist_runtime_append_and_snapshot,
    },
    agent_runtime_snapshot_cursor::AgentRuntimeSnapshotCursor,
    agent_steer_runtime::{apply_pending_agent_steers_with_cursor, AgentSteerApplication},
    agent_tool_runtime::{execute_agent_tool_batch, AgentToolBatchOutcome},
    app_state::AppState,
    configuration_models::ProviderConfig,
    persistence_runtime::tool_registry_for_state,
    runtime_constants::AGENT_MAX_OUTPUT_TOKENS,
    runtime_values::{
        agent_runtime_context_for_run, current_time_millis, effective_agent_objective,
        run_context_steer_epoch,
    },
    suspended_run_runtime::{remember_suspended_agent_run, SuspendedAgentRun},
    view_models::AgentState,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn pause_agent_loop_for_control_stop(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    if cancellation
        .stop_reason()
        .is_some_and(|reason| !reason.is_user_cancelled())
    {
        remember_suspended_agent_run(
            state,
            SuspendedAgentRun {
                runtime: runtime.clone(),
                prompt: prompt.to_string(),
                run_context: run_context.clone(),
                workspace_root: workspace_root.to_path_buf(),
                collaboration: collaboration.cloned(),
                run_control: cancellation.snapshot(),
                last_touched_at_ms: current_time_millis(),
            },
        )?;
    }
    let task_state = capture_persistable_agent_task_state(runtime);
    finish_agent_run_for_control_stop_with_task_state(
        app,
        state,
        run_context,
        cancellation,
        Some(&task_state),
    )
}

#[path = "agent_loop_contract_runtime.rs"]
mod contract_runtime;
#[cfg(test)]
pub(crate) use contract_runtime::{
    apply_run_task_contract, workspace_verification_policy_for_run_context,
};
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use contract_runtime::{
    apply_run_task_contract_with_completion_intent, planned_agent_tools,
};
use contract_runtime::{
    record_retained_agent_decision_after_noop_steer, synchronize_noop_control_epoch_context,
};
#[allow(clippy::large_enum_variant)]
pub(crate) enum AgentLoopExecutionOutcome {
    Finished(AgentState),
    Reprepare {
        runtime: agent_runtime::AgentLoopState,
        prompt: String,
    },
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_agent_loop_epoch_with_provider(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    mut runtime: agent_runtime::AgentLoopState,
    prompt: String,
    mut run_context: Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    actor_provider: &dyn StreamingModelProvider,
    finalizer_provider: &dyn StreamingModelProvider,
    agent_model: &str,
) -> Result<AgentLoopExecutionOutcome, String> {
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    let registry = tool_registry_for_state(state, workspace_root)?;
    let (tools, completion_intent) = planned_agent_tools(
        &registry,
        &run_context,
        effective_agent_objective(&run_context, &prompt),
        config.context_window_tokens,
    );
    apply_run_task_contract_with_completion_intent(
        &mut runtime,
        &run_context,
        &tools,
        collaboration,
        &completion_intent,
    )?;
    let mut snapshot_cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &run_context);
    let mut runtime_context = agent_runtime_context_for_run(&run_context);
    let mut active_collaboration = collaboration;
    let mut actor_requested_finalizer = false;

    'agent_loop: loop {
        match apply_pending_agent_steers_with_cursor(
            state,
            workspace_root,
            &mut runtime,
            &run_context,
            cancellation,
            &mut snapshot_cursor,
        )? {
            AgentSteerApplication::Applied(steer) => {
                run_context.insert("steer_epoch".to_string(), steer.epoch.to_string());
                cancellation.mark_progress_at(steer.epoch, "steering", "User guidance applied");
                append_agent_progress_event(
                    state,
                    &runtime.task_id,
                    &run_context,
                    "Applying user steering",
                )?;
                return Ok(AgentLoopExecutionOutcome::Reprepare {
                    runtime,
                    prompt: steer.prompt,
                });
            }
            AgentSteerApplication::ResolvedNoop { epoch } => {
                runtime_context = synchronize_noop_control_epoch_context(&mut run_context, epoch);
                runtime.advance_prepared_task_control_epoch(epoch);
                record_retained_agent_decision_after_noop_steer(
                    state,
                    &runtime.task_id,
                    &run_context,
                )?;
            }
            AgentSteerApplication::Stopped(_) => {
                return pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    &runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                )
                .map(AgentLoopExecutionOutcome::Finished)
            }
            AgentSteerApplication::NoPending => {}
        }
        let max_output_tokens =
            bounded_max_output_tokens(config.context_window_tokens, AGENT_MAX_OUTPUT_TOKENS);
        let terminal_commit = actor_requested_finalizer
            || matches!(
                cancellation.continuation_directive(),
                RunContinuationDirective::CommitTerminalResult
            );
        if terminal_commit {
            actor_requested_finalizer = false;
            match execute_terminal_finalizer(
                &mut runtime,
                TerminalFinalizerContext {
                    app,
                    state,
                    config,
                    workspace_root,
                    prompt: &prompt,
                    run_context: &run_context,
                    collaboration: active_collaboration,
                    cancellation,
                    provider: finalizer_provider,
                    agent_model,
                    runtime_context: runtime_context.as_deref(),
                    max_output_tokens,
                    snapshot_cursor: &mut snapshot_cursor,
                },
            )? {
                TerminalFinalizerOutcome::Finished(agent_state) => {
                    return Ok(AgentLoopExecutionOutcome::Finished(agent_state));
                }
                TerminalFinalizerOutcome::RestartAfterSteer => {
                    active_collaboration = None;
                    continue 'agent_loop;
                }
                TerminalFinalizerOutcome::Pause => {
                    return pause_agent_loop_for_control_stop(
                        app,
                        state,
                        workspace_root,
                        &runtime,
                        &prompt,
                        &run_context,
                        active_collaboration,
                        cancellation,
                    )
                    .map(AgentLoopExecutionOutcome::Finished);
                }
            }
        }
        let prepared_turn = match AgentKernel::new(&mut runtime, &tools).prepare_model_turn(
            Some(&config.agent_system_prompt),
            runtime_context.as_deref(),
            config.context_window_tokens,
            max_output_tokens,
        ) {
            Ok(prepared_turn) => prepared_turn,
            Err(AgentTurnPreparationError::Budget(exhausted)) => {
                if let Some(partial_answer) = exhausted.partial_answer {
                    if let agent_runtime::RunEpochLeaseOutcome::Acquired(lease) =
                        cancellation.execution_epoch_lease()
                    {
                        cancellation.record_partial_output_at(lease.epoch(), &partial_answer);
                    }
                }
                cancellation.request_stop(RunStopReason::TurnBudgetExhausted);
                return pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    &runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                )
                .map(AgentLoopExecutionOutcome::Finished);
            }
            Err(AgentTurnPreparationError::Context(violation)) => {
                return Err(violation.to_string());
            }
        };
        let visible_contract_evidence_sequences = prepared_turn.visible_contract_evidence_sequences;
        let mut request = prepared_turn.request;
        let context_governor = prepared_turn.context;
        request.metadata.insert(
            "max_output_tokens".to_string(),
            max_output_tokens.to_string(),
        );
        let model_turn = execute_agent_model_turn(
            app,
            state,
            workspace_root,
            &runtime,
            &prompt,
            &run_context,
            active_collaboration,
            cancellation,
            actor_provider,
            agent_model,
            &context_governor,
            AgentModelTurnPlan::actor(&tools, request),
        )?;
        let AgentModelTurnResponse {
            response,
            request_id,
            streamed_output,
            epoch_lease,
        } = match model_turn {
            AgentModelTurnOutcome::Response(response) => response,
            AgentModelTurnOutcome::Unavailable(_) => {
                return Err("actor model turn unexpectedly entered finalizer fallback".to_string())
            }
            AgentModelTurnOutcome::HandoffToFinalizer => {
                actor_requested_finalizer = true;
                continue 'agent_loop;
            }
            AgentModelTurnOutcome::RestartAfterSteer => {
                active_collaboration = None;
                continue 'agent_loop;
            }
            AgentModelTurnOutcome::Finished(agent_state) => {
                return Ok(AgentLoopExecutionOutcome::Finished(*agent_state))
            }
        };
        let visible_stream = active_collaboration.is_none();
        let previous_message_count = runtime.messages.len();
        let response_commit = cancellation.commit_execution_step_with(epoch_lease, || {
            let mut transaction = AgentLoopAppendTransaction::begin(&mut runtime);
            let grounded = transaction.with_append_only_mutation(|next_runtime| {
                advance_grounded_model_response(
                    next_runtime,
                    &tools,
                    response,
                    previous_message_count,
                    run_context_steer_epoch(&run_context),
                    &visible_contract_evidence_sequences,
                )
            });
            let (prepared_snapshot, next_cursor) = snapshot_cursor.prepare_after_append(
                transaction.state(),
                previous_message_count,
                &run_context,
            );
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            persist_runtime_append_and_snapshot(
                &mut store,
                transaction.state(),
                previous_message_count,
                &run_context,
                &prepared_snapshot,
            )?;
            drop(store);
            transaction.commit();
            snapshot_cursor = next_cursor;
            Ok::<_, String>(grounded)
        })?;
        let GroundedModelResponse {
            advance,
            verification_required,
            receipt: grounded_completion_receipt,
            rejection: completion_rejection,
        } = match response_commit {
            agent_runtime::RunExecutionStepCommit::Committed(committed) => committed,
            agent_runtime::RunExecutionStepCommit::RestartAfterSteer => {
                runtime.messages.truncate(previous_message_count);
                if visible_stream && streamed_output {
                    emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
                }
                active_collaboration = None;
                continue 'agent_loop;
            }
            agent_runtime::RunExecutionStepCommit::Stopped(_) => {
                runtime.messages.truncate(previous_message_count);
                return pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    &runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                )
                .map(AgentLoopExecutionOutcome::Finished);
            }
            agent_runtime::RunExecutionStepCommit::TerminalCommitted => {
                let store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                return agent_state_for_session(&store, None, session_id)
                    .map(AgentLoopExecutionOutcome::Finished)
                    .map_err(|error| error.to_string());
            }
        };
        if should_reset_rejected_completion_stream(
            visible_stream,
            streamed_output,
            completion_rejection,
        ) {
            emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
        }
        if verification_required {
            cancellation.mark_progress_at(
                run_context_steer_epoch(&run_context),
                "verification",
                "Waiting for task-contract evidence",
            );
            continue;
        }

        match advance {
            AgentAdvance::Completed { answer } => {
                let grounded_completion_receipt = grounded_completion_receipt.ok_or_else(|| {
                    "completed agent response is missing a grounded completion receipt".to_string()
                })?;
                match finalize_agent_completion(
                    app,
                    state,
                    config,
                    workspace_root,
                    &runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                    &request_id,
                    session_id,
                    streamed_output,
                    AgentCompletionDelivery::Actor,
                    answer,
                    grounded_completion_receipt,
                    epoch_lease,
                )? {
                    AgentCompletionOutcome::Completed(agent_state) => {
                        return Ok(AgentLoopExecutionOutcome::Finished(agent_state))
                    }
                    AgentCompletionOutcome::RestartAfterSteer => {
                        active_collaboration = None;
                        continue 'agent_loop;
                    }
                    AgentCompletionOutcome::Paused(agent_state) => {
                        return Ok(AgentLoopExecutionOutcome::Finished(agent_state))
                    }
                }
            }
            AgentAdvance::TurnBudgetExhausted(exhausted) => {
                if let Some(partial_answer) = exhausted.partial_answer {
                    cancellation.record_partial_output_at(epoch_lease.epoch(), &partial_answer);
                }
                cancellation.request_stop(RunStopReason::TurnBudgetExhausted);
                return pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    &runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                )
                .map(AgentLoopExecutionOutcome::Finished);
            }
            AgentAdvance::Retry { .. } => {
                if visible_stream && streamed_output {
                    emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
                }
                cancellation.mark_progress_at(
                    run_context_steer_epoch(&run_context),
                    "model_retry",
                    "Recovering incomplete model response",
                );
                continue;
            }
            AgentAdvance::Failed { failure } => {
                match resolve_loop_failure(
                    app,
                    state,
                    workspace_root,
                    &runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                    epoch_lease,
                    &failure,
                    &request_id,
                    session_id,
                    streamed_output && completion_rejection.is_none(),
                )? {
                    AgentFailureLoopOutcome::Finished(agent_state) => {
                        return Ok(AgentLoopExecutionOutcome::Finished(agent_state))
                    }
                    AgentFailureLoopOutcome::RestartAfterSteer => {
                        active_collaboration = None;
                        continue 'agent_loop;
                    }
                }
            }
            AgentAdvance::ToolCalls { calls } => {
                match execute_agent_tool_batch(
                    app,
                    state,
                    workspace_root,
                    &mut runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                    epoch_lease,
                    &registry,
                    &tools,
                    calls,
                    &mut snapshot_cursor,
                )? {
                    AgentToolBatchOutcome::Continue => {}
                    AgentToolBatchOutcome::RestartAfterSteer => {
                        active_collaboration = None;
                        continue 'agent_loop;
                    }
                    AgentToolBatchOutcome::Paused(agent_state) => {
                        return Ok(AgentLoopExecutionOutcome::Finished(*agent_state))
                    }
                }
            }
        }
    }
}
