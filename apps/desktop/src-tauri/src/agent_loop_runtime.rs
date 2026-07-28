use crate::desktop_prelude::*;
use crate::{
    agent_commands::{
        add_attachment_metadata, prompt_with_attachments, validate_agent_attachments,
    },
    agent_completion_runtime::{finalize_agent_completion, AgentCompletionOutcome},
    agent_model_turn_runtime::{
        execute_agent_model_turn, AgentModelTurnOutcome, AgentModelTurnResponse,
    },
    agent_query_commands::{
        append_agent_progress_event, append_agent_queue_event, emit_agent_stream_delta,
        finish_agent_run_for_control_stop_with_task_state,
    },
    agent_read_model::agent_state_with_error_in_context,
    agent_runtime_snapshot::{
        capture_persistable_agent_task_state, persist_agent_runtime_snapshot,
    },
    agent_tool_runtime::{execute_agent_tool_batch, AgentToolBatchOutcome},
    app_state::{AppState, SuspendedAgentRun},
    configuration_models::{agent_model_for_run, ProviderConfig},
    event_persistence::persist_new_runtime_messages,
    persistence_runtime::{open_app_read_store, tool_registry_for_state},
    runtime_constants::{AGENT_MAX_OUTPUT_TOKENS, AGENT_MODEL_RECOVERY_WINDOW_SECONDS},
    runtime_values::{
        add_image_generation_run_context, agent_runtime_context_for_run, current_time_millis,
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

pub(crate) fn apply_pending_agent_steers(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
    cancellation: &AgentRunControl,
) -> Result<Option<String>, String> {
    let pending_ids = cancellation.take_pending_steers();
    if pending_ids.is_empty() {
        return Ok(None);
    }
    let Some(session_id) = run_context.get("session_id") else {
        return Ok(None);
    };
    let pending = {
        let store = open_app_read_store()?;
        let model = load_agent_session_read_model_snapshot(&store, session_id)
            .map_err(|error| error.to_string())?;
        pending_ids
            .into_iter()
            .filter_map(|pending| {
                let view = model
                    .state
                    .queued_messages
                    .iter()
                    .find(|message| message.id == pending.queue_id)?
                    .clone();
                let payload = model.queued_payloads.get(&pending.queue_id)?.clone();
                Some((view, payload))
            })
            .collect::<Vec<_>>()
    };

    let mut latest_prompt = None;
    for (view, payload) in pending {
        let attachments = validate_agent_attachments(workspace_root, payload.attachments)?;
        let model_prompt = prompt_with_attachments(&payload.prompt, &attachments);
        let previous_message_count = runtime.messages.len();
        let mut metadata = [
            ("queue_id".to_string(), view.id.clone()),
            ("queue_mode".to_string(), "steer".to_string()),
            ("display_content".to_string(), payload.prompt.clone()),
        ]
        .into_iter()
        .collect::<Metadata>();
        add_attachment_metadata(&mut metadata, &attachments);
        AgentKernel::new(runtime, &[]).apply_steer(model_prompt.clone(), metadata);

        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_agent_queue_event(
            &mut store,
            run_context,
            "start",
            &view.id,
            "steer",
            view.created_at_ms,
            None,
        )?;
        persist_new_runtime_messages(
            &mut store,
            &runtime.task_id,
            &runtime.messages,
            previous_message_count,
            run_context,
        )
        .map_err(|error| error.to_string())?;
        persist_agent_runtime_snapshot(&mut store, runtime, run_context)?;
        latest_prompt = Some(model_prompt);
    }
    if latest_prompt.is_some() {
        cancellation.record_checkpoint(
            "steering",
            "User guidance applied",
            latest_prompt.as_deref().unwrap_or_default(),
        );
    }
    Ok(latest_prompt)
}

pub(crate) fn apply_run_task_contract(
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
) {
    if run_context
        .get("image_generation_required")
        .map(String::as_str)
        == Some("true")
    {
        AgentKernel::new(runtime, &[]).require_tool_success("image.generate");
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn continue_agent_loop(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    runtime: agent_runtime::AgentLoopState,
    prompt: String,
    run_context: Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
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
    continue_agent_loop_with_provider(
        app,
        state,
        config,
        workspace_root,
        runtime,
        prompt,
        run_context,
        collaboration,
        cancellation,
        &provider,
        &agent_model,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn continue_agent_loop_with_provider(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    mut runtime: agent_runtime::AgentLoopState,
    mut prompt: String,
    mut run_context: Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    provider: &dyn StreamingModelProvider,
    agent_model: &str,
) -> Result<AgentState, String> {
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    let registry = tool_registry_for_state(state, workspace_root)?;
    let mut tools = registry
        .exposure_plan(&prompt, config.context_window_tokens)
        .inline;
    apply_run_task_contract(&mut runtime, &run_context);
    let mut runtime_context = agent_runtime_context_for_run(&run_context);
    let mut active_collaboration = collaboration;

    'agent_loop: loop {
        if let Some(steer_prompt) = apply_pending_agent_steers(
            state,
            workspace_root,
            &mut runtime,
            &run_context,
            cancellation,
        )? {
            active_collaboration = None;
            prompt = steer_prompt;
            add_image_generation_run_context(&mut run_context, config, &prompt);
            tools = registry
                .exposure_plan(&prompt, config.context_window_tokens)
                .inline;
            apply_run_task_contract(&mut runtime, &run_context);
            runtime_context = agent_runtime_context_for_run(&run_context);
            cancellation.mark_progress("steering", "User guidance applied");
            append_agent_progress_event(
                state,
                &runtime.task_id,
                &run_context,
                "Applying user steering",
            )?;
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
        let verification_required = run_context
            .get("verification_required")
            .is_some_and(|value| value == "true");
        let prepared_turn = match AgentKernel::new(&mut runtime, &tools)
            .prepare_model_turn_with_contract(
                Some(&config.agent_system_prompt),
                runtime_context.as_deref(),
                verification_required,
                config.context_window_tokens,
                max_output_tokens,
            ) {
            Ok(prepared_turn) => prepared_turn,
            Err(AgentTurnPreparationError::Budget(exhausted)) => {
                if let Some(partial_answer) = exhausted.partial_answer {
                    cancellation.record_partial_output(&partial_answer);
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
                );
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
        } = match model_turn {
            AgentModelTurnOutcome::Response(response) => response,
            AgentModelTurnOutcome::RestartAfterSteer => {
                active_collaboration = None;
                continue 'agent_loop;
            }
            AgentModelTurnOutcome::Finished(agent_state) => return Ok(*agent_state),
        };
        let visible_stream = active_collaboration.is_none();
        let previous_message_count = runtime.messages.len();
        let mut advance = AgentKernel::new(&mut runtime, &tools).advance_model_response(response);
        if matches!(&advance, AgentAdvance::Completed { .. }) {
            let verification_instruction =
                AgentKernel::new(&mut runtime, &tools).completion_gate(verification_required);
            match verification_instruction {
                Ok(Some(instruction)) => {
                    runtime.messages.truncate(previous_message_count);
                    AgentKernel::new(&mut runtime, &tools).apply_instruction(&instruction);
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
                    cancellation
                        .mark_progress("verification", "Waiting for task-contract evidence");
                    continue;
                }
                Ok(None) => {}
                Err(failure) => {
                    runtime.messages.truncate(previous_message_count);
                    advance = AgentAdvance::Failed { failure };
                }
            }
        }
        {
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
                )? {
                    AgentCompletionOutcome::Completed(agent_state) => return Ok(agent_state),
                    AgentCompletionOutcome::RestartAfterSteer => {
                        active_collaboration = None;
                        continue 'agent_loop;
                    }
                    AgentCompletionOutcome::Paused(agent_state) => return Ok(agent_state),
                }
            }
            AgentAdvance::TurnBudgetExhausted(exhausted) => {
                if let Some(partial_answer) = exhausted.partial_answer {
                    cancellation.record_partial_output(&partial_answer);
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
                );
            }
            AgentAdvance::Retry { instruction } => {
                if visible_stream && streamed_output {
                    emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
                }
                let previous_message_count = runtime.messages.len();
                AgentKernel::new(&mut runtime, &tools).apply_model_response_retry(instruction);
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
                drop(store);
                cancellation.mark_progress("model_retry", "Recovering incomplete model response");
                continue;
            }
            AgentAdvance::Failed { failure } => {
                clear_suspended_agent_run_for_context(state, &run_context)?;
                return agent_state_with_error_in_context(state, &run_context, failure.message);
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
                    &registry,
                    &tools,
                    calls,
                )? {
                    AgentToolBatchOutcome::Continue => {}
                    AgentToolBatchOutcome::RestartAfterSteer => {
                        active_collaboration = None;
                        continue 'agent_loop;
                    }
                    AgentToolBatchOutcome::Paused(agent_state) => return Ok(*agent_state),
                }
            }
        }
    }
}
