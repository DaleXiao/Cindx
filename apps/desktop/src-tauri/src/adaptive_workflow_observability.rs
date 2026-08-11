use super::*;
use crate::collaboration_service::WorkflowRoleCoverage;
use orchestrator::AdaptiveWorkflow;

pub(super) struct AdaptiveWorkflowObservabilityContext<'a, 'state> {
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) workflow_plan: &'a WorkflowPlanIr,
    pub(super) workflow: &'a AdaptiveWorkflow,
    pub(super) layer_count: usize,
    pub(super) role_coverage: &'a WorkflowRoleCoverage,
    pub(super) resume_key: &'a str,
    pub(super) resumed_from_checkpoint: bool,
    pub(super) prompt_genome: &'a ConductorPromptGenome,
    pub(super) prompt_genome_json: &'a str,
    pub(super) effort: &'a str,
    pub(super) selection_mode: &'a str,
    pub(super) conductor_model: &'a str,
    pub(super) execution_contract: &'a ConductorExecutionContract,
    pub(super) conductor_attempts: usize,
    pub(super) workflow_plan_source: AdaptiveWorkflowPlanSource,
    pub(super) prior: Option<&'a WorkflowTopologyPrior>,
    pub(super) agent_budget: usize,
    pub(super) workflow_checkpoint: &'a WorkflowExecutionCheckpoint,
}

pub(super) fn record_adaptive_workflow_planned(
    context: AdaptiveWorkflowObservabilityContext<'_, '_>,
) -> Result<(), String> {
    let AdaptiveWorkflowObservabilityContext {
        state,
        task_id,
        run_context,
        collaboration_id,
        workflow_plan,
        workflow,
        layer_count,
        role_coverage,
        resume_key,
        resumed_from_checkpoint,
        prompt_genome,
        prompt_genome_json,
        effort,
        selection_mode,
        conductor_model,
        execution_contract,
        conductor_attempts,
        workflow_plan_source,
        prior,
        agent_budget,
        workflow_checkpoint,
    } = context;
    let workflow_summary = workflow
        .steps
        .iter()
        .map(|step| {
            format!(
                "{}:{}:{}<-[{}]",
                step.id,
                step.role,
                step.model,
                step.access.join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let unique_models = workflow_plan.physical_model_count();
    let workflow_ir = workflow_plan.to_json()?;
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration workflow planned",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "workflow_schema".to_string(),
                        WORKFLOW_IR_SCHEMA.to_string(),
                    ),
                    ("workflow_ir".to_string(), workflow_ir),
                    ("workflow_resume_key".to_string(), resume_key.to_string()),
                    (
                        "workflow_checkpoint_schema".to_string(),
                        WORKFLOW_CHECKPOINT_SCHEMA.to_string(),
                    ),
                    ("resumed".to_string(), resumed_from_checkpoint.to_string()),
                    (
                        "prompt_profile".to_string(),
                        workflow_plan.prompt_profile.clone(),
                    ),
                    ("prompt_effort".to_string(), effort.to_string()),
                    (
                        "prompt_generation".to_string(),
                        prompt_genome.generation.to_string(),
                    ),
                    ("prompt_genome".to_string(), prompt_genome_json.to_string()),
                    (
                        "prompt_selection_mode".to_string(),
                        selection_mode.to_string(),
                    ),
                    (
                        "prompt_evolution_status".to_string(),
                        run_context
                            .get("prompt_rollout_status")
                            .cloned()
                            .unwrap_or_else(|| "unavailable".to_string()),
                    ),
                    ("conductor_version".to_string(), "agent_v2".to_string()),
                    ("conductor_model".to_string(), conductor_model.to_string()),
                    (
                        "conductor_contract".to_string(),
                        execution_contract.to_json()?,
                    ),
                    (
                        "expected_collaboration_uplift_bps".to_string(),
                        execution_contract.expected_uplift_bps.to_string(),
                    ),
                    (
                        "conductor_attempts".to_string(),
                        conductor_attempts.to_string(),
                    ),
                    (
                        "conductor_source".to_string(),
                        workflow_plan_source.label().to_string(),
                    ),
                    (
                        "teacher_examples".to_string(),
                        prior
                            .map(|prior| prior.examples)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    (
                        "workflow_steps".to_string(),
                        workflow.steps.len().to_string(),
                    ),
                    ("workflow_layers".to_string(), layer_count.to_string()),
                    ("worker_models".to_string(), unique_models.to_string()),
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
                        "verifier_steps".to_string(),
                        role_coverage.verifier_steps.to_string(),
                    ),
                    (
                        "synthesizer_steps".to_string(),
                        role_coverage.synthesizer_steps.to_string(),
                    ),
                    (
                        "cross_reviewed".to_string(),
                        role_coverage.cross_reviewed.to_string(),
                    ),
                    (
                        "step_budget".to_string(),
                        adaptive_workflow_step_budget(agent_budget).to_string(),
                    ),
                    (
                        "workflow".to_string(),
                        truncate_for_collaboration(&workflow_summary, 4_000),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        #[cfg(feature = "realworld-eval")]
        crate::collaboration_learning_eval_runtime::append_workflow_assignment_if_enabled(
            &mut store,
            task_id,
            run_context,
            collaboration_id,
            workflow_plan,
            execution_contract.verification_required,
        )
        .map_err(|error| error.to_string())?;
    }
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        if resumed_from_checkpoint {
            "Collaboration workflow checkpoint restored"
        } else {
            "Collaboration workflow checkpoint created"
        },
        if resumed_from_checkpoint {
            "resumed"
        } else {
            "planned"
        },
        None,
        workflow_checkpoint,
    )
}
