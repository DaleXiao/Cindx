use crate::desktop_prelude::*;
use crate::{
    agent_completion_runtime::{finalize_agent_completion, AgentCompletionOutcome},
    agent_model_turn_runtime::{
        execute_agent_model_turn, AgentModelTurnOutcome, AgentModelTurnResponse,
    },
    agent_query_commands::{
        append_agent_progress_event, emit_agent_stream_delta,
        finish_agent_run_for_control_stop_with_task_state,
    },
    agent_read_model::{agent_state_for_session, agent_state_with_error_in_context},
    agent_run_engine::{prepare_agent_execution, AgentRunPreparationError, PreparedAgentExecution},
    agent_runtime_snapshot::{
        capture_persistable_agent_task_state, persist_agent_runtime_snapshot,
    },
    agent_steer_runtime::{apply_pending_agent_steers, AgentSteerApplication},
    agent_tool_runtime::{execute_agent_tool_batch, AgentToolBatchOutcome},
    app_state::{AppState, SuspendedAgentRun},
    configuration_models::{agent_model_for_run, AgentEffort, ProviderConfig},
    event_persistence::persist_new_runtime_messages,
    persistence_runtime::tool_registry_for_state,
    runtime_constants::{AGENT_MAX_OUTPUT_TOKENS, AGENT_MODEL_RECOVERY_WINDOW_SECONDS},
    runtime_values::{
        agent_runtime_context_for_run, current_time_millis, effective_agent_objective,
        run_context_steer_epoch,
    },
    suspended_run_runtime::{clear_suspended_agent_run_for_context, remember_suspended_agent_run},
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
pub(crate) use contract_runtime::apply_run_task_contract;
use contract_runtime::{
    record_retained_agent_decision_after_noop_steer, synchronize_noop_control_epoch_context,
};

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn workspace_verification_policy_for_run_context(
    run_context: &Metadata,
) -> Result<WorkspaceVerificationPolicy, String> {
    contract_runtime::workspace_verification_policy_for_run_context(run_context)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn continue_agent_loop(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    prepared: PreparedAgentExecution,
    effort: AgentEffort,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let base_run_context = prepared.base_run_context;
    let mut run_context = prepared.run_context;
    let mut runtime = prepared.runtime;
    let mut prompt = prepared.prompt;
    let mut collaboration = prepared.collaboration;
    loop {
        let agent_model = agent_model_for_run(config, &run_context);
        let provider_timeout = if collaboration.is_some() {
            cancellation
                .stage_model_call_timeout_with_recovery(
                    RunStageClass::Finalizer,
                    1,
                    Duration::from_secs(AGENT_MODEL_RECOVERY_WINDOW_SECONDS),
                )
                .as_secs()
                .max(1)
        } else {
            cancellation.model_call_timeout_seconds()
        };
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            model: agent_model.clone(),
            embedding_model: config.model_for_role(&ModelRole::Embedder),
            timeout_seconds: provider_timeout,
        });
        match continue_agent_loop_with_provider(
            app,
            state,
            config,
            workspace_root,
            runtime,
            prompt,
            run_context,
            collaboration.as_ref(),
            cancellation,
            &provider,
            &agent_model,
        )? {
            AgentLoopExecutionOutcome::Finished(agent_state) => return Ok(agent_state),
            AgentLoopExecutionOutcome::Reprepare {
                runtime: steered_runtime,
                prompt: steered_prompt,
            } => {
                let task_id = steered_runtime.task_id.clone();
                let next = prepare_agent_execution(
                    app,
                    state,
                    config,
                    &task_id,
                    workspace_root,
                    base_run_context.clone(),
                    steered_runtime,
                    steered_prompt,
                    None,
                    effort,
                    cancellation,
                );
                let next = match next {
                    Ok(prepared) => prepared,
                    Err(AgentRunPreparationError::ControlStop(run_context)) => {
                        return finish_agent_run_for_control_stop_with_task_state(
                            app,
                            state,
                            &run_context,
                            cancellation,
                            None,
                        )
                    }
                    Err(AgentRunPreparationError::Collaboration { error, run_context }) => {
                        return agent_state_with_error_in_context(
                            state,
                            &run_context,
                            format!("Collaboration failed: {error}"),
                        )
                    }
                    Err(AgentRunPreparationError::Runtime { error, run_context }) => {
                        return agent_state_with_error_in_context(state, &run_context, error)
                    }
                };
                run_context = next.run_context;
                runtime = next.runtime;
                prompt = next.prompt;
                collaboration = next.collaboration;
            }
        }
    }
}

enum AgentLoopExecutionOutcome {
    Finished(AgentState),
    Reprepare {
        runtime: agent_runtime::AgentLoopState,
        prompt: String,
    },
}

#[allow(clippy::too_many_arguments)]
fn continue_agent_loop_with_provider(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    mut runtime: agent_runtime::AgentLoopState,
    prompt: String,
    mut run_context: Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    provider: &dyn StreamingModelProvider,
    agent_model: &str,
) -> Result<AgentLoopExecutionOutcome, String> {
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    let registry = tool_registry_for_state(state, workspace_root)?;
    let tools = registry
        .exposure_plan(
            effective_agent_objective(&run_context, &prompt),
            config.context_window_tokens,
        )
        .inline;
    apply_run_task_contract(&mut runtime, &run_context)?;
    let mut runtime_context = agent_runtime_context_for_run(&run_context);
    let mut active_collaboration = collaboration;

    'agent_loop: loop {
        match apply_pending_agent_steers(
            state,
            workspace_root,
            &mut runtime,
            &run_context,
            cancellation,
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
        let terminal_commit = matches!(
            cancellation.continuation_directive(),
            RunContinuationDirective::CommitTerminalResult
        );
        if terminal_commit {
            let previous_message_count = runtime.messages.len();
            ensure_terminal_commit_instruction(&mut runtime);
            if runtime.messages.len() > previous_message_count {
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                persist_new_runtime_messages(
                    &mut store,
                    &runtime.task_id,
                    &runtime.messages,
                    previous_message_count,
                    &run_context,
                )
                .map_err(|error| error.to_string())?;
                persist_agent_runtime_snapshot(&mut store, &runtime, &run_context)?;
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
            provider,
            agent_model,
            &tools,
            request,
            &context_governor,
            terminal_commit,
        )?;
        let AgentModelTurnResponse {
            response,
            request_id,
            streamed_output,
            epoch_lease,
        } = match model_turn {
            AgentModelTurnOutcome::Response(response) => response,
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
            let mut next_runtime = runtime.clone();
            let mut advance =
                AgentKernel::new(&mut next_runtime, &tools).advance_model_response(response);
            let mut verification_required = false;
            if matches!(&advance, AgentAdvance::Completed { .. }) {
                let verification_instruction =
                    AgentKernel::new(&mut next_runtime, &tools).completion_gate_for_task();
                match verification_instruction {
                    Ok(Some(instruction)) => {
                        next_runtime.messages.truncate(previous_message_count);
                        AgentKernel::new(&mut next_runtime, &tools).apply_instruction(&instruction);
                        verification_required = true;
                    }
                    Ok(None) => {}
                    Err(failure) => {
                        next_runtime.messages.truncate(previous_message_count);
                        advance = AgentAdvance::Failed { failure };
                    }
                }
            }
            if let AgentAdvance::Retry { instruction } = &advance {
                AgentKernel::new(&mut next_runtime, &tools)
                    .apply_model_response_retry(instruction.clone());
            }
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            store
                .with_immediate_transaction(|store| {
                    persist_new_runtime_messages(
                        store,
                        &next_runtime.task_id,
                        &next_runtime.messages,
                        previous_message_count,
                        &run_context,
                    )?;
                    persist_agent_runtime_snapshot(store, &next_runtime, &run_context)
                        .map_err(agent_storage::StorageError::new)
                })
                .map_err(|error| error.to_string())?;
            drop(store);
            runtime = next_runtime;
            Ok::<_, String>((advance, verification_required))
        })?;
        let (advance, verification_required) = match response_commit {
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
                    answer,
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
            AgentAdvance::Retry { instruction } => {
                if visible_stream && streamed_output {
                    emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
                }
                let _ = instruction;
                cancellation.mark_progress_at(
                    run_context_steer_epoch(&run_context),
                    "model_retry",
                    "Recovering incomplete model response",
                );
                continue;
            }
            AgentAdvance::Failed { failure } => {
                clear_suspended_agent_run_for_context(state, &run_context)?;
                return agent_state_with_error_in_context(state, &run_context, failure.message)
                    .map(AgentLoopExecutionOutcome::Finished);
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
