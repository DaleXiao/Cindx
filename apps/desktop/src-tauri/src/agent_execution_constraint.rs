use agent_core::Metadata;
use orchestrator::{AgentPolicy, AgentRunDecision};

pub(crate) const AGENT_EXECUTION_CONSTRAINT_KEY: &str = "execution_constraint";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AgentExecutionConstraint {
    #[default]
    Native,
    GroundedDirect,
}

impl AgentExecutionConstraint {
    pub(crate) const fn is_grounded_direct(self) -> bool {
        matches!(self, Self::GroundedDirect)
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::GroundedDirect => "grounded_direct",
        }
    }

    pub(crate) fn write_to_context(self, run_context: &mut Metadata) {
        match self {
            Self::Native => {
                run_context.remove(AGENT_EXECUTION_CONSTRAINT_KEY);
            }
            Self::GroundedDirect => {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_is_metadata_compatible_and_grounded_direct_is_explicit() {
        let mut context = Metadata::new();
        AgentExecutionConstraint::Native.write_to_context(&mut context);
        assert!(context.get(AGENT_EXECUTION_CONSTRAINT_KEY).is_none());
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
}
