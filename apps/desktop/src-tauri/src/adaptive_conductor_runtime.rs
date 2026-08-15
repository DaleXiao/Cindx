use super::*;
use orchestrator::MAX_ADAPTIVE_WORKFLOW_STEPS;

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
}

impl AdaptiveWorkflowPlanSource {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::CheckpointResume => "checkpoint_resume",
            Self::RunDecisionProposal => "run_decision_proposal",
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
    pub(super) checkpoint_resumable: bool,
    pub(super) resume_key: &'a str,
    pub(super) resumed_from_workflow_id: Option<&'a str>,
    pub(super) cancellation: Option<&'a Arc<AgentRunControl>>,
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
        checkpoint_resumable,
        resume_key,
        resumed_from_workflow_id,
        cancellation,
    } = context;

    if collaboration_steer_pending(cancellation) {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }

    if let Some(checkpoint) = checkpoint {
        if !checkpoint_resumable {
            record_conductor_rejection(
                state,
                task_id,
                run_context,
                collaboration_id,
                0,
                "checkpoint is quarantined for Owner untrusted partial handoff",
            );
            return Ok(AdaptiveConductorOutcome::DirectCommit);
        }
        if let Err(error) = checkpoint
            .plan
            .validate_owner_execution_graph(execution_contract.verification_required)
            .and_then(|()| validate_runtime_workflow_model_profiles(config, &checkpoint.plan))
        {
            record_conductor_rejection(
                state,
                task_id,
                run_context,
                collaboration_id,
                0,
                &format!("checkpoint workflow graph rejected: {error}"),
            );
            return Ok(AdaptiveConductorOutcome::DirectCommit);
        }
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

    let Some(proposal) = route_workflow_proposal else {
        record_conductor_rejection(
            state,
            task_id,
            run_context,
            collaboration_id,
            0,
            "run-decision workflow proposal is missing",
        );
        return Ok(AdaptiveConductorOutcome::DirectCommit);
    };
    let (proposal_steps, proposal_models) = route_workflow_capacity(proposal);

    let validation_models = crate::agent_conductor_runtime::unique_configured_models(
        &crate::workflow_routing_runtime::model_candidates_for_config(config),
    );
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
        worker_models: validation_models,
        role_hints: role_hints.clone(),
        budget: WorkflowBudget {
            max_steps: proposal_steps,
            max_models: proposal_models,
            max_model_turns_per_step: DEFAULT_COLLABORATION_WORKER_TURNS,
            max_tool_calls_per_step: MAX_COLLABORATION_WORKER_TOOL_CALLS,
            max_output_tokens_per_step: COLLABORATION_MAX_OUTPUT_TOKENS as usize,
        },
        execution_contract: execution_contract.clone(),
        prior_hint: prior.map(WorkflowTopologyPrior::prompt_hint),
        prompt_evolution_enabled: config.prompt_evolution_enabled,
        prompt_genome: prompt_genome.clone(),
        parallel_read_only_authorized: parallel_read_only_authorized(run_context),
    });
    match harness
        .plan_from_proposal(proposal)
        .and_then(|workflow_plan| {
            workflow_plan
                .validate_owner_execution_graph(execution_contract.verification_required)
                .and_then(|()| validate_runtime_workflow_model_profiles(config, &workflow_plan))
                .map(|()| workflow_plan)
        }) {
        Ok(workflow_plan) => Ok(AdaptiveConductorOutcome::Plan {
            workflow_plan: Box::new(workflow_plan),
            attempts: 0,
            source: AdaptiveWorkflowPlanSource::RunDecisionProposal,
        }),
        Err(error) => {
            record_conductor_rejection(
                state,
                task_id,
                run_context,
                collaboration_id,
                0,
                &format!("run-decision workflow proposal rejected: {error}"),
            );
            Ok(AdaptiveConductorOutcome::DirectCommit)
        }
    }
}

fn validate_runtime_workflow_model_profiles(
    config: &ProviderConfig,
    workflow_plan: &WorkflowPlanIr,
) -> Result<(), String> {
    let candidates = crate::workflow_routing_runtime::model_candidates_for_config(config);
    for step in &workflow_plan.steps {
        orchestrator::validate_workflow_step_model_profile(
            &step.id,
            &step.model,
            &step.contract.output_kind,
            &candidates,
        )?;
    }
    Ok(())
}

/// The only authority that widens the owner execution graph to two read-only
/// specialists: preparation persisted a forbidden prompt effect authority.
/// Any other value (including a missing receipt) fails closed to the
/// single-specialist topology.
pub(crate) fn parallel_read_only_authorized(run_context: &Metadata) -> bool {
    run_context
        .get("route_effect_authority")
        .map(String::as_str)
        == Some(orchestrator::AgentEffectAuthority::Forbidden.label())
}

pub(super) fn route_workflow_capacity(proposal: &WorkflowPlanProposal) -> (usize, usize) {
    let model_count = proposal
        .steps
        .iter()
        .filter(|step| step.output_kind != WorkflowOutputKind::Synthesis)
        .map(|step| step.model.trim())
        .filter(|model| !model.is_empty())
        .collect::<BTreeSet<_>>()
        .len();
    (
        proposal.steps.len().clamp(1, MAX_ADAPTIVE_WORKFLOW_STEPS),
        model_count.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS),
    )
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
