use super::*;

const TARGETED_UPLIFT_REPAIR_SUFFIX: &str = "uplift-repair";

pub(super) struct AdaptiveUpliftRepairContext<'a, 'state> {
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) verification: PromptVerification,
    pub(super) evidence_count: usize,
    pub(super) team_candidate_id: &'a str,
    pub(super) team_output: &'a str,
    pub(super) anchor_output: Option<&'a str>,
    pub(super) cancellation: Option<&'a Arc<AgentRunControl>>,
    pub(super) anytime_controller: &'a mut AnytimeController,
    pub(super) workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
}

#[derive(Debug, Clone)]
pub(super) struct AdaptiveUpliftRepairResult {
    pub(super) candidate_id: String,
    pub(super) quality_gate: AdaptiveQualityGateResult,
    pub(super) pairwise_comparison: Option<AdaptivePairwiseComparison>,
}

fn repair_attempts_remaining(control: Option<&Arc<AgentRunControl>>) -> usize {
    control.map_or(1, |control| {
        control
            .budget()
            .max_repair_attempts
            .saturating_sub(control.progress().repair_attempts)
    })
}

fn anchor_is_eligible(controller: &AnytimeController, verification_required: bool) -> bool {
    controller
        .verdict(DIRECT_ANCHOR_CANDIDATE_ID)
        .is_some_and(|verdict| {
            verdict.deliverable
                && verdict.safety_violations == 0
                && (!verification_required || verdict.verified)
        })
}

fn uplift_gate_input(
    controller: &AnytimeController,
    team_candidate_id: &str,
    anchor_output: Option<&str>,
) -> Option<UpliftGateInput> {
    let snapshot = controller.snapshot();
    let candidate = controller.candidate(team_candidate_id)?;
    let verdict = controller.verdict(team_candidate_id)?;
    let assessment = controller.selection_assessment(team_candidate_id)?;
    let requires_anchor_comparison =
        anchor_output.is_some() || snapshot.config.min_team_uplift_bps > 0;
    let only_missing_anchor = anchor_output.is_none()
        && verdict.deliverable
        && verdict.safety_violations == 0
        && (!snapshot.config.verification_required || verdict.verified)
        && (!snapshot.config.requires_synthesis
            || candidate.kind == AnytimeCandidateKind::Synthesis)
        && assessment.distinct_contributions >= snapshot.config.min_distinct_contributions;
    Some(UpliftGateInput {
        team_candidate_kind: candidate.kind,
        team_deliverable: verdict.deliverable,
        team_verified: !snapshot.config.verification_required || verdict.verified,
        team_safety_violations: verdict.safety_violations,
        distinct_contributions: assessment.distinct_contributions,
        required_distinct_contributions: snapshot.config.min_distinct_contributions,
        requires_synthesis: snapshot.config.requires_synthesis,
        requires_anchor_comparison,
        paired_uplift_bps: verdict.anchor_uplift_bps,
        required_uplift_bps: snapshot.config.min_team_uplift_bps,
        repair_attempts_remaining: if only_missing_anchor { 0 } else { 1 },
        anchor_eligible: anchor_is_eligible(controller, snapshot.config.verification_required),
    })
}

pub(super) fn adaptive_uplift_selection_decision(
    controller: &AnytimeController,
    team_candidate_id: &str,
    anchor_output: Option<&str>,
) -> Option<UpliftGateDecision> {
    let mut input = uplift_gate_input(controller, team_candidate_id, anchor_output)?;
    input.repair_attempts_remaining = 0;
    Some(decide_uplift_gate(&input))
}

fn gap_labels(gaps: &[UpliftGap]) -> String {
    gaps.iter()
        .map(|gap| gap.label())
        .collect::<Vec<_>>()
        .join(", ")
}

struct UpliftRepairEvent<'a> {
    summary: &'a str,
    status: &'a str,
    gaps: &'a [UpliftGap],
    error: Option<&'a str>,
}

fn record_uplift_repair_event(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    event: UpliftRepairEvent<'_>,
) {
    let UpliftRepairEvent {
        summary,
        status,
        gaps,
        error,
    } = event;
    let mut metadata = [
        ("collaboration_id".to_string(), collaboration_id.to_string()),
        ("uplift_repair_status".to_string(), status.to_string()),
        ("uplift_gaps".to_string(), gap_labels(gaps)),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(error) = error {
        metadata.insert(
            "uplift_repair_error".to_string(),
            truncate_for_collaboration(error, 2_000),
        );
    }
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            summary,
            metadata_with_context(metadata, run_context),
        );
    }
}

fn restored_uplift_repair(
    candidate_id: &str,
    controller: &AnytimeController,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> Option<AdaptiveUpliftRepairResult> {
    checkpoint.anytime_outputs.get(candidate_id)?;
    let verdict = controller.verdict(candidate_id)?;
    Some(AdaptiveUpliftRepairResult {
        candidate_id: candidate_id.to_string(),
        quality_gate: AdaptiveQualityGateResult {
            output: String::new(),
            score: f64::from(verdict.quality_bps) / 10_000.0,
            safety_violations: verdict.safety_violations,
            passed: verdict.verified && verdict.safety_violations == 0,
            issues: Vec::new(),
        },
        pairwise_comparison: verdict
            .anchor_uplift_bps
            .map(|uplift| AdaptivePairwiseComparison {
                team_score_bps: verdict.quality_bps,
                anchor_score_bps: i32::from(verdict.quality_bps)
                    .saturating_sub(i32::from(uplift))
                    .clamp(0, 10_000) as u16,
                team_uplift_bps: uplift,
                team_safety_violations: verdict.safety_violations,
                anchor_safety_violations: 0,
                team_unsupported_claims: 0,
                anchor_unsupported_claims: 0,
                team_unmet_requirements: 0,
                anchor_unmet_requirements: 0,
            }),
    })
}

pub(super) fn repair_adaptive_uplift(
    context: AdaptiveUpliftRepairContext<'_, '_>,
) -> Result<Option<AdaptiveUpliftRepairResult>, String> {
    let AdaptiveUpliftRepairContext {
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        verification,
        evidence_count,
        team_candidate_id,
        team_output,
        anchor_output,
        cancellation,
        anytime_controller,
        workflow_checkpoint,
    } = context;
    let repair_candidate_id = format!("{team_candidate_id}:{TARGETED_UPLIFT_REPAIR_SUFFIX}");
    if anytime_controller.candidate(&repair_candidate_id).is_some() {
        return Ok(restored_uplift_repair(
            &repair_candidate_id,
            anytime_controller,
            workflow_checkpoint,
        ));
    }

    let Some(mut gate_input) =
        uplift_gate_input(anytime_controller, team_candidate_id, anchor_output)
    else {
        return Ok(None);
    };
    gate_input.repair_attempts_remaining = gate_input
        .repair_attempts_remaining
        .min(repair_attempts_remaining(cancellation));
    let UpliftGateDecision::RepairTeam { gaps } = decide_uplift_gate(&gate_input) else {
        return Ok(None);
    };
    let repair_budget_exhausted = if let Some(control) = cancellation {
        if control.stage_should_stop(RunStageClass::Repair) {
            true
        } else {
            match control.begin_repair_attempt_at(
                run_context_steer_epoch(run_context),
                "targeted_uplift_repair",
            ) {
                Ok(Some(_)) => false,
                Ok(None) => return Ok(None),
                Err(_) => true,
            }
        }
    } else {
        false
    };
    if repair_budget_exhausted {
        record_uplift_repair_event(
            state,
            task_id,
            run_context,
            collaboration_id,
            UpliftRepairEvent {
                summary: "Collaboration targeted uplift repair skipped",
                status: "budget_exhausted",
                gaps: &gaps,
                error: None,
            },
        );
        return Ok(None);
    }

    let expected_uplift = anytime_controller
        .candidate(team_candidate_id)
        .map_or(5_000, |candidate| candidate.expected_uplift_bps)
        .saturating_add(500)
        .min(10_000);
    anytime_controller.register(
        AnytimeCandidate::workflow(
            &repair_candidate_id,
            vec![team_candidate_id.to_string()],
            expected_uplift,
        )
        .with_kind(AnytimeCandidateKind::Synthesis)
        .with_contribution_signature("targeted_uplift_repair"),
    )?;
    anytime_controller.mark_running(&repair_candidate_id)?;
    persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
    record_uplift_repair_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        UpliftRepairEvent {
            summary: "Collaboration targeted uplift repair started",
            status: "running",
            gaps: &gaps,
            error: None,
        },
    );

    let anchor_section = anchor_output
        .map(|anchor| truncate_for_collaboration(anchor, 14_000))
        .unwrap_or_else(|| "[direct anchor unavailable]".to_string());
    let repair_model = config.model_for_role(&ModelRole::Summarizer);
    let repaired = match run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        "targeted_uplift_repair",
        ModelRole::Summarizer,
        &repair_model,
        format!(
            "Produce a repaired internal execution brief that is demonstrably better than the single-model anchor. Close exactly these failed collaboration-contract dimensions: {}. Preserve provenance-bearing evidence and verified constraints from the team result. Add only useful, non-duplicative contributions from the independent branches. Resolve contradictions, cover weaknesses in the anchor, and return one coherent synthesis for a separate tool-using executor. Do not answer the user directly and do not mention this comparison.\n\nUser objective:\n{}\n\nCurrent team synthesis:\n{}\n\nSingle-model anchor:\n{}",
            gap_labels(&gaps),
            truncate_for_collaboration(prompt, 12_000),
            truncate_for_collaboration(team_output, 14_000),
            anchor_section,
        ),
    ) {
        Ok(output) if !output.trim().is_empty() => output,
        Ok(_) => {
            anytime_controller.fail(&repair_candidate_id)?;
            persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
            record_uplift_repair_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                UpliftRepairEvent {
                    summary: "Collaboration targeted uplift repair failed",
                    status: "failed",
                    gaps: &gaps,
                    error: Some("repair model returned empty guidance"),
                },
            );
            return Ok(None);
        }
        Err(error) => {
            anytime_controller.fail(&repair_candidate_id)?;
            persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
            record_uplift_repair_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                UpliftRepairEvent {
                    summary: "Collaboration targeted uplift repair failed",
                    status: "failed",
                    gaps: &gaps,
                    error: Some(&error),
                },
            );
            if error == COLLABORATION_STEER_INTERRUPTED {
                return Err(error);
            }
            return Ok(None);
        }
    };
    let quality_gate = quality_gate_adaptive_output(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        &repaired,
        verification,
        evidence_count,
    );
    let repaired = quality_gate.output.clone();
    let pairwise_comparison = match anchor_output {
        Some(anchor) => match compare_team_guidance_with_anchor(
            state,
            config,
            task_id,
            run_context,
            collaboration_id,
            prompt,
            &repaired,
            anchor,
        ) {
            Ok(comparison) => Some(comparison),
            Err(error) => {
                record_uplift_repair_event(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    UpliftRepairEvent {
                        summary: "Collaboration repaired uplift comparison unavailable",
                        status: "comparison_unavailable",
                        gaps: &gaps,
                        error: Some(&error),
                    },
                );
                None
            }
        },
        None => None,
    };
    anytime_controller.observe(
        &repair_candidate_id,
        AnytimeVerdict {
            quality_bps: (quality_gate.score.clamp(0.0, 1.0) * 10_000.0).round() as u16,
            confidence_bps: if quality_gate.passed { 8_500 } else { 5_000 },
            constraint_coverage_bps: if quality_gate.passed { 9_000 } else { 6_000 },
            evidence_count,
            safety_violations: quality_gate.safety_violations.saturating_add(
                pairwise_comparison
                    .as_ref()
                    .map_or(0, |comparison| comparison.team_safety_violations),
            ),
            deliverable: !repaired.trim().is_empty(),
            verified: quality_gate.passed,
            anchor_uplift_bps: pairwise_comparison
                .as_ref()
                .map(|comparison| comparison.team_uplift_bps),
        },
    )?;
    workflow_checkpoint
        .anytime_outputs
        .insert(repair_candidate_id.clone(), repaired.clone());
    persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
    if let Some(control) = cancellation {
        control.record_best_known_result_at(
            run_context_steer_epoch(run_context),
            "anytime_targeted_uplift_repair",
            &repaired,
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
    record_uplift_repair_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        UpliftRepairEvent {
            summary: "Collaboration targeted uplift repair completed",
            status: if anytime_controller
                .selection_assessment(&repair_candidate_id)
                .is_some_and(|assessment| assessment.native_effort_success)
            {
                "verified_uplift"
            } else {
                "degraded"
            },
            gaps: &gaps,
            error: None,
        },
    );
    Ok(Some(AdaptiveUpliftRepairResult {
        candidate_id: repair_candidate_id,
        quality_gate,
        pairwise_comparison,
    }))
}
