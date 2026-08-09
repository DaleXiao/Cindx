use agent_core::Metadata;
use orchestrator::{
    AgentExecutionMode, AgentPolicy, AgentRunDecision, AgentToolRequirement, MemoryRecallPlan,
    MemoryRecallPolicy,
};

pub(crate) const AGENT_EXECUTION_CONSTRAINT_KEY: &str = "execution_constraint";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AgentExecutionConstraint {
    #[default]
    Native,
    GroundedDirect,
    MatchedMemoryEffect,
    MatchedDirect,
    MatchedWorkflow,
}

impl AgentExecutionConstraint {
    pub(crate) const fn is_grounded_direct(self) -> bool {
        matches!(self, Self::GroundedDirect)
    }

    pub(crate) const fn is_matched_memory_effect(self) -> bool {
        matches!(self, Self::MatchedMemoryEffect)
    }

    pub(crate) const fn is_matched_route(self) -> bool {
        matches!(self, Self::MatchedDirect | Self::MatchedWorkflow)
    }

    pub(crate) const fn required_execution(self) -> Option<AgentExecutionMode> {
        match self {
            Self::MatchedDirect => Some(AgentExecutionMode::Direct),
            Self::MatchedWorkflow => Some(AgentExecutionMode::Workflow),
            Self::Native | Self::GroundedDirect | Self::MatchedMemoryEffect => None,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::GroundedDirect => "grounded_direct",
            Self::MatchedMemoryEffect => "matched_memory_effect",
            Self::MatchedDirect => "matched_direct",
            Self::MatchedWorkflow => "matched_workflow",
        }
    }

    pub(crate) fn write_to_context(self, run_context: &mut Metadata) {
        match self {
            Self::Native => {
                run_context.remove(AGENT_EXECUTION_CONSTRAINT_KEY);
            }
            Self::GroundedDirect
            | Self::MatchedMemoryEffect
            | Self::MatchedDirect
            | Self::MatchedWorkflow => {
                run_context.insert(
                    AGENT_EXECUTION_CONSTRAINT_KEY.to_string(),
                    self.label().to_string(),
                );
            }
        }
    }

    pub(crate) fn from_context(run_context: &Metadata) -> Result<Self, String> {
        match run_context
            .get(AGENT_EXECUTION_CONSTRAINT_KEY)
            .map(String::as_str)
        {
            None => Ok(Self::Native),
            Some("grounded_direct") => Ok(Self::GroundedDirect),
            Some("matched_memory_effect") => Ok(Self::MatchedMemoryEffect),
            Some("matched_direct") => Ok(Self::MatchedDirect),
            Some("matched_workflow") => Ok(Self::MatchedWorkflow),
            Some(value) => Err(format!("unsupported agent execution constraint {value}")),
        }
    }

    pub(crate) fn apply(
        self,
        decision: AgentRunDecision,
        effort: AgentPolicy,
    ) -> Result<AgentRunDecision, String> {
        match self {
            Self::Native => Ok(decision),
            Self::GroundedDirect if effort != AgentPolicy::Auto => {
                Err("grounded-direct execution requires the Auto policy and budget".to_string())
            }
            Self::GroundedDirect => Ok(decision.constrained_to_grounded_direct()),
            Self::MatchedMemoryEffect if effort != AgentPolicy::Auto => {
                Err("matched-memory evaluation requires the Auto policy and budget".to_string())
            }
            Self::MatchedMemoryEffect => {
                let mut matched = AgentRunDecision::direct(decision.primary_model);
                matched.tool_requirement = AgentToolRequirement::ReadOnly;
                matched.memory = MemoryRecallPlan {
                    policy: MemoryRecallPolicy::Relevant,
                    query: "durable project requirements and prior-session facts".to_string(),
                };
                matched.rationale =
                    "fixed matched-memory evaluation route for causal attribution".to_string();
                Ok(matched)
            }
            Self::MatchedDirect | Self::MatchedWorkflow if effort != AgentPolicy::Pro => {
                Err("matched route evaluation requires the Pro policy and budget".to_string())
            }
            Self::MatchedDirect | Self::MatchedWorkflow => {
                let required = self
                    .required_execution()
                    .expect("matched route constraints have a required execution mode");
                if decision.execution != required {
                    return Err(format!(
                        "matched route evaluation required {} but conductor returned {}",
                        execution_mode_label(required),
                        execution_mode_label(decision.execution),
                    ));
                }
                Ok(decision)
            }
        }
    }
}

const fn execution_mode_label(mode: AgentExecutionMode) -> &'static str {
    match mode {
        AgentExecutionMode::Direct => "direct",
        AgentExecutionMode::Workflow => "workflow",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_is_metadata_compatible_and_grounded_direct_is_explicit() {
        let mut context = Metadata::new();
        AgentExecutionConstraint::Native.write_to_context(&mut context);
        assert!(!context.contains_key(AGENT_EXECUTION_CONSTRAINT_KEY));
        assert_eq!(
            AgentExecutionConstraint::from_context(&context),
            Ok(AgentExecutionConstraint::Native)
        );

        AgentExecutionConstraint::GroundedDirect.write_to_context(&mut context);
        assert_eq!(
            context
                .get(AGENT_EXECUTION_CONSTRAINT_KEY)
                .map(String::as_str),
            Some("grounded_direct")
        );
        assert_eq!(
            AgentExecutionConstraint::from_context(&context),
            Ok(AgentExecutionConstraint::GroundedDirect)
        );

        context.insert(
            AGENT_EXECUTION_CONSTRAINT_KEY.to_string(),
            "unknown".to_string(),
        );
        assert!(AgentExecutionConstraint::from_context(&context).is_err());
    }

    #[test]
    fn grounded_direct_is_available_only_with_the_auto_budget() {
        let decision = AgentRunDecision::direct("executor");
        assert_eq!(
            AgentExecutionConstraint::Native
                .apply(decision.clone(), AgentPolicy::Pro)
                .expect("native execution should remain unchanged"),
            decision
        );
        assert!(AgentExecutionConstraint::GroundedDirect
            .apply(decision.clone(), AgentPolicy::Fast)
            .is_err());
        assert!(AgentExecutionConstraint::GroundedDirect
            .apply(decision.clone(), AgentPolicy::Pro)
            .is_err());
        assert!(AgentExecutionConstraint::GroundedDirect
            .apply(decision, AgentPolicy::Auto)
            .is_ok());
    }

    #[test]
    fn matched_memory_effect_canonicalizes_the_route_before_ablation() {
        let mut first = AgentRunDecision::direct("executor");
        first.rationale = "provider-varying rationale".to_string();
        first.expected_uplift_bps = 9_000;
        let mut second = AgentRunDecision::direct("executor");
        second.rationale = "different provider rationale".to_string();
        second.max_parallelism = 4;
        let first = AgentExecutionConstraint::MatchedMemoryEffect
            .apply(first, AgentPolicy::Auto)
            .expect("matched-memory route");
        let second = AgentExecutionConstraint::MatchedMemoryEffect
            .apply(second, AgentPolicy::Auto)
            .expect("matched-memory route");

        assert_eq!(first, second);
        assert_eq!(first.primary_model, "executor");
        assert_eq!(first.tool_requirement, AgentToolRequirement::ReadOnly);
        assert_eq!(first.memory.policy, MemoryRecallPolicy::Relevant);
        assert_eq!(
            first.memory.query,
            "durable project requirements and prior-session facts"
        );
        assert!(AgentExecutionConstraint::MatchedMemoryEffect
            .apply(AgentRunDecision::direct("executor"), AgentPolicy::Pro)
            .is_err());
    }

    #[test]
    fn matched_route_constraints_are_pro_only_and_validate_without_rewriting() {
        let direct = AgentRunDecision::direct("executor");
        assert_eq!(
            AgentExecutionConstraint::MatchedDirect
                .apply(direct.clone(), AgentPolicy::Pro)
                .expect("matched direct should accept a direct conductor decision"),
            direct
        );
        assert!(AgentExecutionConstraint::MatchedDirect
            .apply(AgentRunDecision::direct("executor"), AgentPolicy::Auto)
            .is_err());
        assert!(AgentExecutionConstraint::MatchedWorkflow
            .apply(AgentRunDecision::direct("executor"), AgentPolicy::Pro)
            .is_err());
    }
}
