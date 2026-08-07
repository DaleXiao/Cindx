use super::*;
use crate::collaboration_service::workflow_contract_coverage;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_adaptive_collaboration(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    collaboration_id: &str,
    prompt: &str,
    history: &[Message],
    models: &[String],
    agent_budget: usize,
) -> Result<AdaptiveCollaborationOutcome, String> {
    let AdaptiveCollaborationSetup {
        workflow_started_at_ms,
        conductor_model,
        role_hints,
        effort,
        policy,
        execution_contract,
        resume_key,
        mut workflow_checkpoint,
        resumed_from_workflow_id,
        resumed_from_checkpoint,
        prior,
        selection_mode,
        prompt_genome,
        prompt_genome_json,
        shared_memory,
        anchor_spec,
        cancellation,
    } = prepare_adaptive_collaboration(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        history,
        models,
        agent_budget,
    )?;
    let anchor_outcome = start_adaptive_anchor(AdaptiveAnchorContext {
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        effort: &effort,
        resumed_from_checkpoint,
        execution_contract: &execution_contract,
        prompt_genome: &prompt_genome,
        anchor_spec: &anchor_spec,
        checkpoint: workflow_checkpoint.as_ref(),
        cancellation: cancellation.clone(),
    })?;
    let AdaptiveAnchorState {
        output: mut direct_anchor_output,
        supervisor: mut anchor_supervisor,
        attempted: direct_anchor_attempted,
    } = match anchor_outcome {
        AdaptiveAnchorOutcome::Continue(state) => state,
        AdaptiveAnchorOutcome::Commit(output) => {
            return Ok(AdaptiveCollaborationOutcome::direct(output));
        }
    };
    let conductor_outcome = plan_adaptive_workflow(AdaptiveConductorContext {
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        models,
        agent_budget,
        shared_memory: &shared_memory,
        effort: &effort,
        policy: &policy,
        conductor_model: &conductor_model,
        role_hints: &role_hints,
        execution_contract: &execution_contract,
        prior: prior.as_ref(),
        prompt_genome: &prompt_genome,
        checkpoint: workflow_checkpoint.as_ref(),
        resume_key: &resume_key,
        resumed_from_workflow_id: resumed_from_workflow_id.as_deref(),
        cancellation: cancellation.as_ref(),
        anchor_spec: &anchor_spec,
        anchor_supervisor: &mut anchor_supervisor,
        direct_anchor_output: &mut direct_anchor_output,
    })?;
    let (workflow_plan, conductor_attempts) = match conductor_outcome {
        AdaptiveConductorOutcome::Plan {
            workflow_plan,
            attempts,
        } => (*workflow_plan, attempts),
        AdaptiveConductorOutcome::DirectCommit(output) => {
            return Ok(AdaptiveCollaborationOutcome::direct(output));
        }
    };
    let workflow = workflow_plan.adaptive_workflow();
    let layers = adaptive_workflow_layers(&workflow)?;
    let role_coverage = workflow_contract_coverage(&workflow_plan, &role_hints);
    let layer_count = layers.len();
    let mut workflow_checkpoint = workflow_checkpoint.take().unwrap_or_else(|| {
        WorkflowExecutionCheckpoint::new(
            resume_key.clone(),
            workflow_plan.clone(),
            workflow_started_at_ms,
        )
    });
    workflow_checkpoint.plan = workflow_plan.clone();
    workflow_checkpoint.prompt_genome_json = prompt_genome_json.clone();
    workflow_checkpoint.validate(models)?;
    let mut anytime_controller =
        initialize_anytime_controller(&execution_contract, &workflow, &workflow_checkpoint)?;
    let mut direct_anchor_verifier = None;
    let mut direct_anchor_verifier_attempted = false;
    if let Some(output) = direct_anchor_output.as_ref() {
        if anytime_controller
            .candidate(DIRECT_ANCHOR_CANDIDATE_ID)
            .is_some_and(|candidate| {
                matches!(
                    candidate.state,
                    AnytimeCandidateState::Pending | AnytimeCandidateState::Running
                )
            })
        {
            controller_mark_running_if_pending(
                &mut anytime_controller,
                DIRECT_ANCHOR_CANDIDATE_ID,
            )?;
            anytime_controller.observe(
                DIRECT_ANCHOR_CANDIDATE_ID,
                direct_anchor_verdict(prompt_genome.verification, Some(output)),
            )?;
        }
        workflow_checkpoint
            .anytime_outputs
            .insert(DIRECT_ANCHOR_CANDIDATE_ID.to_string(), output.clone());
    } else if anchor_supervisor.is_some() {
        controller_mark_running_if_pending(&mut anytime_controller, DIRECT_ANCHOR_CANDIDATE_ID)?;
    } else if direct_anchor_attempted {
        controller_mark_running_if_pending(&mut anytime_controller, DIRECT_ANCHOR_CANDIDATE_ID)?;
        anytime_controller.fail(DIRECT_ANCHOR_CANDIDATE_ID)?;
    }
    persist_anytime_controller(&mut workflow_checkpoint, &anytime_controller)?;
    record_adaptive_workflow_planned(AdaptiveWorkflowObservabilityContext {
        state,
        task_id,
        run_context,
        collaboration_id,
        workflow_plan: &workflow_plan,
        workflow: &workflow,
        layer_count,
        role_coverage: &role_coverage,
        resume_key: &resume_key,
        resumed_from_checkpoint,
        prompt_genome: &prompt_genome,
        prompt_genome_json: &prompt_genome_json,
        effort: &effort,
        selection_mode: &selection_mode,
        conductor_model: &conductor_model,
        execution_contract: &execution_contract,
        conductor_attempts,
        prior: prior.as_ref(),
        agent_budget,
        workflow_checkpoint: &workflow_checkpoint,
    })?;
    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            &mut workflow_checkpoint,
            &anytime_controller,
            anchor_supervisor.as_ref(),
            direct_anchor_verifier.as_ref(),
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    advance_direct_anchor_background(
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        prompt,
        prompt_genome.verification,
        cancellation.as_ref(),
        &anchor_spec,
        &mut anchor_supervisor,
        &mut direct_anchor_output,
        &mut direct_anchor_verifier,
        &mut direct_anchor_verifier_attempted,
        &mut anytime_controller,
        &mut workflow_checkpoint,
    )?;
    let frontier_outcome = run_adaptive_frontier(AdaptiveFrontierContext {
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        prompt,
        models,
        shared_memory: &shared_memory,
        prompt_genome: &prompt_genome,
        execution_contract: &execution_contract,
        workflow_started_at_ms,
        cancellation: cancellation.clone(),
        anchor_spec: &anchor_spec,
        workflow_checkpoint,
        anytime_controller,
        anchor_supervisor,
        direct_anchor_output,
        direct_anchor_verifier,
        direct_anchor_verifier_attempted,
    })?;
    let frontier_state = match frontier_outcome {
        AdaptiveFrontierOutcome::Continue(state) => *state,
        AdaptiveFrontierOutcome::Commit(outcome) => return Ok(outcome),
    };
    let AdaptiveFrontierState {
        mut workflow_checkpoint,
        mut anytime_controller,
        mut anchor_supervisor,
        mut direct_anchor_output,
        direct_anchor_verifier,
        mut outputs,
        evidence_by_step,
    } = frontier_state;

    if direct_anchor_output.is_none() {
        if let Some(completion) = anchor_supervisor
            .as_mut()
            .and_then(ParallelJobSupervisor::try_recv)
        {
            let completion = parallel_completion_or_failure(completion);
            direct_anchor_output = settle_direct_anchor_candidate(
                state,
                task_id,
                run_context,
                collaboration_id,
                &anchor_spec,
                &completion,
                prompt_genome.verification,
                cancellation.as_ref(),
                &mut anytime_controller,
                &mut workflow_checkpoint,
            )?;
        }
    }
    let final_plan = workflow_checkpoint.plan.clone();
    let final_workflow = final_plan.adaptive_workflow();
    let final_step_id = final_plan
        .steps
        .last()
        .map(|step| step.id.clone())
        .ok_or_else(|| "adaptive workflow has no final step".to_string())?;
    let final_layer_count = adaptive_workflow_layers(&final_workflow)?.len();
    let final_role_coverage = workflow_contract_coverage(&final_plan, &role_hints);
    let final_output = if let Some(output) = outputs.remove(&final_step_id) {
        output
    } else {
        if direct_anchor_output.is_none() {
            if let Some(completion) = anchor_supervisor
                .as_mut()
                .and_then(|supervisor| supervisor.recv_timeout(Duration::from_millis(500)))
            {
                let completion = parallel_completion_or_failure(completion);
                let _ = settle_direct_anchor_candidate(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    &anchor_spec,
                    &completion,
                    prompt_genome.verification,
                    cancellation.as_ref(),
                    &mut anytime_controller,
                    &mut workflow_checkpoint,
                )?;
            }
        }
        if let Some((_candidate_id, output, _verdict)) =
            anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
        {
            return AdaptiveCollaborationOutcome::from_checkpoint(output, &workflow_checkpoint);
        }
        return Err("adaptive workflow final output is missing".to_string());
    };
    let evidence_count = evidence_by_step
        .values()
        .flatten()
        .map(|evidence| evidence.tool_call_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let quality_gate = quality_gate_adaptive_output(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        &final_output,
        prompt_genome.verification,
        evidence_count,
    );
    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            &mut workflow_checkpoint,
            &anytime_controller,
            anchor_supervisor.as_ref(),
            direct_anchor_verifier.as_ref(),
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let final_output = match adaptive_quality_handoff(&quality_gate) {
        Ok(output) => output,
        Err(error) => {
            controller_mark_running_if_pending(&mut anytime_controller, &final_step_id)?;
            if anytime_controller
                .candidate(&final_step_id)
                .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Running)
            {
                anytime_controller.observe(
                    &final_step_id,
                    AnytimeVerdict {
                        quality_bps: 0,
                        confidence_bps: 0,
                        constraint_coverage_bps: 0,
                        evidence_count,
                        safety_violations: quality_gate.safety_violations,
                        deliverable: false,
                        verified: false,
                        anchor_uplift_bps: None,
                    },
                )?;
            }
            persist_anytime_controller(&mut workflow_checkpoint, &anytime_controller)?;
            append_workflow_checkpoint_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                "Collaboration workflow blocked by quality gate",
                "paused",
                Some(&final_step_id),
                &workflow_checkpoint,
            )?;
            if let Some((candidate_id, output, verdict)) =
                anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
            {
                if let Some(control) = cancellation.as_ref() {
                    control.record_best_known_result_at(
                        run_context_steer_epoch(run_context),
                        &format!("anytime_quality_rejection:{candidate_id}"),
                        &output,
                        if verdict.verified {
                            ResultQuality::Verified
                        } else {
                            ResultQuality::Grounded
                        },
                        verdict.evidence_count,
                        verdict.verified,
                        false,
                    );
                }
                return AdaptiveCollaborationOutcome::from_checkpoint(output, &workflow_checkpoint);
            }
            return Err(error);
        }
    };
    if direct_anchor_output.is_none() {
        if let Some(completion) = anchor_supervisor
            .as_mut()
            .and_then(|supervisor| supervisor.recv_timeout(Duration::from_millis(750)))
        {
            let completion = parallel_completion_or_failure(completion);
            direct_anchor_output = settle_direct_anchor_candidate(
                state,
                task_id,
                run_context,
                collaboration_id,
                &anchor_spec,
                &completion,
                prompt_genome.verification,
                cancellation.as_ref(),
                &mut anytime_controller,
                &mut workflow_checkpoint,
            )?;
        }
    }
    let guidance = finalize_adaptive_collaboration(AdaptiveCollaborationFinalization {
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
        workflow_steps: final_plan.steps.len(),
        layer_count: final_layer_count,
        role_coverage: final_role_coverage,
        evidence_count,
        quality_gate,
        final_output,
        direct_anchor_output: direct_anchor_output.as_deref(),
        cancellation: cancellation.as_ref(),
        anchor_supervisor: anchor_supervisor.as_ref(),
        direct_anchor_verifier: direct_anchor_verifier.as_ref(),
        anytime_controller: &mut anytime_controller,
        workflow_checkpoint: &mut workflow_checkpoint,
    })?;
    AdaptiveCollaborationOutcome::from_checkpoint(guidance, &workflow_checkpoint)
}
