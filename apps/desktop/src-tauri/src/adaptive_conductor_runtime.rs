use super::*;

pub(super) enum AdaptiveConductorOutcome {
    Plan {
        workflow_plan: Box<WorkflowPlanIr>,
        attempts: usize,
        source: AdaptiveWorkflowPlanSource,
    },
    DirectCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdaptiveWorkflowPlanSource {
    CheckpointResume,
    RunDecisionProposal,
    ParetoSearchTeacher,
    ModelColdStart,
}

impl AdaptiveWorkflowPlanSource {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::CheckpointResume => "checkpoint_resume",
            Self::RunDecisionProposal => "run_decision_proposal",
            Self::ParetoSearchTeacher => "pareto_search_teacher_v2",
            Self::ModelColdStart => "model_cold_start",
        }
    }
}

pub(super) struct AdaptiveConductorContext<'a, 'state> {
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) models: &'a [String],
    pub(super) agent_budget: usize,
    pub(super) shared_memory: &'a str,
    pub(super) effort: &'a str,
    pub(super) policy: &'a str,
    pub(super) conductor_model: &'a str,
    pub(super) role_hints: &'a ConductorRoleHints,
    pub(super) execution_contract: &'a ConductorExecutionContract,
    pub(super) prior: Option<&'a WorkflowTopologyPrior>,
    pub(super) route_workflow_proposal: Option<&'a WorkflowPlanProposal>,
    pub(super) prompt_genome: &'a ConductorPromptGenome,
    pub(super) checkpoint: Option<&'a WorkflowExecutionCheckpoint>,
    pub(super) resume_key: &'a str,
    pub(super) resumed_from_workflow_id: Option<&'a str>,
    pub(super) cancellation: Option<&'a Arc<AgentRunControl>>,
    pub(super) anchor_spec: &'a AdaptiveCollaborationSpec,
    pub(super) anchor_supervisor: &'a mut Option<ParallelJobSupervisor<CollaborationCompletion>>,
    pub(super) direct_anchor_output: &'a mut Option<String>,
}

pub(super) fn plan_adaptive_workflow(
    context: AdaptiveConductorContext<'_, '_>,
) -> Result<AdaptiveConductorOutcome, String> {
    let AdaptiveConductorContext {
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        models,
        agent_budget,
        shared_memory,
        effort,
        policy,
        conductor_model,
        role_hints,
        execution_contract,
        prior,
        route_workflow_proposal,
        prompt_genome,
        checkpoint,
        resume_key,
        resumed_from_workflow_id,
        cancellation,
        anchor_spec,
        anchor_supervisor,
        direct_anchor_output,
    } = context;

    if let Some(checkpoint) = checkpoint {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration workflow resumed",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("workflow_resume_key".to_string(), resume_key.to_string()),
                    (
                        "resumed_from_workflow_id".to_string(),
                        resumed_from_workflow_id.unwrap_or_default().to_string(),
                    ),
                    (
                        "completed_steps".to_string(),
                        checkpoint.completed_step_count().to_string(),
                    ),
                    (
                        "workflow_steps".to_string(),
                        checkpoint.plan.steps.len().to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        return Ok(AdaptiveConductorOutcome::Plan {
            workflow_plan: Box::new(checkpoint.plan.clone()),
            attempts: 0,
            source: AdaptiveWorkflowPlanSource::CheckpointResume,
        });
    }

    let harness = ConductorHarness::new(ConductorRequest {
        workflow_id: collaboration_id.to_string(),
        objective: prompt.to_string(),
        recent_context: shared_memory.to_string(),
        effort: effort.to_string(),
        policy: policy.to_string(),
        conductor_model: conductor_model.to_string(),
        primary_model: run_context
            .get("agent_model")
            .filter(|model| models.contains(model))
            .cloned()
            .unwrap_or_else(|| role_hints.executor.clone()),
        worker_models: models.to_vec(),
        role_hints: role_hints.clone(),
        budget: WorkflowBudget {
            max_steps: adaptive_workflow_step_budget(agent_budget),
            max_models: agent_budget.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS),
            max_model_turns_per_step: DEFAULT_COLLABORATION_WORKER_TURNS,
            max_tool_calls_per_step: MAX_COLLABORATION_WORKER_TOOL_CALLS,
            max_output_tokens_per_step: COLLABORATION_MAX_OUTPUT_TOKENS as usize,
        },
        execution_contract: execution_contract.clone(),
        prior_hint: prior.map(WorkflowTopologyPrior::prompt_hint),
        prompt_evolution_enabled: config.prompt_evolution_enabled,
        prompt_genome: prompt_genome.clone(),
    });
    if let Some(proposal) = route_workflow_proposal {
        match harness.plan_from_proposal(proposal) {
            Ok(workflow_plan) => {
                return Ok(AdaptiveConductorOutcome::Plan {
                    workflow_plan: Box::new(workflow_plan),
                    attempts: 0,
                    source: AdaptiveWorkflowPlanSource::RunDecisionProposal,
                });
            }
            Err(error) => record_conductor_rejection(
                state,
                task_id,
                run_context,
                collaboration_id,
                0,
                &format!("run-decision workflow proposal rejected: {error}"),
            ),
        }
    }
    let conductor_response = run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        "conductor_plan",
        ModelRole::Planner,
        conductor_model,
        harness.planning_prompt(),
    );
    let mut conductor_response = match conductor_response {
        Ok(response) => response,
        Err(error) => {
            if error == COLLABORATION_STEER_INTERRUPTED || collaboration_steer_pending(cancellation)
            {
                cancel_anytime_background(anchor_supervisor.as_ref(), None);
                return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
            }
            if await_direct_anchor_fallback(
                state,
                task_id,
                run_context,
                collaboration_id,
                anchor_spec,
                prompt_genome.verification,
                cancellation,
                anchor_supervisor,
                direct_anchor_output,
                Duration::from_millis(500),
            )?
            .is_some()
            {
                return Ok(AdaptiveConductorOutcome::DirectCommit);
            }
            return Err(error);
        }
    };

    let mut attempts = 1usize;
    loop {
        if collaboration_steer_pending(cancellation) {
            cancel_anytime_background(anchor_supervisor.as_ref(), None);
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        match harness.parse_plan(&conductor_response) {
            Ok(workflow_plan) => {
                return Ok(AdaptiveConductorOutcome::Plan {
                    workflow_plan: Box::new(workflow_plan),
                    attempts,
                    source: if prior.is_some() {
                        AdaptiveWorkflowPlanSource::ParetoSearchTeacher
                    } else {
                        AdaptiveWorkflowPlanSource::ModelColdStart
                    },
                });
            }
            Err(error) if attempts < CONDUCTOR_MAX_ATTEMPTS => {
                if let Some(control) = cancellation {
                    match control.begin_repair_attempt_at(
                        run_context_steer_epoch(run_context),
                        "conductor_repair",
                    ) {
                        Ok(Some(_)) => {}
                        Ok(None) => return Err(COLLABORATION_STEER_INTERRUPTED.to_string()),
                        Err(reason) => {
                            if await_direct_anchor_fallback(
                                state,
                                task_id,
                                run_context,
                                collaboration_id,
                                anchor_spec,
                                prompt_genome.verification,
                                cancellation,
                                anchor_supervisor,
                                direct_anchor_output,
                                Duration::from_millis(500),
                            )?
                            .is_some()
                            {
                                return Ok(AdaptiveConductorOutcome::DirectCommit);
                            }
                            return Err(format!(
                                "Conductor repair budget exhausted: {}",
                                reason.code()
                            ));
                        }
                    }
                }
                record_conductor_rejection(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    attempts,
                    &error,
                );
                conductor_response = match run_collaboration_stage(
                    state,
                    config,
                    task_id,
                    run_context,
                    collaboration_id,
                    "conductor_repair",
                    ModelRole::Planner,
                    conductor_model,
                    harness.repair_prompt(&conductor_response, &error),
                ) {
                    Ok(response) => response,
                    Err(repair_error) => {
                        if repair_error == COLLABORATION_STEER_INTERRUPTED
                            || collaboration_steer_pending(cancellation)
                        {
                            cancel_anytime_background(anchor_supervisor.as_ref(), None);
                            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
                        }
                        if await_direct_anchor_fallback(
                            state,
                            task_id,
                            run_context,
                            collaboration_id,
                            anchor_spec,
                            prompt_genome.verification,
                            cancellation,
                            anchor_supervisor,
                            direct_anchor_output,
                            Duration::from_millis(500),
                        )?
                        .is_some()
                        {
                            return Ok(AdaptiveConductorOutcome::DirectCommit);
                        }
                        return Err(format!(
                            "Conductor repair failed after {attempts} attempt(s): {repair_error}"
                        ));
                    }
                };
                attempts += 1;
            }
            Err(error) => {
                if await_direct_anchor_fallback(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    anchor_spec,
                    prompt_genome.verification,
                    cancellation,
                    anchor_supervisor,
                    direct_anchor_output,
                    Duration::from_millis(500),
                )?
                .is_some()
                {
                    return Ok(AdaptiveConductorOutcome::DirectCommit);
                }
                return Err(format!(
                    "Conductor failed to produce a valid workflow after {attempts} attempts: {error}"
                ));
            }
        }
    }
}

fn record_conductor_rejection(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    attempt: usize,
    validation_error: &str,
) {
    let Ok(mut store) = state.store.lock() else {
        return;
    };
    let _ = append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Conductor workflow rejected",
        metadata_with_context(
            [
                ("collaboration_id".to_string(), collaboration_id.to_string()),
                ("attempt".to_string(), attempt.to_string()),
                (
                    "validation_error".to_string(),
                    truncate_for_collaboration(validation_error, 2_000),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    );
}
