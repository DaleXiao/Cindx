use super::*;
use crate::suspended_run_runtime::clear_suspended_agent_run_for_context;

pub(crate) enum AgentCompletionOutcome {
    Completed(AgentState),
    RestartAfterSteer,
    Paused(AgentState),
}

fn terminal_selection_overrides(
    selection: Option<&agent_runtime::BestKnownResult>,
    delivered_answer: &str,
) -> bool {
    selection.is_some_and(|candidate| candidate.content.trim() != delivered_answer.trim())
}

fn terminal_selection_for_delivered<'a>(
    selection: Option<&'a agent_runtime::BestKnownResult>,
    delivered_answer: &str,
) -> Option<&'a agent_runtime::BestKnownResult> {
    selection.filter(|candidate| candidate.content == delivered_answer)
}

fn terminal_selection_stage(
    selection: Option<&agent_runtime::BestKnownResult>,
    fallback_stage: &str,
) -> String {
    selection
        .map(|candidate| candidate.stage.clone())
        .unwrap_or_else(|| fallback_stage.to_string())
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
    epoch_lease: agent_runtime::RunEpochLease,
) -> Result<AgentCompletionOutcome, String> {
    if !cancellation.execution_epoch_lease_is_current(epoch_lease) {
        emit_agent_stream_delta(app, request_id, session_id, "", false, true, None);
        return Ok(AgentCompletionOutcome::RestartAfterSteer);
    }
    let (completion_evidence, routing_learning_eligible) = completion_learning_signal(runtime);
    let tool_evidence =
        crate::agent_result_evidence::completion_tool_evidence(runtime, epoch_lease.epoch());
    cancellation.record_best_known_result_at(
        epoch_lease.epoch(),
        "executor",
        &answer,
        if tool_evidence.grounded_count > 0 {
            ResultQuality::Grounded
        } else {
            ResultQuality::Substantive
        },
        tool_evidence.grounded_count,
        false,
        true,
    );

    let (mut final_answer, synthesized, delivery_request_id) = if let Some(collaboration) =
        collaboration
    {
        let synthesis_objective = effective_agent_objective(run_context, prompt);
        match synthesize_agent_answer(
            app,
            state,
            config,
            runtime,
            synthesis_objective,
            &answer,
            run_context,
            collaboration,
            cancellation,
        ) {
            Ok(answer) => (answer.content, true, answer.stream_request_id),
            Err(_)
                if !cancellation.execution_epoch_lease_is_current(epoch_lease)
                    && !agent_run_should_stop(cancellation) =>
            {
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
                (answer.clone(), false, request_id.to_string())
            }
        }
    } else {
        if !streamed_output && !answer.trim().is_empty() {
            emit_agent_stream_delta(app, request_id, session_id, &answer, false, false, None);
        }
        (answer.clone(), false, request_id.to_string())
    };

    let terminal_result_stage = if synthesized {
        "synthesizer"
    } else if tool_evidence.verified_postcondition_count > 0 {
        "verified_executor"
    } else if tool_evidence.grounded_count > 0 {
        "grounded_executor"
    } else {
        "executor"
    };
    let terminal_result_quality = if synthesized {
        ResultQuality::Synthesized
    } else if tool_evidence.verified_postcondition_count > 0 {
        ResultQuality::Verified
    } else if tool_evidence.grounded_count > 0 {
        ResultQuality::Grounded
    } else {
        ResultQuality::Substantive
    };
    let terminal_result_verified = tool_evidence.verified_postcondition_count > 0;
    cancellation.record_best_known_result_at(
        epoch_lease.epoch(),
        terminal_result_stage,
        &final_answer,
        terminal_result_quality,
        tool_evidence.grounded_count,
        terminal_result_verified,
        true,
    );
    let terminal_selection = cancellation
        .best_known_result()
        .filter(|candidate| candidate.deliverable);
    let terminal_selection_override =
        terminal_selection_overrides(terminal_selection.as_ref(), &final_answer);
    let persist_selected_terminal_message = synthesized || terminal_selection_override;
    if let Some(selected) = terminal_selection
        .as_ref()
        .filter(|_| terminal_selection_override)
    {
        final_answer = selected.content.clone();
        emit_agent_stream_delta(app, &delivery_request_id, session_id, "", false, true, None);
        emit_agent_stream_delta(
            app,
            &delivery_request_id,
            session_id,
            &final_answer,
            false,
            false,
            None,
        );
    }
    let delivered_selection =
        terminal_selection_for_delivered(terminal_selection.as_ref(), &final_answer);
    let terminal_selected_stage =
        terminal_selection_stage(delivered_selection, terminal_result_stage);
    let terminal_outcome_ledger =
        runtime
            .task_contract
            .completed_outcome_ledger(agent_runtime::OutcomeTerminalObservation {
                steer_epoch: epoch_lease.epoch(),
                model_turn: runtime.turn,
                answer: &final_answer,
                selected_stage: &terminal_selected_stage,
                selector_quality: delivered_selection
                    .map(|candidate| candidate.quality)
                    .unwrap_or(terminal_result_quality),
                selector_marked_verified: delivered_selection
                    .map(|candidate| candidate.verified)
                    .unwrap_or(terminal_result_verified),
                selector_marked_deliverable: delivered_selection
                    .map(|candidate| candidate.deliverable)
                    .unwrap_or(true),
                selector_evidence_count: delivered_selection
                    .map(|candidate| candidate.evidence_count)
                    .unwrap_or(tool_evidence.grounded_count),
                trusted_evidence_sequences: &tool_evidence.trusted_contract_sequences,
            });

    let completion_progress = cancellation.progress();
    let completion_resources = cancellation.resource_usage();
    let completion_usage =
        crate::model_resource_runtime::learning_usage_completeness(&completion_resources);
    let terminal_commit = cancellation.commit_terminal_result_with(epoch_lease, || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .with_immediate_transaction(|store| {
                if persist_selected_terminal_message {
                    append_message_event_with_metadata(
                        store,
                        &runtime.task_id,
                        MessageRole::Assistant,
                        &final_answer,
                        metadata_with_context(
                            [
                                (
                                    "collaboration_final".to_string(),
                                    collaboration.is_some().to_string(),
                                ),
                                ("terminal_selected".to_string(), "true".to_string()),
                                (
                                    "model".to_string(),
                                    if terminal_selection_override {
                                        "result-frontier".to_string()
                                    } else {
                                        config.model_for_role(&ModelRole::Summarizer)
                                    },
                                ),
                                (
                                    "terminal_selected_stage".to_string(),
                                    terminal_selected_stage.clone(),
                                ),
                                (
                                    "terminal_selection_override".to_string(),
                                    terminal_selection_override.to_string(),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                            run_context,
                        ),
                    )?;
                }
                let workflow_terminal = collaboration.and_then(|collaboration| {
                    store
                        .list_by_task_and_metadata(
                            &runtime.task_id,
                            "collaboration_id",
                            &collaboration.id,
                        )
                        .ok()?
                        .into_iter()
                        .rev()
                        .find(|event| {
                            event.summary == "Collaboration workflow completed"
                                && event
                                    .metadata
                                    .get("steer_epoch")
                                    .and_then(|value| value.parse::<u64>().ok())
                                    == Some(epoch_lease.epoch())
                        })
                });
                let mut terminal_metadata = [
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
                        "terminal_selected_stage".to_string(),
                        terminal_selected_stage.clone(),
                    ),
                    (
                        "terminal_selection_override".to_string(),
                        terminal_selection_override.to_string(),
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
                        "successful_tool_evidence".to_string(),
                        tool_evidence.grounded_count.to_string(),
                    ),
                    (
                        "verified_postcondition_evidence".to_string(),
                        tool_evidence.verified_postcondition_count.to_string(),
                    ),
                    (
                        "completion_verification_state".to_string(),
                        if tool_evidence.verified_postcondition_count > 0 {
                            "verified_postcondition"
                        } else {
                            "not_verified"
                        }
                        .to_string(),
                    ),
                    (
                        "user_approval_state".to_string(),
                        "not_observed".to_string(),
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
                    ("last_stage".to_string(), completion_progress.stage.clone()),
                    ("steer_epoch".to_string(), epoch_lease.epoch().to_string()),
                ]
                .into_iter()
                .collect::<Metadata>();
                if let Some(collaboration) = collaboration {
                    terminal_metadata
                        .insert("collaboration_id".to_string(), collaboration.id.clone());
                }
                crate::model_resource_runtime::add_model_resource_snapshot_metadata(
                    &mut terminal_metadata,
                    &completion_resources,
                );
                let outcome_ledger_recorded =
                    terminal_outcome_ledger.insert_metadata(&mut terminal_metadata);
                terminal_metadata.insert(
                    "outcome_ledger_status".to_string(),
                    if outcome_ledger_recorded {
                        "recorded"
                    } else {
                        "omitted_invalid"
                    }
                    .to_string(),
                );
                let learning_evidence = crate::agent_result_evidence::completion_learning_evidence(
                    run_context,
                    completion_usage,
                    epoch_lease.epoch(),
                    tool_evidence,
                    workflow_terminal.as_ref(),
                );
                if let Some(encoded) = learning_evidence.to_metadata_value() {
                    terminal_metadata.insert(
                        orchestrator::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                        encoded,
                    );
                }
                append_event(
                    store,
                    &runtime.task_id,
                    EventKind::TaskStatusChanged,
                    "Agent task completed",
                    metadata_with_context(terminal_metadata, run_context),
                )?;
                delete_persisted_agent_runtime_snapshot(store, session_id)
                    .map_err(agent_storage::StorageError::new)?;
                if let Err(error) = record_project_memory_observed_use(
                    store,
                    &runtime.task_id,
                    run_context,
                    &final_answer,
                ) {
                    eprintln!("project memory utilization unavailable: {error}");
                }
                agent_state_for_session(store, None, session_id)
            })
            .map_err(|error| error.to_string())
    })?;
    let completed_state = match terminal_commit {
        agent_runtime::RunTerminalCommit::Committed(state) => state,
        agent_runtime::RunTerminalCommit::RestartAfterSteer => {
            emit_agent_stream_delta(app, &delivery_request_id, session_id, "", false, true, None);
            return Ok(AgentCompletionOutcome::RestartAfterSteer);
        }
        agent_runtime::RunTerminalCommit::Stopped(_) => {
            return Ok(AgentCompletionOutcome::Paused(
                pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    runtime,
                    prompt,
                    run_context,
                    collaboration,
                    cancellation,
                )?,
            ));
        }
        agent_runtime::RunTerminalCommit::AlreadyCommitted => {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?
        }
    };
    if let Err(error) = clear_suspended_agent_run_for_context(state, run_context) {
        eprintln!("completed agent suspended-run cleanup unavailable: {error}");
    }
    emit_agent_stream_delta(app, &delivery_request_id, session_id, "", true, false, None);
    crate::semantic_memory_worker::schedule_semantic_memory_refresh(
        app.clone(),
        workspace_root.to_path_buf(),
        config.clone(),
        run_context.clone(),
    );
    Ok(AgentCompletionOutcome::Completed(completed_state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(content: &str, stage: &str) -> agent_runtime::BestKnownResult {
        agent_runtime::BestKnownResult {
            content: content.to_string(),
            stage: stage.to_string(),
            quality: ResultQuality::Verified,
            evidence_count: 1,
            verified: true,
            deliverable: true,
        }
    }

    #[test]
    fn terminal_provenance_uses_exact_delivered_bytes_and_real_fallback_stage() {
        let selected = candidate("answer", "verified_executor");
        assert!(!terminal_selection_overrides(Some(&selected), "answer"));
        assert!(!terminal_selection_overrides(Some(&selected), "answer "));
        assert!(terminal_selection_overrides(Some(&selected), "different"));
        assert_eq!(
            terminal_selection_for_delivered(Some(&selected), "answer"),
            Some(&selected)
        );
        assert!(terminal_selection_for_delivered(Some(&selected), "answer ").is_none());
        assert_eq!(
            terminal_selection_stage(Some(&selected), "synthesizer"),
            "verified_executor"
        );
        assert_eq!(terminal_selection_stage(None, "synthesizer"), "synthesizer");
    }
}
