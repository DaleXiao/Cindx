use super::*;

pub(crate) const PARTIAL_HANDOFF_CANDIDATE_ID: &str = "__partial_handoff";

pub(super) fn initialize_anytime_controller(
    contract: &ConductorExecutionContract,
    _workflow: &orchestrator::AdaptiveWorkflow,
    checkpoint: &mut WorkflowExecutionCheckpoint,
) -> Result<AnytimeController, String> {
    let restored = AgentEngineSession::restore(contract, checkpoint.clone())?
        .into_parts()
        .1;
    let mut snapshot = restored.snapshot();
    snapshot
        .candidates
        .retain(|candidate| candidate.id != DIRECT_ANCHOR_CANDIDATE_ID);
    snapshot.verdicts.remove(DIRECT_ANCHOR_CANDIDATE_ID);
    if snapshot
        .best
        .as_ref()
        .is_some_and(|best| best.candidate_id == DIRECT_ANCHOR_CANDIDATE_ID)
    {
        snapshot.best = None;
    }
    checkpoint
        .anytime_outputs
        .remove(DIRECT_ANCHOR_CANDIDATE_ID);
    AnytimeController::from_snapshot(snapshot)
}

pub(super) fn persist_anytime_controller(
    checkpoint: &mut WorkflowExecutionCheckpoint,
    controller: &AnytimeController,
) -> Result<(), String> {
    orchestrator::persist_anytime_controller(checkpoint, controller)
}

pub(super) fn collaboration_steer_pending(cancellation: Option<&Arc<AgentRunControl>>) -> bool {
    cancellation.is_some_and(|control| control.has_pending_steer())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pause_anytime_for_steer(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    checkpoint: &mut WorkflowExecutionCheckpoint,
    controller: &AnytimeController,
) -> Result<(), String> {
    persist_anytime_controller(checkpoint, controller)?;
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        "Collaboration workflow paused for user steering",
        "steered",
        None,
        checkpoint,
    )
}

pub(crate) fn anytime_best_known_output(
    controller: &AnytimeController,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> Option<(String, String, AnytimeVerdict)> {
    let best = controller.best()?;
    let output = checkpoint
        .anytime_outputs
        .get(&best.candidate_id)
        .filter(|output| !output.trim().is_empty())?;
    Some((
        best.candidate_id.clone(),
        output.clone(),
        best.verdict.clone(),
    ))
}

pub(crate) fn register_partial_handoff_candidate(
    controller: &mut AnytimeController,
    checkpoint: &mut WorkflowExecutionCheckpoint,
    handoff: &str,
    completed_steps: usize,
    failed_steps: usize,
    evidence_count: usize,
) -> Result<(), String> {
    if handoff.trim().is_empty() {
        return Ok(());
    }
    if controller.candidate(PARTIAL_HANDOFF_CANDIDATE_ID).is_none() {
        controller.register(AnytimeCandidate {
            id: PARTIAL_HANDOFF_CANDIDATE_ID.to_string(),
            kind: AnytimeCandidateKind::Synthesis,
            contribution_signature: None,
            commit_eligible: true,
            dependencies: Vec::new(),
            expected_uplift_bps: 6_000,
            uncertainty_bps: 3_000,
            evidence_gap_bps: 3_000,
            latency_risk_bps: 0,
            failure_risk_bps: 1_000,
            state: AnytimeCandidateState::Pending,
        })?;
    }
    controller_mark_running_if_pending(controller, PARTIAL_HANDOFF_CANDIDATE_ID)?;
    if controller
        .candidate(PARTIAL_HANDOFF_CANDIDATE_ID)
        .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Running)
    {
        let total_steps = completed_steps.saturating_add(failed_steps).max(1);
        let completion_bps = u16::try_from(
            completed_steps
                .saturating_mul(10_000)
                .saturating_div(total_steps),
        )
        .unwrap_or(10_000)
        .min(10_000);
        let quality_bps = 4_500u16.saturating_add(
            u16::try_from(u32::from(completion_bps).saturating_mul(2_500) / 10_000)
                .unwrap_or(2_500),
        );
        let coverage_bps = 4_500u16.saturating_add(
            u16::try_from(u32::from(completion_bps).saturating_mul(3_000) / 10_000)
                .unwrap_or(3_000),
        );
        controller.observe(
            PARTIAL_HANDOFF_CANDIDATE_ID,
            AnytimeVerdict {
                quality_bps,
                confidence_bps: 4_500u16
                    .saturating_add(u16::try_from(evidence_count.min(10) * 200).unwrap_or(2_000)),
                constraint_coverage_bps: coverage_bps,
                evidence_count,
                safety_violations: 0,
                deliverable: true,
                verified: false,
                anchor_uplift_bps: None,
            },
        )?;
    }
    checkpoint.anytime_outputs.insert(
        PARTIAL_HANDOFF_CANDIDATE_ID.to_string(),
        handoff.to_string(),
    );
    persist_anytime_controller(checkpoint, controller)
}

pub(super) fn controller_mark_running_if_pending(
    controller: &mut AnytimeController,
    candidate_id: &str,
) -> Result<(), String> {
    if controller
        .candidate(candidate_id)
        .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Pending)
    {
        controller.mark_running(candidate_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::{AdaptiveWorkflow, AdaptiveWorkflowStep};

    #[test]
    fn foreground_anytime_initialization_excludes_legacy_direct_anchor() {
        let plan = WorkflowPlanIr::from_adaptive_with_profile(
            "no-direct-anchor",
            "Inspect the workspace",
            "pro",
            "best_of_n",
            "planner",
            "seed-pro-v1",
            &AdaptiveWorkflow {
                steps: vec![AdaptiveWorkflowStep {
                    id: "inspect".to_string(),
                    role: "worker".to_string(),
                    model: "worker".to_string(),
                    subtask: "Inspect bounded evidence".to_string(),
                    access: Vec::new(),
                }],
            },
            WorkflowBudget {
                max_steps: 1,
                max_models: 1,
                max_model_turns_per_step: 1,
                max_tool_calls_per_step: 0,
                max_output_tokens_per_step: 1_024,
            },
        );
        let workflow = plan.adaptive_workflow();
        let mut checkpoint = WorkflowExecutionCheckpoint::new("no-direct-anchor", plan, 1);
        checkpoint.anytime_outputs.insert(
            DIRECT_ANCHOR_CANDIDATE_ID.to_string(),
            "legacy anchor".to_string(),
        );
        let contract = ConductorExecutionContract::from_routing(
            &RoutingContext::from_prompt("Inspect the workspace", Vec::new()),
            "pro",
            OrchestrationPolicy::AutoRouter,
        );

        let controller =
            initialize_anytime_controller(&contract, &workflow, &mut checkpoint).unwrap();

        assert!(controller.candidate(DIRECT_ANCHOR_CANDIDATE_ID).is_none());
        assert!(controller.candidate("inspect").is_some());
        assert!(!checkpoint
            .anytime_outputs
            .contains_key(DIRECT_ANCHOR_CANDIDATE_ID));
    }
}
