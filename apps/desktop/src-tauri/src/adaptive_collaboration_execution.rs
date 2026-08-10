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
        checkpoint_resumable,
        resumed_from_workflow_id,
        resumed_from_checkpoint,
        prior,
        route_workflow_proposal,
        selection_mode,
        prompt_genome,
        prompt_genome_json,
        shared_memory,
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
    let conductor_outcome = plan_adaptive_workflow(AdaptiveConductorContext {
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        models,
        shared_memory: &shared_memory,
        effort: &effort,
        policy: &policy,
        conductor_model: &conductor_model,
        role_hints: &role_hints,
        execution_contract: &execution_contract,
        prior: prior.as_ref(),
        route_workflow_proposal: route_workflow_proposal.as_ref(),
        prompt_genome: &prompt_genome,
        checkpoint: workflow_checkpoint.as_ref(),
        checkpoint_resumable,
        resume_key: &resume_key,
        resumed_from_workflow_id: resumed_from_workflow_id.as_deref(),
        cancellation: cancellation.as_ref(),
    })?;
    let (workflow_plan, conductor_attempts, workflow_plan_source) = match conductor_outcome {
        AdaptiveConductorOutcome::Plan {
            workflow_plan,
            attempts,
            source,
        } => (*workflow_plan, attempts, source),
        AdaptiveConductorOutcome::DirectCommit => {
            return adaptive_direct_commit_outcome(prompt, workflow_checkpoint.as_ref());
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
    let anytime_controller =
        initialize_anytime_controller(&execution_contract, &workflow, &mut workflow_checkpoint)?;
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
        workflow_plan_source,
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
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
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
        workflow_checkpoint,
        anytime_controller,
    })?;
    let frontier_state = match frontier_outcome {
        AdaptiveFrontierOutcome::Continue(state) => *state,
        AdaptiveFrontierOutcome::Commit(outcome) => return Ok(outcome),
    };
    let AdaptiveFrontierState {
        mut workflow_checkpoint,
        mut anytime_controller,
        mut outputs,
        evidence_by_step,
        ..
    } = frontier_state;
    let final_plan = workflow_checkpoint.plan.clone();
    let final_workflow = final_plan.adaptive_workflow();
    let final_step_id = adaptive_delivery_schedule(
        &workflow_checkpoint,
        effective_workflow_step_attempt_budget(&prompt_genome, &workflow_checkpoint),
    )?
    .target_step_id;
    let final_output = outputs
        .remove(&final_step_id)
        .ok_or_else(|| "adaptive workflow owner handoff is missing".to_string())?;
    let evidence_count = evidence_by_step
        .values()
        .flatten()
        .map(|evidence| evidence.tool_call_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    finalize_adaptive_collaboration(AdaptiveCollaborationFinalization {
        state,
        task_id,
        run_context,
        collaboration_id,
        effort,
        workflow_started_at_ms,
        final_step_id,
        workflow_steps: final_plan.steps.len(),
        layer_count: adaptive_workflow_layers(&final_workflow)?.len(),
        role_coverage: workflow_contract_coverage(&final_plan, &role_hints),
        evidence_count,
        verification_required: execution_contract.verification_required,
        final_output,
        cancellation: cancellation.as_ref(),
        anytime_controller: &mut anytime_controller,
        workflow_checkpoint: &mut workflow_checkpoint,
    })
}

fn adaptive_direct_commit_outcome(
    prompt: &str,
    checkpoint: Option<&WorkflowExecutionCheckpoint>,
) -> Result<AdaptiveCollaborationOutcome, String> {
    let Some(checkpoint) = checkpoint else {
        return Ok(AdaptiveCollaborationOutcome::foreground_direct());
    };
    let outputs = checkpoint.completed_outputs();
    let failures = [
        "The saved workflow checkpoint is not executable under the current Owner graph and model catalog; do not schedule its legacy steps."
            .to_string(),
    ];
    let Some(handoff) = adaptive_partial_work_handoff(prompt, &outputs, &failures) else {
        return Ok(AdaptiveCollaborationOutcome::foreground_direct());
    };
    adaptive_untrusted_partial_outcome(handoff, checkpoint)
}
