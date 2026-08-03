use super::PlannedAgentRun;
use agent_core::Metadata;
use orchestrator::{
    AgentExecutionMode, AgentRouteRequirements, AgentRunDecision, ModelCandidate,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentPlanningSource {
    FastDirect,
    DynamicConductor,
    DynamicConductorReplanned,
    CalibratedDirect,
    DegradedDirect,
    DegradedWorkflow,
}

impl AgentPlanningSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::FastDirect => "fast_direct",
            Self::DynamicConductor => "dynamic_conductor_v2",
            Self::DynamicConductorReplanned => "dynamic_conductor_replanned",
            Self::CalibratedDirect => "dynamic_conductor_calibrated_direct",
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

pub(super) fn selected_conductor_source(
    decision: &AgentRunDecision,
    attempted_models: usize,
) -> AgentPlanningSource {
    if decision.calibration_reason.is_some() {
        AgentPlanningSource::CalibratedDirect
    } else if attempted_models > 1 {
        AgentPlanningSource::DynamicConductorReplanned
    } else {
        AgentPlanningSource::DynamicConductor
    }
}

pub(super) fn route_decision_metadata(planned: &PlannedAgentRun) -> Metadata {
    [
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
            "decision_calibration".to_string(),
            planned
                .decision
                .calibration_reason
                .as_ref()
                .map(|_| "matched_evidence_direct")
                .unwrap_or_default()
                .to_string(),
        ),
    ]
    .into_iter()
    .collect()
}
