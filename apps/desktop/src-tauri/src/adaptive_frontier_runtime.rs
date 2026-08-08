use super::*;

pub(super) enum AdaptiveFrontierOutcome {
    Continue(Box<AdaptiveFrontierState>),
    Commit(AdaptiveCollaborationOutcome),
}

pub(super) struct AdaptiveFrontierState {
    pub(super) workflow_checkpoint: WorkflowExecutionCheckpoint,
    pub(super) anytime_controller: AnytimeController,
    pub(super) anchor_supervisor: Option<ParallelJobSupervisor<CollaborationCompletion>>,
    pub(super) direct_anchor_output: Option<String>,
    pub(super) direct_anchor_verifier: Option<DirectAnchorVerifier>,
    pub(super) outputs: BTreeMap<String, String>,
    pub(super) evidence_by_step: BTreeMap<String, Vec<CollaborationEvidence>>,
}

pub(super) struct AdaptiveFrontierContext<'a, 'state> {
    pub(super) app: &'a tauri::AppHandle,
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) workspace_root: &'a Path,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) models: &'a [String],
    pub(super) shared_memory: &'a str,
    pub(super) prompt_genome: &'a ConductorPromptGenome,
    pub(super) execution_contract: &'a ConductorExecutionContract,
    pub(super) workflow_started_at_ms: u64,
    pub(super) cancellation: Option<Arc<AgentRunControl>>,
    pub(super) anchor_spec: &'a AdaptiveCollaborationSpec,
    pub(super) workflow_checkpoint: WorkflowExecutionCheckpoint,
    pub(super) anytime_controller: AnytimeController,
    pub(super) anchor_supervisor: Option<ParallelJobSupervisor<CollaborationCompletion>>,
    pub(super) direct_anchor_output: Option<String>,
    pub(super) direct_anchor_verifier: Option<DirectAnchorVerifier>,
    pub(super) direct_anchor_verifier_attempted: bool,
}

pub(super) fn run_adaptive_frontier(
    context: AdaptiveFrontierContext<'_, '_>,
) -> Result<AdaptiveFrontierOutcome, String> {
    let AdaptiveFrontierContext {
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        prompt,
        models,
        shared_memory,
        prompt_genome,
        execution_contract,
        workflow_started_at_ms,
        cancellation,
        anchor_spec,
        mut workflow_checkpoint,
        mut anytime_controller,
        mut anchor_supervisor,
        mut direct_anchor_output,
        mut direct_anchor_verifier,
        mut direct_anchor_verifier_attempted,
    } = context;
    let max_step_attempts =
        effective_workflow_step_attempt_budget(prompt_genome, &workflow_checkpoint);
    let mut outputs = workflow_checkpoint.completed_outputs();
    let mut evidence_by_step = checkpoint_evidence_by_step(&workflow_checkpoint);

    let mut frontier_round = 0usize;
    loop {
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
        let current_plan = workflow_checkpoint.plan.clone();
        let current_workflow = current_plan.adaptive_workflow();
        let current_layer_count = adaptive_workflow_layers(&current_workflow)?.len();
        let max_model_turns_per_step =
            effective_workflow_model_turn_budget(&current_plan, &workflow_checkpoint);
        let final_step_id = current_workflow
            .steps
            .last()
            .map(|step| step.id.clone())
            .ok_or_else(|| "adaptive workflow has no final step".to_string())?;
        let graph_frontier = workflow_checkpoint.execution_frontier(max_step_attempts)?;
        let graph_runnable = graph_frontier
            .runnable_steps()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let ready_ids = anytime_controller
            .ready_candidates()
            .into_iter()
            .filter(|candidate| candidate.id != DIRECT_ANCHOR_CANDIDATE_ID)
            .filter(|candidate| graph_runnable.contains(&candidate.id))
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let layer = ready_ids
            .iter()
            .filter_map(|candidate_id| {
                current_workflow
                    .steps
                    .iter()
                    .position(|step| &step.id == candidate_id)
            })
            .filter(|step_index| {
                workflow_checkpoint
                    .steps
                    .get(&current_workflow.steps[*step_index].id)
                    .is_some_and(|step| {
                        !matches!(
                            step.status,
                            WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
                        )
                    })
            })
            .collect::<Vec<_>>();
        if layer.is_empty() {
            if graph_frontier.is_complete(current_plan.steps.len()) {
                break;
            }
            while (direct_anchor_output.is_none()
                && anchor_supervisor
                    .as_ref()
                    .is_some_and(|supervisor| supervisor.pending() > 0))
                || direct_anchor_verifier.is_some()
            {
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
                    anchor_spec,
                    &mut anchor_supervisor,
                    &mut direct_anchor_output,
                    &mut direct_anchor_verifier,
                    &mut direct_anchor_verifier_attempted,
                    &mut anytime_controller,
                    &mut workflow_checkpoint,
                )?;
                if direct_anchor_should_commit(&anytime_controller, cancellation.as_ref()) {
                    let output = direct_anchor_output.clone().ok_or_else(|| {
                        "anytime controller committed a missing direct anchor".to_string()
                    })?;
                    return commit_adaptive_frontier(output, &workflow_checkpoint);
                }
                if cancellation.as_ref().is_some_and(agent_run_should_stop) {
                    return Err(MODEL_REQUEST_CANCELLED.to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Some((candidate_id, output, verdict)) =
                anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
            {
                if let Some(control) = cancellation.as_ref() {
                    control.record_best_known_result_at(
                        run_context_steer_epoch(run_context),
                        &format!("anytime_frontier_blocked:{candidate_id}"),
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
                return commit_adaptive_frontier(output, &workflow_checkpoint);
            }
            return Err(format!(
                "anytime workflow frontier is blocked without runnable candidates; exhausted=[{}] blocked=[{}]",
                graph_frontier.exhausted_steps.join(","),
                graph_frontier.blocked_steps.join(",")
            ));
        }
        let layer_index = frontier_round;
        frontier_round = frontier_round.saturating_add(1);
        let wave = prepare_adaptive_wave(AdaptiveWavePlanningContext {
            state,
            task_id,
            run_context,
            collaboration_id,
            prompt,
            shared_memory,
            workflow: &current_workflow,
            workflow_plan: &current_plan,
            execution_contract,
            layer,
            layer_index,
            layer_count: current_layer_count,
            max_step_attempts,
            max_model_turns_per_step,
            outputs: &outputs,
            workflow_checkpoint: &mut workflow_checkpoint,
            anytime_controller: &mut anytime_controller,
        })?;
        let wave_outcome = execute_adaptive_wave(AdaptiveWaveExecutionContext {
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            collaboration_id,
            prompt,
            prompt_genome,
            execution_contract,
            anchor_spec,
            wave: &wave,
            workflow_checkpoint: &mut workflow_checkpoint,
            anytime_controller: &mut anytime_controller,
            anchor_supervisor: &mut anchor_supervisor,
            direct_anchor_output: &mut direct_anchor_output,
            direct_anchor_verifier: &mut direct_anchor_verifier,
            direct_anchor_verifier_attempted: &mut direct_anchor_verifier_attempted,
        })?;
        let wave_completion = match wave_outcome {
            AdaptiveWaveOutcome::Completed(completion) => completion,
            AdaptiveWaveOutcome::Commit(output) => {
                return commit_adaptive_frontier(output, &workflow_checkpoint);
            }
        };
        let reconciliation = reconcile_adaptive_wave(AdaptiveWaveReconciliationContext {
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            collaboration_id,
            prompt,
            models,
            prompt_genome,
            execution_contract,
            anchor_spec,
            final_step_id: &final_step_id,
            workflow_started_at_ms,
            wave: &wave,
            completion: wave_completion,
            workflow_checkpoint: &mut workflow_checkpoint,
            anytime_controller: &mut anytime_controller,
            anchor_supervisor: &mut anchor_supervisor,
            direct_anchor_output: &mut direct_anchor_output,
            direct_anchor_verifier: &mut direct_anchor_verifier,
            direct_anchor_verifier_attempted: &mut direct_anchor_verifier_attempted,
            outputs: &mut outputs,
            evidence_by_step: &mut evidence_by_step,
        })?;
        if let AdaptiveWaveReconciliationOutcome::Commit(output) = reconciliation {
            return commit_adaptive_frontier(output, &workflow_checkpoint);
        }
    }

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

    Ok(AdaptiveFrontierOutcome::Continue(Box::new(
        AdaptiveFrontierState {
            workflow_checkpoint,
            anytime_controller,
            anchor_supervisor,
            direct_anchor_output,
            direct_anchor_verifier,
            outputs,
            evidence_by_step,
        },
    )))
}

fn commit_adaptive_frontier(
    _output: String,
    _checkpoint: &WorkflowExecutionCheckpoint,
) -> Result<AdaptiveFrontierOutcome, String> {
    Ok(AdaptiveFrontierOutcome::Commit(
        AdaptiveCollaborationOutcome::foreground_direct(),
    ))
}
