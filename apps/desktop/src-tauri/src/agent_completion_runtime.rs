use super::*;

pub(crate) enum AgentCompletionOutcome {
    Completed(AgentState),
    RestartAfterSteer,
    Paused(AgentState),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_agent_completion(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    request_id: &str,
    session_id: Option<&str>,
    streamed_output: bool,
    answer: String,
) -> Result<AgentCompletionOutcome, String> {
    clear_suspended_agent_run_for_context(state, run_context)?;
    let (completion_evidence, routing_learning_eligible) = completion_learning_signal(runtime);
    let completion_evidence_count = runtime
        .messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::Tool))
        .count();
    cancellation.record_best_known_result(
        "executor",
        &answer,
        if completion_evidence_count > 0 {
            ResultQuality::Grounded
        } else {
            ResultQuality::Substantive
        },
        completion_evidence_count,
        false,
        true,
    );

    let (final_answer, synthesized) = if let Some(collaboration) = collaboration {
        match synthesize_agent_answer(
            app,
            state,
            config,
            runtime,
            prompt,
            &answer,
            run_context,
            collaboration,
            cancellation,
        ) {
            Ok(answer) => (answer, true),
            Err(_) if cancellation.has_pending_steer() && !agent_run_should_stop(cancellation) => {
                emit_agent_stream_delta(app, request_id, session_id, "", false, true, None);
                return Ok(AgentCompletionOutcome::RestartAfterSteer);
            }
            Err(_) if agent_run_should_stop(cancellation) => {
                return Ok(AgentCompletionOutcome::Paused(
                    pause_agent_loop_for_control_stop(
                        app,
                        state,
                        workspace_root,
                        runtime,
                        prompt,
                        run_context,
                        Some(collaboration),
                        cancellation,
                    )?,
                ));
            }
            Err(_) => {
                emit_agent_stream_delta(app, request_id, session_id, "", false, true, None);
                emit_agent_stream_delta(app, request_id, session_id, &answer, false, false, None);
                (answer.clone(), false)
            }
        }
    } else {
        if !streamed_output && !answer.trim().is_empty() {
            emit_agent_stream_delta(app, request_id, session_id, &answer, false, false, None);
        }
        (answer.clone(), false)
    };

    cancellation.record_best_known_result(
        if synthesized {
            "synthesizer"
        } else if completion_evidence_count > 0 {
            "verified_executor"
        } else {
            "executor"
        },
        &final_answer,
        if synthesized {
            ResultQuality::Synthesized
        } else if completion_evidence_count > 0 {
            ResultQuality::Verified
        } else {
            ResultQuality::Substantive
        },
        completion_evidence_count,
        completion_evidence_count > 0,
        true,
    );

    let completion_progress = cancellation.progress();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    if synthesized {
        append_message_event_with_metadata(
            &mut store,
            &runtime.task_id,
            MessageRole::Assistant,
            &final_answer,
            metadata_with_context(
                [
                    ("collaboration_final".to_string(), "true".to_string()),
                    (
                        "model".to_string(),
                        config.model_for_role(&ModelRole::Summarizer),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    append_event(
        &mut store,
        &runtime.task_id,
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [
                ("answer_length".to_string(), final_answer.len().to_string()),
                (
                    "collaboration".to_string(),
                    collaboration.is_some().to_string(),
                ),
                (
                    "collaboration_synthesized".to_string(),
                    synthesized.to_string(),
                ),
                (
                    "elapsed_ms".to_string(),
                    completion_progress.elapsed.as_millis().to_string(),
                ),
                (
                    "model_calls".to_string(),
                    completion_progress.model_calls.to_string(),
                ),
                (
                    "tool_calls".to_string(),
                    completion_progress.tool_calls.to_string(),
                ),
                (
                    "agent_turns".to_string(),
                    completion_progress.agent_turns.to_string(),
                ),
                (
                    "repair_attempts".to_string(),
                    completion_progress.repair_attempts.to_string(),
                ),
                (
                    "completion_evidence".to_string(),
                    completion_evidence.to_string(),
                ),
                (
                    "routing_learning_eligible".to_string(),
                    routing_learning_eligible.to_string(),
                ),
                (
                    "verification_gate_requests".to_string(),
                    runtime.verification_gate_requests.to_string(),
                ),
                (
                    "interaction_verification_gate_requests".to_string(),
                    runtime.interaction_verification_gate_requests.to_string(),
                ),
                (
                    "verified_interactions".to_string(),
                    runtime.verified_interactions.to_string(),
                ),
                (
                    "pending_interaction_verifications".to_string(),
                    runtime.pending_interaction_verifications.len().to_string(),
                ),
                (
                    "checkpoints".to_string(),
                    completion_progress.checkpoints.to_string(),
                ),
                (
                    "observations".to_string(),
                    completion_progress.observations.to_string(),
                ),
                (
                    "budget_extensions".to_string(),
                    completion_progress.budget_extensions.to_string(),
                ),
                ("last_stage".to_string(), completion_progress.stage),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    if let Err(error) =
        record_project_memory_observed_use(&mut store, &runtime.task_id, run_context, &final_answer)
    {
        eprintln!("project memory utilization unavailable: {error}");
    }
    let memory_ledger = match refresh_project_memory_after_run(&mut store, run_context) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("project memory checkpoint unavailable: {error}");
            None
        }
    };
    let completed_state =
        agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?;
    drop(store);
    emit_agent_stream_delta(app, request_id, session_id, "", true, false, None);
    if let Some(ledger) = memory_ledger {
        schedule_project_memory_vector_refresh(
            workspace_root.to_path_buf(),
            config.clone(),
            ledger,
        );
    }
    Ok(AgentCompletionOutcome::Completed(completed_state))
}
