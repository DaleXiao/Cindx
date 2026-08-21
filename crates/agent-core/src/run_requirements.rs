use crate::model_candidate::ModelCandidate;
use crate::run_decision_enums::{AgentEffectAuthority, AgentToolRequirement};
use serde::{Deserialize, Serialize};

pub const AGENT_RUN_DECISION_SCHEMA: &str = "cindx.agent-run-decision.v1";
pub const MAX_RUN_DECISION_QUERY_CHARS: usize = 2_000;
pub const MAX_RUN_DECISION_RATIONALE_CHARS: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRiskLevel {
    Low,
    Elevated,
    High,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentRouteRequirements {
    pub minimum_tool_requirement: AgentToolRequirement,
    pub effect_authority: AgentEffectAuthority,
    pub image_input_required: bool,
}

impl AgentRouteRequirements {
    /// Validates a prepared run's scheduling facts against the runtime route
    /// requirements and the configured model capability pool. Expressed over
    /// plain fields so the check does not depend on the retired run-decision
    /// harness type.
    pub fn validate_decision(
        &self,
        primary_model: &str,
        tool_requirement: AgentToolRequirement,
        vision_required: bool,
        model_candidates: &[ModelCandidate],
    ) -> Result<(), String> {
        if self.effect_authority == AgentEffectAuthority::Forbidden
            && tool_requirement == AgentToolRequirement::Effects
        {
            return Err("run decision exceeds the prompt effect authority".to_string());
        }
        if self.effect_authority == AgentEffectAuthority::Required
            && tool_requirement != AgentToolRequirement::Effects
        {
            return Err("run decision omitted required effect authority".to_string());
        }
        if !tool_requirement.satisfies(self.minimum_tool_requirement) {
            return Err(format!(
                "run decision tool requirement {} is below the runtime minimum {}",
                tool_requirement.label(),
                self.minimum_tool_requirement.label(),
            ));
        }
        if self.image_input_required && !vision_required {
            return Err("run decision omitted vision for the active image input".to_string());
        }

        if tool_requirement != AgentToolRequirement::None
            && !model_candidates.iter().any(|candidate| {
                candidate.name.trim() == primary_model.trim() && candidate.supports_tools
            })
        {
            return Err(format!(
                "selected model {} is not configured with tool capability",
                primary_model
            ));
        }
        if vision_required
            && !model_candidates.iter().any(|candidate| {
                candidate.name.trim() == primary_model.trim() && candidate.supports_vision
            })
        {
            return Err(format!(
                "selected model {} is not configured with vision capability",
                primary_model
            ));
        }
        Ok(())
    }

    pub fn model_satisfies(&self, model: &str, model_candidates: &[ModelCandidate]) -> bool {
        let needs_tools = self.minimum_tool_requirement != AgentToolRequirement::None;
        (!needs_tools
            || model_candidates
                .iter()
                .any(|candidate| candidate.name.trim() == model.trim() && candidate.supports_tools))
            && (!self.image_input_required
                || model_candidates.iter().any(|candidate| {
                    candidate.name.trim() == model.trim() && candidate.supports_vision
                }))
    }
}
