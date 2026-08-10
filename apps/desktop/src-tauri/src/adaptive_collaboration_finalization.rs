use super::collaboration_service::WorkflowRoleCoverage;
use super::*;

pub(super) struct AdaptiveCollaborationFinalization<'a, 'state> {
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) effort: String,
    pub(super) workflow_started_at_ms: u64,
    pub(super) final_step_id: String,
    pub(super) workflow_steps: usize,
    pub(super) layer_count: usize,
    pub(super) role_coverage: WorkflowRoleCoverage,
    pub(super) evidence_count: usize,
    pub(super) verification_required: bool,
    pub(super) final_output: String,
    pub(super) cancellation: Option<&'a Arc<AgentRunControl>>,
    pub(super) anytime_controller: &'a mut AnytimeController,
    pub(super) workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
}

pub(crate) const fn owner_handoff_guidance_admitted(
    verification_required: bool,
    verification_satisfied: bool,
) -> bool {
    !verification_required || verification_satisfied
}

pub(super) fn finalize_adaptive_collaboration(
    context: AdaptiveCollaborationFinalization<'_, '_>,
) -> Result<AdaptiveCollaborationOutcome, String> {
    let AdaptiveCollaborationFinalization {
        state,
        task_id,
        run_context,
        collaboration_id,
        effort,
        workflow_started_at_ms,
        final_step_id,
        workflow_steps,
        layer_count,
        role_coverage,
        evidence_count,
        verification_required,
        final_output,
        cancellation,
        anytime_controller,
        workflow_checkpoint,
    } = context;

    if collaboration_steer_pending(cancellation) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            workflow_checkpoint,
            anytime_controller,
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }

    let verification_satisfied =
        workflow_checkpoint.workflow_verification_satisfied(verification_required);
    let guidance_admitted =
        owner_handoff_guidance_admitted(verification_required, verification_satisfied);
    if let Some(control) = cancellation {
        control.record_best_known_result_at(
            run_context_steer_epoch(run_context),
            "workflow_owner_handoff",
            &final_output,
            if verification_satisfied && verification_required {
                ResultQuality::Verified
            } else {
                ResultQuality::Grounded
            },
            evidence_count,
            verification_satisfied && verification_required,
            false,
        );
    }

    let unfinished_candidate_ids = anytime_controller
        .snapshot()
        .candidates
        .into_iter()
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
    persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
    workflow_checkpoint.finalize(final_output.clone(), current_time_millis())?;
    let completion_status = if guidance_admitted {
        "completed"
    } else {
        "degraded"
    };
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
                    (!guidance_admitted).to_string(),
                ),
                ("workflow_steps".to_string(), workflow_steps.to_string()),
                ("workflow_layers".to_string(), layer_count.to_string()),
                ("evidence_count".to_string(), evidence_count.to_string()),
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
                ("step_credits".to_string(), "[]".to_string()),
                ("quality_pass".to_string(), "false".to_string()),
                ("quality_score".to_string(), "0.000".to_string()),
                ("quality_issues".to_string(), String::new()),
                ("safety_violations".to_string(), "0".to_string()),
                (
                    "delivery_mode".to_string(),
                    "deterministic_owner_handoff".to_string(),
                ),
                ("model_call".to_string(), "false".to_string()),
                (
                    "workflow_verification_required".to_string(),
                    verification_required.to_string(),
                ),
                (
                    "workflow_verification_satisfied".to_string(),
                    verification_satisfied.to_string(),
                ),
                (
                    "anytime_selected_candidate".to_string(),
                    final_step_id.clone(),
                ),
                (
                    "anytime_selected_kind".to_string(),
                    "owner_handoff".to_string(),
                ),
                ("anytime_selected_verified".to_string(), "false".to_string()),
                (
                    "anytime_guidance_applied".to_string(),
                    guidance_admitted.to_string(),
                ),
                (
                    "anytime_native_effort_success".to_string(),
                    guidance_admitted.to_string(),
                ),
                (
                    "anytime_degradation_reasons".to_string(),
                    if guidance_admitted {
                        String::new()
                    } else {
                        "independent_verification_not_satisfied".to_string()
                    },
                ),
                (
                    "anytime_distinct_contributions".to_string(),
                    "1".to_string(),
                ),
                ("anytime_selected_quality_bps".to_string(), "0".to_string()),
                (
                    "anytime_cancelled_candidates".to_string(),
                    unfinished_candidate_ids.len().to_string(),
                ),
                (
                    "anytime_routing_learning_eligible".to_string(),
                    "false".to_string(),
                ),
                (
                    "anytime_prompt_learning_eligible".to_string(),
                    "false".to_string(),
                ),
                (
                    crate::prompt_evolution_transfer_outbox::AUTO_TRANSFER_REQUIRED_KEY.to_string(),
                    "false".to_string(),
                ),
                ("effort".to_string(), effort),
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
    drop(store);

    if guidance_admitted {
        AdaptiveCollaborationOutcome::from_checkpoint(final_output, workflow_checkpoint)
    } else {
        Ok(AdaptiveCollaborationOutcome::foreground_direct())
    }
}

#[cfg(test)]
mod tests {
    use super::owner_handoff_guidance_admitted;

    #[test]
    fn owner_handoff_requires_the_requested_independent_verification() {
        assert!(owner_handoff_guidance_admitted(false, false));
        assert!(owner_handoff_guidance_admitted(true, true));
        assert!(!owner_handoff_guidance_admitted(true, false));
    }
}
