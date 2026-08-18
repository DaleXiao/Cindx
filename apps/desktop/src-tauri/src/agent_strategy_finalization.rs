use super::{causal_route, requirements, AgentPlanningSource, PlannedAgentRun};
use orchestrator::{
    validate_primary_model_profile, AgentExecutionMode, AgentPolicy, AgentRouteRequirements,
    AgentRunDecision, CausalRouteSelectionV2, ConductorPromptGenome, ExecutionPlan,
    ExecutionPlanDecisionReason, ModelCandidate,
};

pub(super) struct PlannedRunFinalizeInput {
    pub(super) conductor_candidate: AgentRunDecision,
    pub(super) decision: AgentRunDecision,
    pub(super) compatibility_route: Option<CausalRouteSelectionV2>,
    pub(super) source: AgentPlanningSource,
    pub(super) decision_reason: ExecutionPlanDecisionReason,
    pub(super) attempts: usize,
    pub(super) prompt_genome: ConductorPromptGenome,
    pub(super) effort: AgentPolicy,
    pub(super) degradation_reason: Option<String>,
    pub(super) attempted_conductor_models: Vec<String>,
    pub(super) selected_conductor_model: Option<String>,
    pub(super) route_requirements: AgentRouteRequirements,
    pub(super) budget_fingerprint: Option<String>,
    pub(super) recent_context: String,
    pub(super) route_prompt_profile_sha256: String,
}

pub(super) fn finalize_planned_run(
    prompt: &str,
    candidates: Vec<ModelCandidate>,
    input: PlannedRunFinalizeInput,
) -> Result<PlannedAgentRun, String> {
    let PlannedRunFinalizeInput {
        mut conductor_candidate,
        mut decision,
        compatibility_route,
        source,
        decision_reason,
        attempts,
        prompt_genome,
        effort,
        degradation_reason,
        attempted_conductor_models,
        selected_conductor_model,
        route_requirements,
        budget_fingerprint,
        recent_context,
        route_prompt_profile_sha256,
    } = input;
    decision = requirements::apply_and_validate_route_requirements(
        decision,
        degradation_reason.is_some(),
        &candidates,
        route_requirements,
    )?;
    // Mirror the degraded projection onto the recorded candidate so its action
    // identity cannot diverge from the executed decision; matched-route anchors
    // never carry a degradation reason and keep their raw workflow candidate.
    conductor_candidate = requirements::apply_and_validate_route_requirements(
        conductor_candidate,
        degradation_reason.is_some(),
        &candidates,
        route_requirements,
    )?;
    validate_primary_model_profile(&decision, &candidates)?;
    validate_primary_model_profile(&conductor_candidate, &candidates)?;
    let compatibility_route = causal_route::finalize_causal_route(
        prompt,
        &recent_context,
        effort,
        route_requirements,
        &candidates,
        budget_fingerprint,
        route_prompt_profile_sha256,
        source,
        degradation_reason.is_some(),
        &conductor_candidate,
        compatibility_route,
    )?;
    let routing_context = decision.routing_context(prompt, candidates);
    let routing_decision = decision.routing_decision();
    let execution_contract = decision.execution_contract(effort.label());
    let workflow_execution_profile_sha256 = (decision.execution == AgentExecutionMode::Workflow)
        .then(|| prompt_genome.workflow_execution_profile_sha256())
        .transpose()?;
    let execution_plan = ExecutionPlan::new(
        conductor_candidate,
        decision,
        decision_reason,
        compatibility_route,
        workflow_execution_profile_sha256,
    )?;
    Ok(PlannedAgentRun {
        policy: effort,
        execution_plan,
        routing_context,
        routing_decision,
        execution_contract,
        source,
        attempts,
        prompt_genome,
        degradation_reason,
        attempted_conductor_models,
        selected_conductor_model,
        route_requirements,
    })
}
