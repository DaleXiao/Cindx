use super::*;
use crate::agent_terminal_commit_runtime::persist_agent_terminal_once;
use crate::suspended_run_runtime::clear_suspended_agent_run_for_context;

pub(crate) enum AgentCompletionOutcome {
    Completed(AgentState),
    RestartAfterSteer,
    Paused(AgentState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentCompletionDelivery {
    Actor,
    Finalizer {
        already_persisted: bool,
        used_fallback: bool,
    },
}

impl AgentCompletionDelivery {
    fn is_finalizer(self) -> bool {
        matches!(self, Self::Finalizer { .. })
    }

    fn already_persisted(self) -> bool {
        matches!(
            self,
            Self::Finalizer {
                already_persisted: true,
                ..
            }
        )
    }

    fn used_fallback(self) -> bool {
        matches!(
            self,
            Self::Finalizer {
                used_fallback: true,
                ..
            }
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Actor => "actor",
            Self::Finalizer { .. } => "finalizer",
        }
    }
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

fn routed_terminal_model(config: &ProviderConfig, run_context: &Metadata) -> String {
    crate::configuration_models::agent_model_for_run(config, run_context)
}

fn terminal_selection_is_eligible(
    candidate: &agent_runtime::BestKnownResult,
    delivered_answer: &str,
    exact_content_required: bool,
) -> bool {
    candidate.deliverable && (!exact_content_required || candidate.content == delivered_answer)
}

fn grounded_completion_quality(basis: agent_runtime::GroundedCompletionBasis) -> ResultQuality {
    match basis {
        agent_runtime::GroundedCompletionBasis::SelfContained => ResultQuality::Substantive,
        agent_runtime::GroundedCompletionBasis::EvidenceVisible => ResultQuality::Grounded,
        agent_runtime::GroundedCompletionBasis::PostconditionVerified => ResultQuality::Verified,
        agent_runtime::GroundedCompletionBasis::ConstraintObserved => ResultQuality::Substantive,
    }
}

fn grounded_completion_basis_label(basis: agent_runtime::GroundedCompletionBasis) -> &'static str {
    match basis {
        agent_runtime::GroundedCompletionBasis::SelfContained => "self_contained",
        agent_runtime::GroundedCompletionBasis::EvidenceVisible => "evidence_visible",
        agent_runtime::GroundedCompletionBasis::PostconditionVerified => "postcondition_verified",
        agent_runtime::GroundedCompletionBasis::ConstraintObserved => "constraint_observed",
    }
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
    delivery: AgentCompletionDelivery,
    answer: String,
    mut grounded_completion_receipt: agent_runtime::GroundedCompletionReceipt,
    epoch_lease: agent_runtime::RunEpochLease,
) -> Result<AgentCompletionOutcome, String> {
    if !cancellation.execution_epoch_lease_is_current(epoch_lease) {
        emit_agent_stream_delta(app, request_id, session_id, "", false, true, None);
        return Ok(AgentCompletionOutcome::RestartAfterSteer);
    }
    let receipt_sequences = grounded_completion_receipt
        .visible_evidence_sequences
        .clone();
    grounded_completion_receipt = runtime
        .task_contract
        .rebind_grounded_completion_receipt(
            &grounded_completion_receipt,
            epoch_lease.epoch(),
            runtime.turn,
            &answer,
            &receipt_sequences,
        )
        .map_err(|issue| format!("{} receipt validation failed: {issue:?}", delivery.label()))?;
    let (answer, mut grounded_completion_receipt, judge_disposition) = if delivery.used_fallback()
        || collaboration.is_some()
    {
        (
            answer,
            grounded_completion_receipt,
            "direct_judge_not_applicable".to_string(),
        )
    } else {
        let judge_candidate = crate::agent_finalizer_runtime::GroundedFinalizerCandidate {
            content: answer,
            receipt: grounded_completion_receipt,
            already_persisted: false,
        };
        let (judged, disposition) = crate::direct_judge_runtime::apply_direct_judge_gate(
            state,
            config,
            &runtime.task_id,
            run_context,
            runtime,
            run_context
                .get("agent_model")
                .map(String::as_str)
                .unwrap_or_default(),
            prompt,
            judge_candidate,
        );
        (judged.content, judged.receipt, disposition)
    };
    let mut direct_judge_run_context = run_context.clone();
    direct_judge_run_context.insert(
        "direct_judge_disposition".to_string(),
        judge_disposition,
    );
    let run_context = &direct_judge_run_context;
    let (completion_evidence, routing_learning_eligible) = completion_learning_signal(runtime);
    let tool_evidence =
        crate::agent_result_evidence::completion_tool_evidence(runtime, epoch_lease.epoch());
    let candidate_quality = grounded_completion_quality(grounded_completion_receipt.basis);
    cancellation.record_best_known_result_at(
        epoch_lease.epoch(),
        delivery.label(),
        &answer,
        candidate_quality,
        grounded_completion_receipt.visible_evidence_sequences.len(),
        grounded_completion_receipt.basis
            == agent_runtime::GroundedCompletionBasis::PostconditionVerified,
        true,
    );

    if !streamed_output && !answer.trim().is_empty() {
        emit_agent_stream_delta(app, request_id, session_id, &answer, false, false, None);
    }
    let mut final_answer = answer.clone();
    let delivery_request_id = request_id.to_string();

    let terminal_result_stage = if delivery.used_fallback() {
        "finalizer_fallback"
    } else if delivery.is_finalizer() {
        "finalizer"
    } else if grounded_completion_receipt.basis
        == agent_runtime::GroundedCompletionBasis::ConstraintObserved
    {
        "constrained_executor"
    } else if grounded_completion_receipt.basis
        == agent_runtime::GroundedCompletionBasis::PostconditionVerified
    {
        "verified_executor"
    } else if grounded_completion_receipt.basis
        == agent_runtime::GroundedCompletionBasis::EvidenceVisible
    {
        "grounded_executor"
    } else {
        "executor"
    };
    let terminal_result_quality = grounded_completion_quality(grounded_completion_receipt.basis);
    let terminal_result_verified = grounded_completion_receipt.basis
        == agent_runtime::GroundedCompletionBasis::PostconditionVerified;
    cancellation.record_best_known_result_at(
        epoch_lease.epoch(),
        terminal_result_stage,
        &final_answer,
        terminal_result_quality,
        grounded_completion_receipt.visible_evidence_sequences.len(),
        terminal_result_verified,
        true,
    );
    let exact_content_required = delivery.is_finalizer()
        || grounded_completion_receipt.basis
            != agent_runtime::GroundedCompletionBasis::SelfContained;
    let terminal_selection = cancellation.best_known_result().filter(|candidate| {
        terminal_selection_is_eligible(candidate, &final_answer, exact_content_required)
    });
    let terminal_selection_override =
        terminal_selection_overrides(terminal_selection.as_ref(), &final_answer);
    let persist_selected_terminal_message =
        terminal_selection_override || (delivery.is_finalizer() && !delivery.already_persisted());
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
        grounded_completion_receipt = runtime
            .task_contract
            .rebind_grounded_completion_receipt(
                &grounded_completion_receipt,
                epoch_lease.epoch(),
                runtime.turn,
                &final_answer,
                &grounded_completion_receipt.visible_evidence_sequences,
            )
            .map_err(|issue| format!("terminal receipt rebind failed: {issue:?}"))?;
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
                selector_quality: terminal_result_quality,
                selector_marked_verified: terminal_result_verified,
                selector_marked_deliverable: true,
                selector_evidence_count: grounded_completion_receipt
                    .visible_evidence_sequences
                    .len(),
                trusted_evidence_sequences: &grounded_completion_receipt.visible_evidence_sequences,
            });

    let completion_progress = cancellation.progress();
    let completion_resources = cancellation.resource_usage();
    let completion_usage =
        crate::model_resource_runtime::learning_usage_completeness(&completion_resources);
    let selected_terminal_message_metadata = if persist_selected_terminal_message {
        let mut metadata = metadata_with_context(
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
                    } else if delivery.used_fallback() {
                        "grounded-fallback".to_string()
                    } else {
                        routed_terminal_model(config, run_context)
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
                (
                    "completion_delivery".to_string(),
                    delivery.label().to_string(),
                ),
                (
                    "finalizer_fallback".to_string(),
                    delivery.used_fallback().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        );
        if !grounded_completion_receipt.insert_metadata(&mut metadata) {
            return Err("invalid grounded completion receipt for terminal message".to_string());
        }
        Some(metadata)
    } else {
        None
    };
    let mut terminal_metadata = [
        ("answer_length".to_string(), final_answer.len().to_string()),
        (
            "collaboration".to_string(),
            collaboration.is_some().to_string(),
        ),
        ("collaboration_synthesized".to_string(), "false".to_string()),
        (
            "completion_delivery".to_string(),
            delivery.label().to_string(),
        ),
        (
            "finalizer_fallback".to_string(),
            delivery.used_fallback().to_string(),
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
            grounded_completion_basis_label(grounded_completion_receipt.basis).to_string(),
        ),
        (
            "grounded_completion_basis".to_string(),
            grounded_completion_basis_label(grounded_completion_receipt.basis).to_string(),
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
            runtime
                .pending_interaction_verifications()
                .len()
                .to_string(),
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
        terminal_metadata.insert("collaboration_id".to_string(), collaboration.id.clone());
    }
    crate::model_resource_runtime::add_model_resource_snapshot_metadata(
        &mut terminal_metadata,
        &completion_resources,
    );
    if !grounded_completion_receipt.insert_metadata(&mut terminal_metadata) {
        return Err("invalid grounded completion receipt".to_string());
    }
    terminal_metadata.insert(
        "grounded_completion_status".to_string(),
        "recorded".to_string(),
    );
    if !terminal_outcome_ledger.insert_metadata(&mut terminal_metadata) {
        return Err("invalid terminal outcome ledger".to_string());
    }
    terminal_metadata.insert("outcome_ledger_status".to_string(), "recorded".to_string());
    if agent_runtime::GroundedCompletionReceipt::from_terminal_metadata(
        &terminal_metadata,
        &final_answer,
    )
    .is_none()
    {
        return Err("grounded terminal lineage validation failed".to_string());
    }
    let memory_attribution_observation =
        crate::memory_projection_runtime::attribution::CompletionMemoryAttributionObservation::from_runtime(
            runtime,
            epoch_lease.epoch(),
            &terminal_metadata,
        );
    let mut prompt_learning_eligible = false;
    let terminal_commit = cancellation.commit_terminal_result_with(epoch_lease, || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        persist_agent_terminal_once(
            &mut store,
            &runtime.task_id,
            run_context,
            epoch_lease.epoch(),
            |store, terminal_identity| {
                if let Some(message_metadata) = selected_terminal_message_metadata {
                    append_message_event_with_metadata(
                        store,
                        &runtime.task_id,
                        MessageRole::Assistant,
                        &final_answer,
                        message_metadata,
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
                let mut terminal_metadata = terminal_metadata;
                terminal_metadata.extend(terminal_identity.metadata());
                let learning_tool_evidence = crate::agent_result_evidence::CompletionToolEvidence {
                    grounded_count: grounded_completion_receipt.visible_evidence_sequences.len(),
                    verified_postcondition_count: usize::from(
                        grounded_completion_receipt.basis
                            == agent_runtime::GroundedCompletionBasis::PostconditionVerified,
                    ),
                    trusted_contract_sequences: grounded_completion_receipt
                        .visible_evidence_sequences
                        .clone(),
                };
                let learning_evidence = crate::agent_result_evidence::completion_learning_evidence(
                    run_context,
                    completion_usage,
                    epoch_lease.epoch(),
                    learning_tool_evidence,
                    workflow_terminal.as_ref(),
                );
                prompt_learning_eligible = learning_evidence.is_learnable();
                let encoded_learning_evidence = learning_evidence.to_metadata_value();
                if let Some(encoded) = encoded_learning_evidence {
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
                if let Err(error) =
                    crate::memory_projection_runtime::attribution::append_project_memory_attribution(
                        store,
                        &runtime.task_id,
                        run_context,
                        epoch_lease.epoch(),
                        &memory_attribution_observation,
                    )
                {
                    eprintln!("project memory attribution unavailable: {error}");
                }
                delete_persisted_agent_runtime_snapshot(store, session_id)
                    .map_err(agent_storage::StorageError::new)?;
                agent_state_for_session(store, None, session_id)
            },
        )
        .map_err(|error| error.to_string())
    })?;
    let (completed_state, inserted_terminal) = match terminal_commit {
        agent_runtime::RunTerminalCommit::Committed(persisted) => {
            (persisted.state, persisted.inserted)
        }
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
            (
                agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string())?,
                false,
            )
        }
    };
    if let Err(error) = clear_suspended_agent_run_for_context(state, run_context) {
        eprintln!("completed agent suspended-run cleanup unavailable: {error}");
    }
    emit_agent_stream_delta(app, &delivery_request_id, session_id, "", true, false, None);
    if inserted_terminal {
        if prompt_learning_eligible {
            if let Err(error) =
                crate::prompt_terminal_learning_runtime::schedule_terminal_prompt_pairwise_evaluation(
                    app,
                    &runtime.task_id,
                    config,
                    run_context,
                )
            {
                eprintln!("terminal prompt evaluation could not be scheduled: {error}");
            }
        }
        crate::semantic_memory_worker::schedule_semantic_memory_refresh(
            app.clone(),
            workspace_root.to_path_buf(),
            config.clone(),
            run_context.clone(),
        );
    }
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

    #[test]
    fn evidence_bound_result_cannot_be_replaced_by_an_unreceipted_frontier_candidate() {
        let selected = candidate("different", "frontier");
        assert!(!terminal_selection_is_eligible(&selected, "answer", true));
        assert!(terminal_selection_is_eligible(&selected, "answer", false));
        let exact = candidate("answer", "frontier");
        assert!(terminal_selection_is_eligible(&exact, "answer", true));
    }

    #[test]
    fn evidence_bound_actor_answer_is_not_replaced_by_a_different_candidate() {
        let longer_executor = candidate("a much longer executor draft", "executor");
        assert!(!terminal_selection_is_eligible(
            &longer_executor,
            "short synthesis",
            true
        ));
    }

    #[test]
    fn finalizer_metadata_uses_the_routed_owner_model_not_the_utility_profile() {
        let config = ProviderConfig {
            executor_model: "primary".to_string(),
            summarizer_model: "utility".to_string(),
            ..ProviderConfig::default()
        };
        let run_context = Metadata::from([(
            "agent_model".to_string(),
            "routed-primary".to_string(),
        )]);

        assert_eq!(
            routed_terminal_model(&config, &run_context),
            "routed-primary"
        );
        assert_ne!(
            routed_terminal_model(&config, &run_context),
            config.model_for_role(&ModelRole::Summarizer)
        );
    }

    #[test]
    fn delivery_stage_never_inflates_epistemic_quality() {
        assert_eq!(
            grounded_completion_quality(agent_runtime::GroundedCompletionBasis::SelfContained),
            ResultQuality::Substantive
        );
        assert_eq!(
            grounded_completion_quality(agent_runtime::GroundedCompletionBasis::EvidenceVisible),
            ResultQuality::Grounded
        );
        assert_eq!(
            grounded_completion_quality(
                agent_runtime::GroundedCompletionBasis::PostconditionVerified
            ),
            ResultQuality::Verified
        );
        assert_eq!(
            grounded_completion_quality(agent_runtime::GroundedCompletionBasis::ConstraintObserved),
            ResultQuality::Substantive
        );
    }
}
