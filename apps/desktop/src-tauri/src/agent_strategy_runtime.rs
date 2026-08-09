#[path = "agent_strategy_causal_route.rs"]
mod causal_route;
#[path = "agent_strategy_context.rs"]
mod context;
#[path = "agent_strategy_finalization.rs"]
mod finalization;
#[path = "agent_strategy_preparation.rs"]
mod preparation;
#[path = "agent_strategy_recording.rs"]
mod recording;
#[path = "agent_strategy_requirements.rs"]
mod requirements;

#[cfg(test)]
pub(crate) use self::causal_route::causal_route_event_metadata;
use self::causal_route::run_decision_evolved_directive;
use self::finalization::{finalize_planned_run, PlannedRunFinalizeInput};
#[cfg(test)]
pub(crate) use self::preparation::should_evaluate_strategy_profile;
pub(crate) use self::preparation::{
    cumulative_effective_prompt_objective, effective_prompt_objective_for_messages,
};
use self::preparation::{ensure_planning_current, selected_strategy_profile};
#[cfg(test)]
pub(crate) use self::requirements::preferred_compatible_route_model;
pub(crate) use self::requirements::AgentPlanningSource;
use crate::agent_conductor_runtime::{
    attempt_conductor_decision, conductor_model_sequence, preferred_fallback_model,
    unique_configured_models,
};
use crate::agent_conductor_scheduler::{
    schedule_conductor_decision, ConductorDecisionOutcome, ConductorDecisionSchedule,
};
use crate::agent_execution_constraint::AgentExecutionConstraint;
use crate::app_state::AppState;
use crate::collaboration_service::{collaboration_recent_context, truncate_for_collaboration};
use crate::collaboration_stage_runtime::CollaborationStageError;
use crate::conductor_health_runtime;
use crate::configuration_models::ProviderConfig;
use crate::routing_learning_runtime::learning_budget_fingerprint;
use crate::workflow_routing_runtime::{conductor_historical_evidence, model_candidates_for_config};
use agent_core::{Message, Metadata, TaskId};
use agent_runtime::AgentRunControl;
use orchestrator::{
    AgentExecutionMode, AgentPolicy, AgentRouteRequirements, AgentRunDecision,
    AgentRunDecisionHarness, AgentRunDecisionRequest, ConductorExecutionContract,
    ConductorPromptGenome, ExecutionPlan, ExecutionPlanDecisionReason, RoutingContext,
    RoutingDecision,
};

#[derive(Debug, Clone)]
pub(crate) struct PlannedAgentRun {
    pub(crate) policy: AgentPolicy,
    pub(crate) execution_plan: ExecutionPlan,
    pub(crate) routing_context: RoutingContext,
    pub(crate) routing_decision: RoutingDecision,
    pub(crate) execution_contract: ConductorExecutionContract,
    pub(crate) source: AgentPlanningSource,
    pub(crate) attempts: usize,
    pub(crate) prompt_genome: ConductorPromptGenome,
    pub(crate) degradation_reason: Option<String>,
    pub(crate) attempted_conductor_models: Vec<String>,
    pub(crate) selected_conductor_model: Option<String>,
    pub(crate) route_requirements: AgentRouteRequirements,
}

pub(crate) struct AgentRunPlanningRequest<'a> {
    pub(crate) config: &'a ProviderConfig,
    pub(crate) task_id: &'a TaskId,
    pub(crate) prompt: &'a str,
    pub(crate) history: &'a [Message],
    pub(crate) effort: AgentPolicy,
    pub(crate) route_requirements: AgentRouteRequirements,
    pub(crate) cancellation: &'a AgentRunControl,
}

pub(crate) fn plan_agent_run(
    state: &tauri::State<'_, AppState>,
    run_context: &mut Metadata,
    request: AgentRunPlanningRequest<'_>,
) -> Result<PlannedAgentRun, CollaborationStageError> {
    let AgentRunPlanningRequest {
        config,
        task_id,
        prompt,
        history,
        effort,
        route_requirements,
        cancellation,
    } = request;
    ensure_planning_current(cancellation)?;
    let execution_constraint = AgentExecutionConstraint::from_context(run_context)
        .map_err(CollaborationStageError::Failed)?;
    run_context
        .entry("prompt_objective".to_string())
        .or_insert_with(|| truncate_for_collaboration(prompt, 6_000));
    let default_effective_objective = truncate_for_collaboration(
        run_context
            .get("initial_prompt_objective")
            .map(String::as_str)
            .unwrap_or(prompt),
        6_000,
    );
    run_context
        .entry("effective_prompt_objective".to_string())
        .or_insert(default_effective_objective);
    let candidates = model_candidates_for_config(config);
    let budget_fingerprint = learning_budget_fingerprint(run_context);
    let allowed_models = unique_configured_models(&candidates);
    let preferred_fallback_model = preferred_fallback_model(config, effort, &allowed_models);
    let (profile, profile_source) = selected_strategy_profile(state, config, effort, run_context)
        .map_err(CollaborationStageError::Failed)?;
    let route_prompt_profile_sha256 = profile
        .route_decision_profile_sha256(effort.label())
        .map_err(CollaborationStageError::Failed)?;
    let evolved_route_directive = run_decision_evolved_directive(&profile, effort)
        .map_err(CollaborationStageError::Failed)?;
    let recent_context = collaboration_recent_context(history);

    if execution_constraint.is_matched_memory_effect() {
        let decision = requirements::fast_direct_route_decision(
            preferred_fallback_model,
            &candidates,
            route_requirements,
        )
        .map_err(CollaborationStageError::Failed)?;
        let decision = execution_constraint
            .apply(decision, effort)
            .map_err(CollaborationStageError::Failed)?;
        let planned = finalize_planned_run(
            prompt,
            candidates,
            PlannedRunFinalizeInput {
                conductor_candidate: decision.clone(),
                decision,
                compatibility_route: None,
                source: AgentPlanningSource::MatchedMemoryEvaluation,
                decision_reason: ExecutionPlanDecisionReason::MatchedMemoryEvaluation,
                attempts: 0,
                prompt_genome: profile,
                effort,
                degradation_reason: None,
                attempted_conductor_models: Vec::new(),
                selected_conductor_model: None,
                route_requirements,
                budget_fingerprint,
                recent_context: recent_context.clone(),
                route_prompt_profile_sha256: route_prompt_profile_sha256.clone(),
            },
        )
        .map_err(CollaborationStageError::Failed)?;
        ensure_planning_current(cancellation)?;
        planned
            .apply_to_context(run_context)
            .map_err(CollaborationStageError::Failed)?;
        run_context.insert("prompt_profile_source".to_string(), profile_source.clone());
        recording::record_planned_agent_run(state, task_id, run_context, &planned, &profile_source)
            .map_err(CollaborationStageError::Failed)?;
        return Ok(planned);
    }

    if !effort.uses_conductor() {
        conductor_health_runtime::record_conductor_fast_bypass(run_context);
        let decision = requirements::fast_direct_route_decision(
            preferred_fallback_model,
            &candidates,
            route_requirements,
        )
        .map_err(CollaborationStageError::Failed)?;
        let decision = execution_constraint
            .apply(decision, effort)
            .map_err(CollaborationStageError::Failed)?;
        let planned = finalize_planned_run(
            prompt,
            candidates,
            PlannedRunFinalizeInput {
                conductor_candidate: decision.clone(),
                decision,
                compatibility_route: None,
                source: AgentPlanningSource::FastDirect,
                decision_reason: ExecutionPlanDecisionReason::FastPolicy,
                attempts: 0,
                prompt_genome: profile,
                effort,
                degradation_reason: None,
                attempted_conductor_models: Vec::new(),
                selected_conductor_model: None,
                route_requirements,
                budget_fingerprint,
                recent_context: recent_context.clone(),
                route_prompt_profile_sha256: route_prompt_profile_sha256.clone(),
            },
        )
        .map_err(CollaborationStageError::Failed)?;
        ensure_planning_current(cancellation)?;
        planned
            .apply_to_context(run_context)
            .map_err(CollaborationStageError::Failed)?;
        run_context.insert("prompt_profile_source".to_string(), profile_source.clone());
        recording::record_planned_agent_run(state, task_id, run_context, &planned, &profile_source)
            .map_err(CollaborationStageError::Failed)?;
        return Ok(planned);
    }
    let fallback_model = requirements::compatible_route_fallback_model(
        &preferred_fallback_model,
        &allowed_models,
        &candidates,
        route_requirements,
    )
    .map_err(CollaborationStageError::Failed)?;
    let max_parallelism = effort.max_parallelism();
    let configured_conductor_models = conductor_model_sequence(config);
    let (provider_scope, health_generation, conductor_models) =
        conductor_health_runtime::route_conductor_models(
            &state.conductor_health,
            &config.base_url,
            &configured_conductor_models,
            run_context,
        );
    let (historical_evidence, matched_collaboration_evidence) =
        conductor_historical_evidence(state, &allowed_models).unwrap_or_default();
    let base_request = AgentRunDecisionRequest {
        objective: prompt.to_string(),
        recent_context: recent_context.clone(),
        effort: effort.label().to_string(),
        conductor_model: conductor_models
            .first()
            .cloned()
            .unwrap_or_else(|| config.model_for_conductor()),
        allowed_models: allowed_models.clone(),
        model_candidates: candidates.clone(),
        max_parallelism,
        evolved_directive: evolved_route_directive,
        historical_evidence,
        matched_collaboration_evidence,
        route_requirements,
        execution_constraints: "The foreground executor may use permission-gated tools after user approval. Isolated workflow workers can use only exposed permissionless read-only evidence tools: they cannot operate browser/computer controls, mutate the workspace, execute shell commands, or request user approval. For interactive or effectful tasks, choose workflow only when bounded isolated analysis or verification adds independent value around foreground execution."
            .to_string(),
        budget_fingerprint: budget_fingerprint.clone(),
        prompt_profile_sha256: route_prompt_profile_sha256.clone(),
    };
    let decision_id = format!(
        "{}-run-decision",
        run_context
            .get("agent_run_id")
            .map(String::as_str)
            .unwrap_or("agent")
    );
    let mut attempts = 0usize;
    let schedule = schedule_conductor_decision(
        &conductor_models,
        &mut attempts,
        |model_index, conductor_model, has_alternate_model, attempts| {
            let mut request = base_request.clone();
            request.conductor_model = conductor_model.to_string();
            let harness = AgentRunDecisionHarness::new(request);
            let calls_before = *attempts;
            let result = attempt_conductor_decision(
                state,
                config,
                task_id,
                run_context,
                &decision_id,
                model_index,
                has_alternate_model,
                conductor_model,
                &harness,
                attempts,
            );
            conductor_health_runtime::record_conductor_attempt(
                &state.conductor_health,
                &provider_scope,
                health_generation,
                conductor_model,
                (*attempts).saturating_sub(calls_before),
                &result,
                run_context,
            );
            result
        },
    )?;
    let ConductorDecisionSchedule {
        outcome,
        attempted_models: attempted_conductor_models,
        selected_model: selected_conductor_model,
        failure_reasons,
    } = schedule;

    let (
        conductor_candidate,
        decision,
        compatibility_route,
        source,
        mut decision_reason,
        degradation_reason,
    ) =
        match outcome {
            ConductorDecisionOutcome::Selected(draft) => {
                let source =
                    requirements::selected_conductor_source(attempted_conductor_models.len());
                (
                    draft.conductor_candidate.clone(),
                    draft.conductor_candidate,
                    Some(draft.compatibility_route),
                    source,
                    ExecutionPlanDecisionReason::ConductorSelection,
                    None,
                )
            }
            ConductorDecisionOutcome::Exhausted => {
                let reason = if failure_reasons.is_empty() {
                    "no configured conductor model was available".to_string()
                } else {
                    failure_reasons.join(" | ")
                };
                let decision = AgentRunDecision::degraded_conductor_fallback(
                    fallback_model,
                    effort.label(),
                    allowed_models.len(),
                    max_parallelism,
                    &reason,
                );
                let source = if decision.execution == AgentExecutionMode::Workflow {
                    AgentPlanningSource::DegradedWorkflow
                } else {
                    AgentPlanningSource::DegradedDirect
                };
                (
                    decision.clone(),
                    decision,
                    None,
                    source,
                    ExecutionPlanDecisionReason::DegradedFallback,
                    Some(reason),
                )
            }
        };
    let decision = execution_constraint
        .apply(decision, effort)
        .map_err(CollaborationStageError::Failed)?;
    if execution_constraint.is_grounded_direct() {
        decision_reason = ExecutionPlanDecisionReason::RuntimeGroundedDirect;
        decision
            .validate(&allowed_models, max_parallelism)
            .map_err(CollaborationStageError::Failed)?;
    }
    let planned = finalize_planned_run(
        prompt,
        candidates,
        PlannedRunFinalizeInput {
            conductor_candidate,
            decision,
            compatibility_route,
            source,
            decision_reason,
            attempts,
            prompt_genome: profile,
            effort,
            degradation_reason,
            attempted_conductor_models,
            selected_conductor_model,
            route_requirements,
            budget_fingerprint,
            recent_context,
            route_prompt_profile_sha256,
        },
    )
    .map_err(CollaborationStageError::Failed)?;
    ensure_planning_current(cancellation)?;
    planned
        .apply_to_context(run_context)
        .map_err(CollaborationStageError::Failed)?;
    run_context.insert("prompt_profile_source".to_string(), profile_source.clone());
    recording::record_planned_agent_run(state, task_id, run_context, &planned, &profile_source)
        .map_err(CollaborationStageError::Failed)?;
    Ok(planned)
}

#[cfg(test)]
#[path = "agent_strategy_shipping_tests.rs"]
mod tests;
