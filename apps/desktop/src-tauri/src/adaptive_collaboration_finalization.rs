use super::collaboration_service::WorkflowRoleCoverage;
use super::*;

pub(super) struct AdaptiveCollaborationFinalization<'a, 'state> {
    pub(super) app: &'a tauri::AppHandle,
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) models: &'a [String],
    pub(super) agent_budget: usize,
    pub(super) effort: String,
    pub(super) policy: String,
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
) -> Result<String, String> {
    let AdaptiveCollaborationFinalization {
        app,
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        models,
        agent_budget,
        effort,
        policy,
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
                confidence_bps: if quality_gate.passed { 8_000 } else { 5_000 },
                constraint_coverage_bps: if quality_gate.passed { 8_500 } else { 6_000 },
                evidence_count,
                safety_violations: quality_gate.safety_violations.saturating_add(
                    pairwise_comparison
                        .as_ref()
                        .map_or(0, |comparison| comparison.team_safety_violations),
                ),
                deliverable: !final_output.trim().is_empty(),
                verified: quality_gate.passed,
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
        control.record_best_known_result(
            "anytime_workflow_final",
            &final_output,
            if quality_gate.passed {
                ResultQuality::Verified
            } else {
                ResultQuality::Grounded
            },
            evidence_count,
            quality_gate.passed,
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
    let selected_candidate = match adaptive_uplift_selection_decision(
        anytime_controller,
        team_frontier_candidate_id,
        direct_anchor_output,
    ) {
        Some(UpliftGateDecision::AcceptTeam) => Some(team_frontier_candidate_id.to_string()),
        Some(UpliftGateDecision::SelectAnchor { .. })
            if workflow_checkpoint
                .anytime_outputs
                .get(DIRECT_ANCHOR_CANDIDATE_ID)
                .is_some_and(|output| !output.trim().is_empty()) =>
        {
            Some(DIRECT_ANCHOR_CANDIDATE_ID.to_string())
        }
        Some(UpliftGateDecision::RepairTeam { .. })
        | Some(UpliftGateDecision::ReturnBestKnown { .. })
        | Some(UpliftGateDecision::SelectAnchor { .. })
        | None => frontier_candidate,
    };
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
        .map(|candidate| match candidate.kind {
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
                    ("anytime_selected_kind".to_string(), selected_candidate_kind),
                    (
                        "anytime_selected_verified".to_string(),
                        selected_verified.to_string(),
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
    if config.prompt_evolution_enabled && effort != "fast" {
        schedule_prompt_pairwise_evaluation(
            app.clone(),
            config.clone(),
            task_id.clone(),
            run_context.clone(),
            effort,
            policy,
            models.to_vec(),
            agent_budget,
            prompt_genome,
        );
    }
    Ok(final_output)
}
