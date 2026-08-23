use crate::agent_effort_planner::KnowledgeDecision;
use agent_core::{MemoryRecallPolicy, Metadata};

pub(crate) const AGENT_MEMORY_EVALUATION_CONSTRAINT_KEY: &str = "memory_evaluation_constraint";
pub(crate) const ROUTED_MEMORY_POLICY_KEY: &str = "routed_memory_policy";
pub(crate) const EFFECTIVE_MEMORY_POLICY_KEY: &str = "effective_memory_policy";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AgentMemoryEvaluationConstraint {
    #[default]
    Native,
    MemoryOn,
    MemoryOff,
}

impl AgentMemoryEvaluationConstraint {
    pub(crate) const fn is_native(self) -> bool {
        matches!(self, Self::Native)
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::MemoryOn => "memory_on",
            Self::MemoryOff => "memory_off",
        }
    }

    pub(crate) fn write_to_context(self, run_context: &mut Metadata) {
        match self {
            Self::Native => {
                run_context.remove(AGENT_MEMORY_EVALUATION_CONSTRAINT_KEY);
            }
            Self::MemoryOn | Self::MemoryOff => {
                run_context.insert(
                    AGENT_MEMORY_EVALUATION_CONSTRAINT_KEY.to_string(),
                    self.label().to_string(),
                );
            }
        }
    }

    pub(crate) fn from_context(run_context: &Metadata) -> Result<Self, String> {
        match run_context
            .get(AGENT_MEMORY_EVALUATION_CONSTRAINT_KEY)
            .map(String::as_str)
        {
            None => Ok(Self::Native),
            Some("memory_on") => Ok(Self::MemoryOn),
            Some("memory_off") => Ok(Self::MemoryOff),
            Some(value) => Err(format!(
                "unsupported agent memory evaluation constraint {value}"
            )),
        }
    }

    /// Applies the ablation after routing so every non-memory decision remains
    /// the product decision. This constraint is not exposed through product
    /// ingress and therefore cannot disable memory for ordinary runs.
    pub(crate) fn apply_after_routing(
        self,
        run_context: &mut Metadata,
        routed: &KnowledgeDecision,
    ) -> KnowledgeDecision {
        debug_assert!(!self.is_native());
        let routed_policy = memory_policy_label(routed.memory_policy);
        let mut effective = routed.clone();
        if self == Self::MemoryOff {
            // The workspace query and retrieval flag ride along, so turning memory
            // off ablates memory recall without touching workspace knowledge.
            effective.memory_policy = MemoryRecallPolicy::None;
        }
        run_context.insert(
            ROUTED_MEMORY_POLICY_KEY.to_string(),
            routed_policy.to_string(),
        );
        run_context.insert(
            EFFECTIVE_MEMORY_POLICY_KEY.to_string(),
            memory_policy_label(effective.memory_policy).to_string(),
        );
        effective
    }
}

fn memory_policy_label(policy: MemoryRecallPolicy) -> &'static str {
    match policy {
        MemoryRecallPolicy::None => "none",
        MemoryRecallPolicy::Relevant => "relevant",
        MemoryRecallPolicy::Comprehensive => "comprehensive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_off_changes_only_the_routed_memory_plan() {
        let routed = KnowledgeDecision {
            memory_policy: MemoryRecallPolicy::Relevant,
            memory_query: "durable release constraint".to_string(),
            retrieve_workspace: false,
            workspace_max_results: 8,
        };
        let mut context = Metadata::new();
        AgentMemoryEvaluationConstraint::MemoryOff.write_to_context(&mut context);

        let effective =
            AgentMemoryEvaluationConstraint::MemoryOff.apply_after_routing(&mut context, &routed);

        let mut expected = routed.clone();
        expected.memory_policy = MemoryRecallPolicy::None;
        assert_eq!(effective, expected);
        assert_eq!(context.get(ROUTED_MEMORY_POLICY_KEY).unwrap(), "relevant");
        assert_eq!(context.get(EFFECTIVE_MEMORY_POLICY_KEY).unwrap(), "none");
    }

    #[test]
    fn memory_off_preserves_workspace_retrieval() {
        let routed = KnowledgeDecision {
            memory_policy: MemoryRecallPolicy::Relevant,
            memory_query: "workspace implementation evidence".to_string(),
            retrieve_workspace: true,
            workspace_max_results: 8,
        };
        let mut context = Metadata::new();

        let effective =
            AgentMemoryEvaluationConstraint::MemoryOff.apply_after_routing(&mut context, &routed);

        assert_eq!(effective.memory_policy, MemoryRecallPolicy::None);
        assert!(!effective.memory_enabled());
        assert!(effective.retrieve_workspace);
        assert!(effective.workspace_plan().is_some());
    }

    #[test]
    fn memory_on_preserves_routed_none_without_forcing_recall() {
        let routed = KnowledgeDecision::none();
        let mut context = Metadata::new();
        AgentMemoryEvaluationConstraint::MemoryOn.write_to_context(&mut context);

        let effective =
            AgentMemoryEvaluationConstraint::MemoryOn.apply_after_routing(&mut context, &routed);

        assert_eq!(effective, routed);
        assert_eq!(context.get(ROUTED_MEMORY_POLICY_KEY).unwrap(), "none");
        assert_eq!(context.get(EFFECTIVE_MEMORY_POLICY_KEY).unwrap(), "none");
    }
}
