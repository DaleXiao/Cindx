use super::finalization::{finalize_planned_run, PlannedRunFinalizeInput};
use super::recording;
use super::{AgentPlanningSource, PlannedAgentRun};
use crate::agent_execution_constraint::{AgentExecutionConstraint, MatchedRoutePlanAnchor};
use crate::app_state::AppState;
use crate::collaboration_stage_runtime::CollaborationStageError;
use agent_core::{Metadata, TaskId};
use agent_runtime::AgentRunControl;
use orchestrator::{
    AgentPolicy, AgentRouteRequirements, ConductorPromptGenome, ExecutionPlanDecisionReason,
    ModelCandidate,
};

pub(super) struct MatchedRoutePlanningInput<'a> {
    pub(super) prompt: &'a str,
    pub(super) candidates: Vec<ModelCandidate>,
    pub(super) anchor: MatchedRoutePlanAnchor,
    pub(super) profile: ConductorPromptGenome,
    pub(super) profile_source: &'a str,
    pub(super) effort: AgentPolicy,
    pub(super) route_requirements: AgentRouteRequirements,
    pub(super) cancellation: &'a AgentRunControl,
    pub(super) budget_fingerprint: Option<String>,
    pub(super) recent_context: String,
    pub(super) route_prompt_profile_sha256: String,
    pub(super) execution_constraint: AgentExecutionConstraint,
}

pub(super) fn shared_anchor_from_context(
    run_context: &Metadata,
    execution_constraint: AgentExecutionConstraint,
) -> Result<Option<MatchedRoutePlanAnchor>, CollaborationStageError> {
    let anchor = MatchedRoutePlanAnchor::from_context(run_context)
        .map_err(CollaborationStageError::Failed)?;
    if anchor.is_some() && !execution_constraint.is_matched_route() {
        return Err(CollaborationStageError::Failed(
            "matched route plan anchor requires a matched route execution constraint".to_string(),
        ));
    }
    Ok(anchor)
}

pub(super) fn plan_from_shared_anchor(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &mut Metadata,
    input: MatchedRoutePlanningInput<'_>,
) -> Result<PlannedAgentRun, CollaborationStageError> {
    let conductor_candidate = input.anchor.conductor_candidate;
    let decision = input
        .execution_constraint
        // Matched-route evaluation arms keep workflow authority; quarantine
        // applies to production Native runs only.
        .apply(conductor_candidate.clone(), input.effort, false)
        .map_err(CollaborationStageError::Failed)?;
    let planned = finalize_planned_run(
        input.prompt,
        input.candidates,
        PlannedRunFinalizeInput {
            conductor_candidate,
            decision,
            compatibility_route: Some(input.anchor.compatibility_route),
            source: AgentPlanningSource::MatchedRouteEvaluation,
            decision_reason: ExecutionPlanDecisionReason::MatchedRouteEvaluation,
            attempts: 0,
            prompt_genome: input.profile,
            effort: input.effort,
            degradation_reason: None,
            attempted_conductor_models: Vec::new(),
            selected_conductor_model: None,
            route_requirements: input.route_requirements,
            workflow_plan: Some(input.anchor.workflow_plan),
            budget_fingerprint: input.budget_fingerprint,
            recent_context: input.recent_context,
            route_prompt_profile_sha256: input.route_prompt_profile_sha256,
        },
    )
    .map_err(CollaborationStageError::Failed)?;
    super::preparation::ensure_planning_current(input.cancellation)?;
    planned
        .apply_to_context(run_context)
        .map_err(CollaborationStageError::Failed)?;
    run_context.insert(
        "prompt_profile_source".to_string(),
        input.profile_source.to_string(),
    );
    recording::record_planned_agent_run(
        state,
        task_id,
        run_context,
        &planned,
        input.profile_source,
        input.cancellation,
    )?;
    Ok(planned)
}
