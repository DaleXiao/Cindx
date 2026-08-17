use crate::{
    agent_completion_runtime::{
        finalize_agent_completion, AgentCompletionDelivery, AgentCompletionOutcome,
    },
    agent_failure_terminal_runtime::{resolve_loop_failure, AgentFailureLoopOutcome},
    agent_finalizer_runtime::direct_finalizer_policy::{
        insert_direct_finalizer_receipt_metadata, selected_direct_finalizer_policy,
    },
    agent_finalizer_runtime::{
        grounded_finalizer_fallback, prepare_finalizer_turn, resolve_finalizer_fallback,
        resolve_finalizer_response, FinalizerResolution,
    },
    agent_model_turn_runtime::{
        execute_agent_model_turn, AgentModelTurnOutcome, AgentModelTurnPlan, AgentModelTurnResponse,
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

fn finalizer_attempt_is_retryable(
    resolution: &Result<FinalizerResolution, AgentFailure>,
    fallback_available: bool,
    retries: u8,
) -> bool {
    const MAX_FINALIZER_RETRIES: u8 = 1;
    retries < MAX_FINALIZER_RETRIES
        && !fallback_available
        && matches!(resolution, Err(failure) if failure.code == "finalizer_no_grounded_candidate")
}

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
        emit_agent_stream_delta(context.app, request_id, session_id, "", false, true, None);
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
    let direct_finalizer_policy = selected_direct_finalizer_policy(context.run_context);
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

    // A direct run's delivery gate is one toolless finalizer call. Empty or
    // unusable finalizer output is usually a provider flake, so when no verified
    // fallback exists the gate gets one bounded retry before the run fails.
    let mut finalizer_retries = 0u8;
    let (resolution, request_id, streamed_output, epoch_lease) = loop {
        let prepared_turn = match prepare_finalizer_turn(
            runtime,
            Some(&context.config.agent_system_prompt),
            direct_finalizer_policy
                .as_ref()
                .and_then(|selection| selection.phenotype.directive()),
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
        if let Some(selection) = &direct_finalizer_policy {
            insert_direct_finalizer_receipt_metadata(&mut request.metadata, selection);
        }
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
        let attempt = match model_turn {
            AgentModelTurnOutcome::Response(AgentModelTurnResponse {
                response,
                request_id,
                streamed_output,
                epoch_lease,
            }) => (
                resolve_finalizer_response(
                    runtime,
                    response,
                    fallback.clone(),
                    epoch_lease.epoch(),
                    &visible_contract_evidence_sequences,
                ),
                request_id,
                streamed_output,
                epoch_lease,
            ),
            AgentModelTurnOutcome::Unavailable(unavailable) => {
                let resolution = match fallback.clone() {
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
        if !finalizer_attempt_is_retryable(&attempt.0, fallback.is_some(), finalizer_retries) {
            break attempt;
        }
        finalizer_retries += 1;
        if attempt.2 {
            emit_agent_stream_delta(
                context.app,
                &attempt.1,
                context.run_context.get("session_id").map(String::as_str),
                "",
                false,
                true,
                None,
            );
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
            let mut completion_run_context = context.run_context.clone();
            if !resolution.used_fallback {
                if let Some(selection) = &direct_finalizer_policy {
                    insert_direct_finalizer_receipt_metadata(
                        &mut completion_run_context,
                        selection,
                    );
                    completion_run_context.insert(
                        "direct_finalizer_profile_exercised".to_string(),
                        "true".to_string(),
                    );
                    completion_run_context.insert(
                        "direct_finalizer_delivery_request_id".to_string(),
                        request_id.clone(),
                    );
                }
            }
            let completion = finalize_agent_completion(
                context.app,
                context.state,
                context.config,
                context.workspace_root,
                runtime,
                context.prompt,
                &completion_run_context,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalizer_retry_is_bounded_and_only_for_unbacked_no_candidate_failures() {
        let no_candidate = Err(AgentFailure::model_output(
            "finalizer_no_grounded_candidate",
            "empty finalizer output",
        ));
        assert!(finalizer_attempt_is_retryable(&no_candidate, false, 0));
        assert!(!finalizer_attempt_is_retryable(&no_candidate, true, 0));
        assert!(!finalizer_attempt_is_retryable(&no_candidate, false, 1));
        let other = Err(AgentFailure::model_output("finalizer_lineage", "x"));
        assert!(!finalizer_attempt_is_retryable(&other, false, 0));
    }
}
