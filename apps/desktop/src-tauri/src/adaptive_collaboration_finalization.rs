use super::collaboration_service::WorkflowRoleCoverage;
use super::*;

pub(crate) fn select_adaptive_guidance(
    decision: Option<&UpliftGateDecision>,
    team_candidate_id: &str,
    anchor_available: bool,
    frontier_candidate: Option<String>,
) -> (Option<String>, bool) {
    if matches!(decision, Some(UpliftGateDecision::AcceptTeam)) {
        return (Some(team_candidate_id.to_string()), true);
    }
    (
        anchor_available
            .then(|| DIRECT_ANCHOR_CANDIDATE_ID.to_string())
            .or(frontier_candidate),
        false,
    )
}

pub(super) struct AdaptiveCollaborationFinalization<'a, 'state> {
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) effort: String,
    pub(super) prompt_genome: ConductorPromptGenome,
    pub(super) workflow_started_at_ms: u64,
    pub(super) final_step_id: String,
    pub(super) workflow_steps: usize,
    pub(super) layer_count: usize,
    pub(super) role_coverage: WorkflowRoleCoverage,
    pub(super) evidence_count: usize,
    pub(super) quality_gate: AdaptiveQualityGateResult,
    pub(super) final_output: String,
    pub(super) direct_anchor_output: Option<&'a str>,
    pub(super) cancellation: Option<&'a Arc<AgentRunControl>>,
    pub(super) anchor_supervisor: Option<&'a ParallelJobSupervisor<CollaborationCompletion>>,
    pub(super) direct_anchor_verifier: Option<&'a DirectAnchorVerifier>,
    pub(super) anytime_controller: &'a mut AnytimeController,
    pub(super) workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
}

pub(super) fn finalize_adaptive_collaboration(
    context: AdaptiveCollaborationFinalization<'_, '_>,
) -> Result<AdaptiveCollaborationOutcome, String> {
    let AdaptiveCollaborationFinalization {
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        effort,
        prompt_genome,
        workflow_started_at_ms,
        final_step_id,
        workflow_steps,
        layer_count,
        role_coverage,
        evidence_count,
        quality_gate,
        mut final_output,
        direct_anchor_output,
        cancellation,
        anchor_supervisor,
        direct_anchor_verifier,
        anytime_controller,
        workflow_checkpoint,
    } = context;

    let workflow_verification_satisfied = workflow_checkpoint.workflow_verification_satisfied(
        anytime_controller.snapshot().config.verification_required,
    );
    if enforce_candidate_verification_gate(
        anytime_controller,
        &final_step_id,
        workflow_verification_satisfied,
    )? {
        persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
    }
    let final_candidate_verified = quality_gate.passed && workflow_verification_satisfied;

    let pairwise_comparison = direct_anchor_output.and_then(|anchor_output| {
        match compare_team_guidance_with_anchor(
            state,
            config,
            task_id,
            run_context,
            collaboration_id,
            prompt,
            &final_output,
            anchor_output,
        ) {
            Ok(comparison) => Some(comparison),
            Err(error) => {
                if let Ok(mut store) = state.store.lock() {
                    let _ = append_event(
                        &mut store,
                        task_id,
                        EventKind::TaskStatusChanged,
                        "Collaboration team-anchor comparison unavailable",
                        metadata_with_context(
                            [
                                ("collaboration_id".to_string(), collaboration_id.to_string()),
                                ("comparison_available".to_string(), "false".to_string()),
                                (
                                    "comparison_error".to_string(),
                                    truncate_for_collaboration(&error, 2_000),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                            run_context,
                        ),
                    );
                }
                None
            }
        }
    });
    controller_mark_running_if_pending(anytime_controller, &final_step_id)?;
    if anytime_controller
        .candidate(&final_step_id)
        .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Running)
    {
        anytime_controller.observe(
            &final_step_id,
            AnytimeVerdict {
                quality_bps: (quality_gate.score.clamp(0.0, 1.0) * 10_000.0).round() as u16,
                confidence_bps: if final_candidate_verified {
                    8_000
                } else {
                    5_000
                },
                constraint_coverage_bps: if final_candidate_verified {
                    8_500
                } else {
                    6_000
                },
                evidence_count,
                safety_violations: quality_gate.safety_violations.saturating_add(
                    pairwise_comparison
                        .as_ref()
                        .map_or(0, |comparison| comparison.team_safety_violations),
                ),
                deliverable: !final_output.trim().is_empty(),
                verified: final_candidate_verified,
                anchor_uplift_bps: pairwise_comparison
                    .as_ref()
                    .map(|comparison| comparison.team_uplift_bps),
            },
        )?;
    }
    workflow_checkpoint
        .anytime_outputs
        .insert(final_step_id.clone(), final_output.clone());
    persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
    if let Some(control) = cancellation {
        control.record_best_known_result_at(
            run_context_steer_epoch(run_context),
            "anytime_workflow_final",
            &final_output,
            if final_candidate_verified {
                ResultQuality::Verified
            } else {
                ResultQuality::Grounded
            },
            evidence_count,
            final_candidate_verified,
            false,
        );
    }
    let uplift_repair = repair_adaptive_uplift(AdaptiveUpliftRepairContext {
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        verification: prompt_genome.verification,
        evidence_count,
        team_candidate_id: &final_step_id,
        team_output: &final_output,
        anchor_output: direct_anchor_output,
        cancellation,
        anytime_controller,
        workflow_checkpoint,
    })?;
    if collaboration_steer_pending(cancellation) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            workflow_checkpoint,
            anytime_controller,
            anchor_supervisor,
            direct_anchor_verifier,
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let (remaining_ms, terminal_reserve_ms) = cancellation.map_or((u64::MAX, 0), |control| {
        let progress = control.progress();
        let budget = control.budget();
        (
            u64::try_from(progress.remaining.as_millis()).unwrap_or(u64::MAX),
            u64::try_from(budget.terminal_time_reserve.as_millis()).unwrap_or(u64::MAX),
        )
    });
    let frontier_candidate = match anytime_controller.decision(remaining_ms, terminal_reserve_ms) {
        AnytimeDecision::Commit { candidate_id } => Some(candidate_id),
        AnytimeDecision::Continue | AnytimeDecision::Wait | AnytimeDecision::Exhausted => {
            anytime_controller
                .best()
                .map(|best| best.candidate_id.clone())
        }
    };
    let team_frontier_candidate_id = uplift_repair
        .as_ref()
        .map_or(final_step_id.as_str(), |repair| {
            repair.candidate_id.as_str()
        });
    let uplift_decision = adaptive_uplift_selection_decision(
        anytime_controller,
        team_frontier_candidate_id,
        direct_anchor_output,
    );
    let anchor_available = workflow_checkpoint
        .anytime_outputs
        .get(DIRECT_ANCHOR_CANDIDATE_ID)
        .is_some_and(|output| !output.trim().is_empty());
    let (selected_candidate, guidance_admitted) = select_adaptive_guidance(
        uplift_decision.as_ref(),
        team_frontier_candidate_id,
        anchor_available,
        frontier_candidate,
    );
    let mut selected_candidate_id = final_step_id.clone();
    if let Some(candidate_id) = selected_candidate.as_deref() {
        if let Some(output) = workflow_checkpoint
            .anytime_outputs
            .get(candidate_id)
            .filter(|output| !output.trim().is_empty())
        {
            final_output = output.clone();
            selected_candidate_id = candidate_id.to_string();
        }
    }
    let apply_guidance = guidance_admitted && selected_candidate_id == team_frontier_candidate_id;
    let selected_verdict = anytime_controller.verdict(&selected_candidate_id).cloned();
    let (selected_quality_gate, selected_pairwise_comparison) = if let Some(repair) = uplift_repair
        .as_ref()
        .filter(|repair| repair.candidate_id == selected_candidate_id)
    {
        (
            repair.quality_gate.clone(),
            repair.pairwise_comparison.clone(),
        )
    } else if selected_candidate_id == final_step_id {
        (quality_gate.clone(), pairwise_comparison.clone())
    } else {
        (
            selected_verdict.as_ref().map_or_else(
                || quality_gate.clone(),
                |verdict| AdaptiveQualityGateResult {
                    output: String::new(),
                    score: f64::from(verdict.quality_bps) / 10_000.0,
                    safety_violations: verdict.safety_violations,
                    passed: verdict.verified && verdict.safety_violations == 0,
                    issues: Vec::new(),
                },
            ),
            None,
        )
    };
    let selected_candidate_kind = anytime_controller
        .candidate(&selected_candidate_id)
        .map(|candidate| candidate.kind);
    let selected_candidate_kind_label = selected_candidate_kind
        .map(|kind| match kind {
            AnytimeCandidateKind::DirectAnchor => "direct_anchor",
            AnytimeCandidateKind::Workflow => "workflow",
            AnytimeCandidateKind::Verification => "verification",
            AnytimeCandidateKind::Repair => "repair",
            AnytimeCandidateKind::Synthesis => "synthesis",
        })
        .unwrap_or("unknown")
        .to_string();
    let selected_verified = selected_verdict
        .as_ref()
        .is_some_and(|verdict| verdict.verified && verdict.safety_violations == 0);
    let selection_assessment = anytime_controller.selection_assessment(&selected_candidate_id);
    let native_effort_success = selection_assessment
        .as_ref()
        .is_some_and(|assessment| assessment.native_effort_success);
    let selected_is_final_synthesis = selected_candidate_id == final_step_id
        && workflow_checkpoint
            .plan
            .steps
            .last()
            .is_some_and(|step| step.contract.output_kind == WorkflowOutputKind::Synthesis);
    let selected_deliverable = selected_verified
        && (selected_is_final_synthesis
            || matches!(
                selected_candidate_kind,
                Some(AnytimeCandidateKind::DirectAnchor | AnytimeCandidateKind::Synthesis)
            ));
    if let Some(control) = cancellation {
        let quality = if selected_is_final_synthesis
            || selected_candidate_kind == Some(AnytimeCandidateKind::Synthesis)
        {
            ResultQuality::Synthesized
        } else if selected_verified {
            ResultQuality::Verified
        } else {
            ResultQuality::Grounded
        };
        control.record_best_known_result_at(
            run_context_steer_epoch(run_context),
            &format!("anytime_selected_{selected_candidate_kind_label}"),
            &final_output,
            quality,
            selected_verdict
                .as_ref()
                .map_or(evidence_count, |verdict| verdict.evidence_count),
            selected_verified,
            selected_deliverable,
        );
    }
    let completion_status = if native_effort_success {
        "completed"
    } else {
        "degraded"
    };
    let routing_learning_eligible = selected_verified && native_effort_success;
    let prompt_learning_eligible = selected_candidate_id == final_step_id
        && selected_verified
        && native_effort_success
        && selected_quality_gate.passed
        && selected_quality_gate.safety_violations == 0;
    if collaboration_steer_pending(cancellation) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            workflow_checkpoint,
            anytime_controller,
            anchor_supervisor,
            direct_anchor_verifier,
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let unfinished_candidate_ids = anytime_controller
        .snapshot()
        .candidates
        .into_iter()
        .filter(|candidate| candidate.id != selected_candidate_id)
        .filter(|candidate| {
            matches!(
                candidate.state,
                AnytimeCandidateState::Pending | AnytimeCandidateState::Running
            )
        })
        .map(|candidate| candidate.id)
        .collect::<Vec<_>>();
    for candidate_id in &unfinished_candidate_ids {
        anytime_controller.cancel(candidate_id)?;
    }
    cancel_anytime_background(anchor_supervisor, direct_anchor_verifier);
    persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
    let recovered_steps = workflow_checkpoint
        .steps
        .values()
        .filter(|step| step.attempts > 1)
        .count();
    let step_credits = workflow_checkpoint.assign_step_credits(selected_quality_gate.score);
    workflow_checkpoint.finalize(final_output.clone(), current_time_millis())?;
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        "Collaboration workflow checkpoint finalized",
        completion_status,
        Some(&final_step_id),
        workflow_checkpoint,
    )?;
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration workflow completed",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "workflow_schema".to_string(),
                        WORKFLOW_IR_SCHEMA.to_string(),
                    ),
                    ("status".to_string(), completion_status.to_string()),
                    (
                        "fallback_used".to_string(),
                        (!native_effort_success).to_string(),
                    ),
                    ("workflow_steps".to_string(), workflow_steps.to_string()),
                    ("workflow_layers".to_string(), layer_count.to_string()),
                    ("evidence_count".to_string(), evidence_count.to_string()),
                    ("recovered_steps".to_string(), recovered_steps.to_string()),
                    (
                        "role_aligned_steps".to_string(),
                        role_coverage.aligned_steps.to_string(),
                    ),
                    (
                        "role_total_steps".to_string(),
                        role_coverage.total_steps.to_string(),
                    ),
                    (
                        "independent_branch_models".to_string(),
                        role_coverage.independent_models.to_string(),
                    ),
                    (
                        "cross_reviewed".to_string(),
                        role_coverage.cross_reviewed.to_string(),
                    ),
                    (
                        "step_credits".to_string(),
                        serde_json::to_string(&step_credits).unwrap_or_else(|_| "[]".to_string()),
                    ),
                    (
                        "quality_pass".to_string(),
                        selected_quality_gate.passed.to_string(),
                    ),
                    (
                        "quality_score".to_string(),
                        format!("{:.3}", selected_quality_gate.score.clamp(0.0, 1.0)),
                    ),
                    (
                        "quality_issues".to_string(),
                        truncate_for_collaboration(
                            &selected_quality_gate.issues.join(" | "),
                            4_000,
                        ),
                    ),
                    (
                        "safety_violations".to_string(),
                        selected_quality_gate.safety_violations.to_string(),
                    ),
                    (
                        "anytime_selected_candidate".to_string(),
                        selected_candidate_id.clone(),
                    ),
                    (
                        "anytime_selected_kind".to_string(),
                        selected_candidate_kind_label,
                    ),
                    (
                        "anytime_selected_verified".to_string(),
                        selected_verified.to_string(),
                    ),
                    (
                        "anytime_guidance_applied".to_string(),
                        apply_guidance.to_string(),
                    ),
                    (
                        "anytime_native_effort_success".to_string(),
                        native_effort_success.to_string(),
                    ),
                    (
                        "anytime_degradation_reasons".to_string(),
                        selection_assessment
                            .as_ref()
                            .map(|assessment| assessment.degradation_reasons.join(" | "))
                            .unwrap_or_else(|| "selection assessment unavailable".to_string()),
                    ),
                    (
                        "anytime_distinct_contributions".to_string(),
                        selection_assessment
                            .as_ref()
                            .map(|assessment| assessment.distinct_contributions.to_string())
                            .unwrap_or_default(),
                    ),
                    (
                        "anytime_team_uplift_bps".to_string(),
                        selected_pairwise_comparison
                            .as_ref()
                            .map(|comparison| comparison.team_uplift_bps.to_string())
                            .unwrap_or_default(),
                    ),
                    (
                        "anytime_team_score_bps".to_string(),
                        selected_pairwise_comparison
                            .as_ref()
                            .map(|comparison| comparison.team_score_bps.to_string())
                            .unwrap_or_default(),
                    ),
                    (
                        "anytime_anchor_score_bps".to_string(),
                        selected_pairwise_comparison
                            .as_ref()
                            .map(|comparison| comparison.anchor_score_bps.to_string())
                            .unwrap_or_default(),
                    ),
                    (
                        "anytime_selected_quality_bps".to_string(),
                        selected_verdict
                            .as_ref()
                            .map(|verdict| verdict.quality_bps.to_string())
                            .unwrap_or_default(),
                    ),
                    (
                        "anytime_cancelled_candidates".to_string(),
                        unfinished_candidate_ids.len().to_string(),
                    ),
                    (
                        "anytime_routing_learning_eligible".to_string(),
                        routing_learning_eligible.to_string(),
                    ),
                    (
                        "anytime_prompt_learning_eligible".to_string(),
                        prompt_learning_eligible.to_string(),
                    ),
                    (
                        crate::prompt_evolution_transfer_outbox::AUTO_TRANSFER_REQUIRED_KEY
                            .to_string(),
                        (config.prompt_evolution_enabled && effort == "auto").to_string(),
                    ),
                    (
                        "latency_ms".to_string(),
                        current_time_millis()
                            .saturating_sub(workflow_started_at_ms)
                            .to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    if apply_guidance {
        AdaptiveCollaborationOutcome::from_checkpoint(final_output, workflow_checkpoint)
    } else {
        Ok(AdaptiveCollaborationOutcome::foreground_direct())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_accepted_team_candidate_can_be_applied_as_guidance() {
        let accepted = select_adaptive_guidance(
            Some(&UpliftGateDecision::AcceptTeam),
            "team",
            true,
            Some("frontier".to_string()),
        );
        assert_eq!(accepted.0.as_deref(), Some("team"));
        assert!(accepted.1);

        for decision in [
            None,
            Some(&UpliftGateDecision::SelectAnchor { gaps: Vec::new() }),
            Some(&UpliftGateDecision::RepairTeam { gaps: Vec::new() }),
            Some(&UpliftGateDecision::ReturnBestKnown { gaps: Vec::new() }),
        ] {
            let rejected = select_adaptive_guidance(
                decision,
                "team",
                true,
                Some("frontier".to_string()),
            );
            assert_eq!(rejected.0.as_deref(), Some(DIRECT_ANCHOR_CANDIDATE_ID));
            assert!(!rejected.1);
        }
    }

    #[test]
    fn missing_anchor_still_fails_closed_to_foreground_direct() {
        let selection = select_adaptive_guidance(
            Some(&UpliftGateDecision::ReturnBestKnown { gaps: Vec::new() }),
            "team",
            false,
            Some("frontier".to_string()),
        );
        assert_eq!(selection.0.as_deref(), Some("frontier"));
        assert!(!selection.1);
    }
}
