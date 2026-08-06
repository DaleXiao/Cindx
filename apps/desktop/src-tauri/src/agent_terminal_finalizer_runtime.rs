use crate::{
    agent_completion_runtime::{
        finalize_agent_completion, AgentCompletionDelivery, AgentCompletionOutcome,
    },
    agent_failure_terminal_runtime::{resolve_loop_failure, AgentFailureLoopOutcome},
    agent_finalizer_runtime::{
        grounded_finalizer_fallback, prepare_finalizer_turn, resolve_finalizer_fallback,
        resolve_finalizer_response,
    },
    agent_model_turn_runtime::{
        execute_agent_model_turn, AgentModelTurnOutcome, AgentModelTurnPlan,
        AgentModelTurnResponse,
    },
    agent_query_commands::emit_agent_stream_delta,
    agent_read_model::agent_state_for_session,
    agent_runtime_snapshot::persist_runtime_append_and_snapshot,
    agent_runtime_snapshot_cursor::AgentRuntimeSnapshotCursor,
    app_state::AppState,
    collaboration_service::AgentCollaboration,
    configuration_models::ProviderConfig,
    runtime_values::run_context_steer_epoch,
    view_models::AgentState,
};
use agent_core::Metadata;
use agent_runtime::{
    ensure_terminal_commit_instruction, AgentFailure, AgentLoopAppendTransaction, AgentRunControl,
    AgentTurnPreparationError, RunEpochLease, RunStopReason,
};
use model_provider::StreamingModelProvider;
use std::{path::Path, sync::Arc};

pub(crate) struct TerminalFinalizerContext<'a, 'state> {
    pub(crate) app: &'a tauri::AppHandle,
    pub(crate) state: &'a tauri::State<'state, AppState>,
    pub(crate) config: &'a ProviderConfig,
    pub(crate) workspace_root: &'a Path,
    pub(crate) prompt: &'a str,
    pub(crate) run_context: &'a Metadata,
    pub(crate) collaboration: Option<&'a AgentCollaboration>,
    pub(crate) cancellation: &'a Arc<AgentRunControl>,
    pub(crate) provider: &'a dyn StreamingModelProvider,
    pub(crate) agent_model: &'a str,
    pub(crate) runtime_context: Option<&'a str>,
    pub(crate) max_output_tokens: u64,
    pub(crate) snapshot_cursor: &'a mut AgentRuntimeSnapshotCursor,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum TerminalFinalizerOutcome {
    Finished(AgentState),
    RestartAfterSteer,
    Pause,
}

#[allow(clippy::too_many_arguments)]
fn resolve_internal_finalizer_failure(
    runtime: &agent_runtime::AgentLoopState,
    context: &TerminalFinalizerContext<'_, '_>,
    epoch_lease: RunEpochLease,
    code: &'static str,
    message: String,
    request_id: &str,
    reset_stream: bool,
) -> Result<TerminalFinalizerOutcome, String> {
    let session_id = context.run_context.get("session_id").map(String::as_str);
    if reset_stream {
        emit_agent_stream_delta(
            context.app,
            request_id,
            session_id,
            "",
            false,
            true,
            None,
        );
    }
    let failure = AgentFailure::internal(code, message);
    match resolve_loop_failure(
        context.app,
        context.state,
        context.workspace_root,
        runtime,
        context.prompt,
        context.run_context,
        context.collaboration,
        context.cancellation,
        epoch_lease,
        &failure,
        request_id,
        session_id,
        false,
    )? {
        AgentFailureLoopOutcome::Finished(agent_state) => {
            Ok(TerminalFinalizerOutcome::Finished(agent_state))
        }
        AgentFailureLoopOutcome::RestartAfterSteer => {
            Ok(TerminalFinalizerOutcome::RestartAfterSteer)
        }
    }
}

pub(crate) fn execute_terminal_finalizer(
    runtime: &mut agent_runtime::AgentLoopState,
    context: TerminalFinalizerContext<'_, '_>,
) -> Result<TerminalFinalizerOutcome, String> {
    let epoch_lease = match context.cancellation.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        agent_runtime::RunEpochLeaseOutcome::RestartAfterSteer => {
            return Ok(TerminalFinalizerOutcome::RestartAfterSteer);
        }
        agent_runtime::RunEpochLeaseOutcome::Stopped(_) => {
            return Ok(TerminalFinalizerOutcome::Pause);
        }
        agent_runtime::RunEpochLeaseOutcome::TerminalCommitted => {
            let store = context
                .state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            let agent_state = agent_state_for_session(
                &store,
                None,
                context.run_context.get("session_id").map(String::as_str),
            )
            .map_err(|error| error.to_string())?;
            return Ok(TerminalFinalizerOutcome::Finished(agent_state));
        }
    };
    let instruction_commit =
        context
            .cancellation
            .commit_execution_step_with(epoch_lease, || {
                let mut transaction = AgentLoopAppendTransaction::begin(runtime);
                let previous_message_count = transaction.original_message_count();
                let instruction_added =
                    transaction.with_append_only_mutation(ensure_terminal_commit_instruction);
                if instruction_added {
                    let (prepared_snapshot, next_cursor) =
                        context.snapshot_cursor.prepare_after_append(
                            transaction.state(),
                            previous_message_count,
                            context.run_context,
                        );
                    let mut store = context
                        .state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    persist_runtime_append_and_snapshot(
                        &mut store,
                        transaction.state(),
                        previous_message_count,
                        context.run_context,
                        &prepared_snapshot,
                    )?;
                    drop(store);
                    transaction.commit();
                    *context.snapshot_cursor = next_cursor;
                } else {
                    transaction.commit();
                }
                Ok::<_, String>(())
            })?;
    match instruction_commit {
        agent_runtime::RunExecutionStepCommit::Committed(()) => {}
        agent_runtime::RunExecutionStepCommit::RestartAfterSteer => {
            return Ok(TerminalFinalizerOutcome::RestartAfterSteer);
        }
        agent_runtime::RunExecutionStepCommit::Stopped(_) => {
            return Ok(TerminalFinalizerOutcome::Pause);
        }
        agent_runtime::RunExecutionStepCommit::TerminalCommitted => {
            let store = context
                .state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            let agent_state = agent_state_for_session(
                &store,
                None,
                context.run_context.get("session_id").map(String::as_str),
            )
            .map_err(|error| error.to_string())?;
            return Ok(TerminalFinalizerOutcome::Finished(agent_state));
        }
    }

    let prepared_turn = match prepare_finalizer_turn(
        runtime,
        Some(&context.config.agent_system_prompt),
        context.runtime_context,
        context.config.context_window_tokens,
        context.max_output_tokens,
    ) {
        Ok(prepared_turn) => prepared_turn,
        Err(AgentTurnPreparationError::Budget(exhausted)) => {
            if let Some(partial_answer) = exhausted.partial_answer {
                if let agent_runtime::RunEpochLeaseOutcome::Acquired(lease) =
                    context.cancellation.execution_epoch_lease()
                {
                    context
                        .cancellation
                        .record_partial_output_at(lease.epoch(), &partial_answer);
                }
            }
            context
                .cancellation
                .request_stop(RunStopReason::TurnBudgetExhausted);
            return Ok(TerminalFinalizerOutcome::Pause);
        }
        Err(AgentTurnPreparationError::Context(violation)) => {
            return resolve_internal_finalizer_failure(
                runtime,
                &context,
                epoch_lease,
                "finalizer_context_invalid",
                violation.to_string(),
                "agent-finalizer-context",
                false,
            );
        }
    };
    let visible_contract_evidence_sequences = prepared_turn.visible_contract_evidence_sequences;
    let mut request = prepared_turn.request;
    let context_governor = prepared_turn.context;
    request.metadata.insert(
        "max_output_tokens".to_string(),
        context.max_output_tokens.to_string(),
    );
    let fallback = grounded_finalizer_fallback(
        runtime,
        context.cancellation,
        run_context_steer_epoch(context.run_context),
        &visible_contract_evidence_sequences,
    );
    let model_turn = execute_agent_model_turn(
        context.app,
        context.state,
        context.workspace_root,
        runtime,
        context.prompt,
        context.run_context,
        context.collaboration,
        context.cancellation,
        context.provider,
        context.agent_model,
        &context_governor,
        AgentModelTurnPlan::finalizer(request),
    )?;
    let (resolution, request_id, streamed_output, epoch_lease) = match model_turn {
        AgentModelTurnOutcome::Response(AgentModelTurnResponse {
            response,
            request_id,
            streamed_output,
            epoch_lease,
        }) => (
            resolve_finalizer_response(
                runtime,
                response,
                fallback,
                epoch_lease.epoch(),
                &visible_contract_evidence_sequences,
            ),
            request_id,
            streamed_output,
            epoch_lease,
        ),
        AgentModelTurnOutcome::Unavailable(unavailable) => {
            let resolution = match fallback {
                Some(fallback) => resolve_finalizer_fallback(
                    runtime,
                    Some(fallback),
                    unavailable.epoch_lease.epoch(),
                    &visible_contract_evidence_sequences,
                ),
                None => Err(unavailable.failure),
            };
            (
                resolution,
                unavailable.request_id,
                unavailable.streamed_output,
                unavailable.epoch_lease,
            )
        }
        AgentModelTurnOutcome::HandoffToFinalizer => {
            return resolve_internal_finalizer_failure(
                runtime,
                &context,
                epoch_lease,
                "finalizer_role_violation",
                "the Finalizer requested an Actor-to-Finalizer handoff".to_string(),
                "agent-finalizer-role",
                false,
            );
        }
        AgentModelTurnOutcome::RestartAfterSteer => {
            return Ok(TerminalFinalizerOutcome::RestartAfterSteer);
        }
        AgentModelTurnOutcome::Finished(agent_state) => {
            return Ok(TerminalFinalizerOutcome::Finished(*agent_state));
        }
    };
    let session_id = context.run_context.get("session_id").map(String::as_str);
    match resolution {
        Ok(resolution) => {
            if resolution.used_fallback && streamed_output {
                emit_agent_stream_delta(
                    context.app,
                    &request_id,
                    session_id,
                    "",
                    false,
                    true,
                    None,
                );
            }
            let streamed_output = streamed_output && !resolution.used_fallback;
            let candidate = resolution.candidate;
            if resolution.used_fallback {
                context
                    .cancellation
                    .record_partial_output_at(epoch_lease.epoch(), &candidate.content);
            }
            let completion = finalize_agent_completion(
                context.app,
                context.state,
                context.config,
                context.workspace_root,
                runtime,
                context.prompt,
                context.run_context,
                context.collaboration,
                context.cancellation,
                &request_id,
                session_id,
                streamed_output,
                AgentCompletionDelivery::Finalizer {
                    already_persisted: candidate.already_persisted,
                    used_fallback: resolution.used_fallback,
                },
                candidate.content,
                candidate.receipt,
                epoch_lease,
            );
            let completion = match completion {
                Ok(completion) => completion,
                Err(error) => {
                    return resolve_internal_finalizer_failure(
                        runtime,
                        &context,
                        epoch_lease,
                        "finalizer_terminal_commit_failed",
                        error,
                        &request_id,
                        true,
                    );
                }
            };
            match completion {
                AgentCompletionOutcome::Completed(agent_state)
                | AgentCompletionOutcome::Paused(agent_state) => {
                    Ok(TerminalFinalizerOutcome::Finished(agent_state))
                }
                AgentCompletionOutcome::RestartAfterSteer => {
                    Ok(TerminalFinalizerOutcome::RestartAfterSteer)
                }
            }
        }
        Err(failure) => {
            if streamed_output {
                emit_agent_stream_delta(
                    context.app,
                    &request_id,
                    session_id,
                    "",
                    false,
                    true,
                    None,
                );
            }
            match resolve_loop_failure(
                context.app,
                context.state,
                context.workspace_root,
                runtime,
                context.prompt,
                context.run_context,
                context.collaboration,
                context.cancellation,
                epoch_lease,
                &failure,
                &request_id,
                session_id,
                false,
            )? {
                AgentFailureLoopOutcome::Finished(agent_state) => {
                    Ok(TerminalFinalizerOutcome::Finished(agent_state))
                }
                AgentFailureLoopOutcome::RestartAfterSteer => {
                    Ok(TerminalFinalizerOutcome::RestartAfterSteer)
                }
            }
        }
    }
}
