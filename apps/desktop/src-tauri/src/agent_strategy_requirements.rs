use super::PlannedAgentRun;
use agent_core::Metadata;
use orchestrator::{
    AgentExecutionMode, AgentPolicy, AgentRouteRequirements, AgentRunDecision,
    CausalRouteSelectionV2, MemoryRecallPlan, MemoryRecallPolicy, ModelCandidate,
    MAX_RUN_DECISION_QUERY_CHARS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentPlanningSource {
    FastDirect,
    MatchedMemoryEvaluation,
    MatchedRouteEvaluation,
    DynamicConductor,
    DynamicConductorReplanned,
    DegradedDirect,
    DegradedWorkflow,
}

impl AgentPlanningSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::FastDirect => "fast_direct",
            Self::MatchedMemoryEvaluation => "matched_memory_evaluation",
            Self::MatchedRouteEvaluation => "matched_route_shared_anchor",
            Self::DynamicConductor => "dynamic_conductor_v2",
            Self::DynamicConductorReplanned => "dynamic_conductor_replanned",
            Self::DegradedDirect => "dynamic_conductor_degraded_direct",
            Self::DegradedWorkflow => "dynamic_conductor_degraded_workflow",
        }
    }
}

pub(crate) fn preferred_compatible_route_model(
    preferred: &str,
    allowed_models: &[String],
    candidates: &[ModelCandidate],
    requirements: AgentRouteRequirements,
) -> Option<String> {
    allowed_models
        .iter()
        .filter(|model| model.as_str() == preferred)
        .chain(
            allowed_models
                .iter()
                .filter(|model| model.as_str() != preferred),
        )
        .map(String::as_str)
        .find(|model| requirements.model_satisfies(model, candidates))
        .map(str::to_string)
}

pub(super) fn fast_direct_route_decision(
    preferred_model: String,
    candidates: &[ModelCandidate],
    requirements: AgentRouteRequirements,
) -> Result<AgentRunDecision, String> {
    let decision = requirements.apply_to_direct(AgentRunDecision::direct(preferred_model));
    requirements
        .validate_decision(&decision, candidates)
        .map_err(|error| {
            format!("Fast default model cannot satisfy runtime route requirements: {error}")
        })?;
    Ok(decision)
}

pub(super) fn compatible_route_fallback_model(
    preferred: &str,
    allowed_models: &[String],
    candidates: &[ModelCandidate],
    requirements: AgentRouteRequirements,
) -> Result<String, String> {
    preferred_compatible_route_model(preferred, allowed_models, candidates, requirements)
        .ok_or_else(|| {
            format!(
                "no configured model satisfies runtime route requirements: minimum_tool_requirement={}, image_input_required={}",
                requirements.minimum_tool_requirement.label(),
                requirements.image_input_required,
            )
        })
}

pub(super) fn apply_and_validate_route_requirements(
    decision: AgentRunDecision,
    degraded: bool,
    candidates: &[ModelCandidate],
    requirements: AgentRouteRequirements,
) -> Result<AgentRunDecision, String> {
    let decision = if decision.execution == AgentExecutionMode::Direct || degraded {
        requirements.apply_to_direct(decision)
    } else {
        decision
    };
    requirements.validate_decision(&decision, candidates)?;
    Ok(decision)
}

pub(super) fn selected_conductor_source(attempted_models: usize) -> AgentPlanningSource {
    if attempted_models > 1 {
        AgentPlanningSource::DynamicConductorReplanned
    } else {
        AgentPlanningSource::DynamicConductor
    }
}

pub(super) fn route_decision_metadata(planned: &PlannedAgentRun) -> Metadata {
    let decision_reason = planned
        .execution_plan
        .decision_receipt
        .as_ref()
        .map(|receipt| receipt.reason.label())
        .unwrap_or("legacy");
    let mut metadata = [
        (
            "route_minimum_tool_requirement".to_string(),
            planned
                .route_requirements
                .minimum_tool_requirement
                .label()
                .to_string(),
        ),
        (
            "route_image_input_required".to_string(),
            planned.route_requirements.image_input_required.to_string(),
        ),
        (
            "route_effect_authority".to_string(),
            planned
                .route_requirements
                .effect_authority
                .label()
                .to_string(),
        ),
        (
            "execution_plan_decision_reason".to_string(),
            decision_reason.to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    metadata.extend(
        planned
            .execution_plan
            .action()
            .route_observability_metadata(),
    );
    metadata
}

/// Auto/Pro default memory recall for production (Native) runs: when the
/// conductor declines memory entirely, upgrade the policy to relevant recall
/// keyed on the run prompt so durable project memory participates in every
/// non-trivial task. Fast returns before this point; explicit
/// Relevant/Comprehensive choices are kept; evaluation arms are exempt.
pub(crate) fn apply_default_memory_recall(
    constraint: &crate::agent_execution_constraint::AgentExecutionConstraint,
    decision: &mut AgentRunDecision,
    prompt: &str,
) {
    if constraint.is_native() && decision.memory.policy == MemoryRecallPolicy::None {
        decision.memory = MemoryRecallPlan {
            policy: MemoryRecallPolicy::Relevant,
            query: crate::collaboration_service::truncate_for_collaboration(
                &plan_conditioned_recall_query(decision, prompt),
                MAX_RUN_DECISION_QUERY_CHARS.saturating_sub(12),
            ),
        };
    }
}

/// Condition default memory recall on the plan, not just the raw prompt: the
/// task class and the plan's focused retrieval query make recall target the
/// knowledge the plan determined it needs, surfacing more relevant durable
/// memory for the same recall budget.
fn plan_conditioned_recall_query(decision: &AgentRunDecision, prompt: &str) -> String {
    let mut query = prompt.to_string();
    query.push_str(" | task_class: ");
    query.push_str(decision.task_class.label());
    if decision.retrieval.enabled() {
        let retrieval_query = decision.retrieval.query.trim();
        if !retrieval_query.is_empty() {
            query.push_str(" | plan_retrieval: ");
            query.push_str(retrieval_query);
        }
    }
    query
}

pub(super) fn align_candidate_and_route(
    constraint: &crate::agent_execution_constraint::AgentExecutionConstraint,
    decision: AgentRunDecision,
    conductor_candidate: AgentRunDecision,
    mut compatibility_route: Option<CausalRouteSelectionV2>,
    effort: AgentPolicy,
    workflow_quarantined: bool,
    prompt: &str,
) -> Result<(AgentRunDecision, AgentRunDecision, Option<CausalRouteSelectionV2>), String> {
    let mut decision = constraint.apply(decision, effort, workflow_quarantined)?;
    // Quarantine clamps the action; clamp the recorded candidate identically and
    // drop the stale route so finalize recomputes it, keeping the plan consistent.
    let (mut conductor_candidate, candidate_clamped) = constraint
        .quarantine_conductor_candidate(conductor_candidate, effort, workflow_quarantined)?;
    if candidate_clamped {
        compatibility_route = None;
    }
    // Default memory recall rewrites the action identity after the draft route
    // receipt was computed; mirror it onto the recorded candidate and drop the
    // stale route so finalize recomputes it, keeping the plan consistent.
    if apply_default_memory_recall_pair(constraint, &mut decision, &mut conductor_candidate, prompt)
    {
        compatibility_route = None;
    }
    Ok((decision, conductor_candidate, compatibility_route))
}

pub(crate) fn apply_default_memory_recall_pair(
    constraint: &crate::agent_execution_constraint::AgentExecutionConstraint,
    decision: &mut AgentRunDecision,
    conductor_candidate: &mut AgentRunDecision,
    prompt: &str,
) -> bool {
    let before = decision.memory.policy;
    apply_default_memory_recall(constraint, decision, prompt);
    let changed = decision.memory.policy != before;
    if changed {
        apply_default_memory_recall(constraint, conductor_candidate, prompt);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_memory_recall_upgrades_none_and_preserves_explicit_policies() {
        let native = crate::agent_execution_constraint::AgentExecutionConstraint::Native;
        let mut decision = AgentRunDecision::direct("executor");
        assert_eq!(decision.memory.policy, MemoryRecallPolicy::None);
        apply_default_memory_recall(&native, &mut decision, "Summarize the project memory needs");
        assert_eq!(decision.memory.policy, MemoryRecallPolicy::Relevant);
        assert!(decision.memory.query.starts_with("Summarize the project memory needs"));
        assert!(decision.memory.query.contains("task_class:"));

        let mut decision = AgentRunDecision::direct("executor");
        decision.memory = MemoryRecallPlan {
            policy: MemoryRecallPolicy::Comprehensive,
            query: "existing".to_string(),
        };
        apply_default_memory_recall(&native, &mut decision, "ignored");
        assert_eq!(decision.memory.policy, MemoryRecallPolicy::Comprehensive);
        assert_eq!(decision.memory.query, "existing");
    }

    #[test]
    fn default_memory_recall_conditions_the_query_on_the_plan() {
        let native = crate::agent_execution_constraint::AgentExecutionConstraint::Native;
        let mut decision = AgentRunDecision::direct("executor");
        decision.retrieval.query = "auth session expiry".to_string();
        decision
            .retrieval
            .channels
            .insert(orchestrator::WorkspaceRetrievalChannel::Semantic);

        apply_default_memory_recall(&native, &mut decision, "Why does login expire early?");
        assert!(decision.memory.query.contains("Why does login expire early?"));
        assert!(decision.memory.query.contains("plan_retrieval: auth session expiry"));
        assert!(decision.memory.query.contains("task_class:"));
    }

    #[test]
    fn default_memory_recall_skips_matched_evaluation_arms() {
        let grounded = crate::agent_execution_constraint::AgentExecutionConstraint::GroundedDirect;
        let mut decision = AgentRunDecision::direct("executor");
        apply_default_memory_recall(&grounded, &mut decision, "prompt");
        assert_eq!(decision.memory.policy, MemoryRecallPolicy::None);
    }

    #[test]
    fn default_memory_recall_pair_mirrors_the_candidate_and_reports_change() {
        let native = crate::agent_execution_constraint::AgentExecutionConstraint::Native;
        let mut decision = AgentRunDecision::direct("executor");
        let mut candidate = decision.clone();
        assert!(apply_default_memory_recall_pair(
            &native,
            &mut decision,
            &mut candidate,
            "greet the user"
        ));
        assert_eq!(decision.memory.policy, MemoryRecallPolicy::Relevant);
        assert_eq!(decision.memory, candidate.memory);

        let mut decision = AgentRunDecision::direct("executor");
        decision.memory = MemoryRecallPlan {
            policy: MemoryRecallPolicy::Comprehensive,
            query: "kept".to_string(),
        };
        let mut candidate = decision.clone();
        assert!(!apply_default_memory_recall_pair(
            &native,
            &mut decision,
            &mut candidate,
            "ignored"
        ));
        assert_eq!(candidate.memory.query, "kept");
    }

    #[test]
    fn default_memory_recall_truncates_long_prompts_to_the_query_budget() {
        let native = crate::agent_execution_constraint::AgentExecutionConstraint::Native;
        let mut decision = AgentRunDecision::direct("executor");
        let long_prompt = "x".repeat(MAX_RUN_DECISION_QUERY_CHARS + 500);
        apply_default_memory_recall(&native, &mut decision, &long_prompt);
        assert_eq!(
            decision.memory.query.chars().count(),
            MAX_RUN_DECISION_QUERY_CHARS
        );
    }
}
