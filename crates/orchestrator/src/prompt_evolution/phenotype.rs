use super::*;
use serde::Serialize;

pub const DIRECT_FINALIZER_PHENOTYPE_SCHEMA: &str = "cindx.prompt-direct-finalizer-phenotype.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DirectFinalizerPromptPhenotype {
    pub schema: &'static str,
    pub verification: PromptVerification,
}

impl DirectFinalizerPromptPhenotype {
    pub fn directive(self) -> Option<&'static str> {
        match self.verification {
            PromptVerification::Evidence => None,
            PromptVerification::Minimal => Some(
                "Use a minimal final check: preserve the strongest grounded result, ensure the requested deliverable is present, and do not add new analysis or unsupported claims.",
            ),
            PromptVerification::Adversarial => Some(
                "Before committing the final answer, challenge it against the visible evidence and every user constraint. Correct contradictions, unsupported claims, omitted deliverables, and premature completion, but do not start new work, call tools, or claim evidence that is not present.",
            ),
        }
    }

    pub fn sha256(self) -> Result<String, String> {
        serde_json::to_vec(&self)
            .map(|encoded| crate::sha256_hex(&encoded))
            .map_err(|error| format!("direct finalizer phenotype serialization failed: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PromptGenomeExecutionPhenotype {
    graph_depth: PromptGraphDepth,
    verification: PromptVerification,
    context_policy: PromptContextPolicy,
    max_parallel_branches: usize,
    tool_policy: PromptToolPolicy,
    retry_policy: PromptRetryPolicy,
    topology_strategy: PromptTopologyStrategy,
    role_strategy: PromptRoleStrategy,
    commit_strategy: PromptCommitStrategy,
    max_step_attempts: usize,
    max_model_turns_per_step: usize,
    max_tool_calls_per_step: usize,
    require_final_synthesis: bool,
    custom_directive: String,
}

impl ConductorPromptGenome {
    pub fn direct_finalizer_phenotype(&self) -> DirectFinalizerPromptPhenotype {
        DirectFinalizerPromptPhenotype {
            schema: DIRECT_FINALIZER_PHENOTYPE_SCHEMA,
            verification: self.direct_finalizer_verification,
        }
    }

    pub(super) fn normalize_behavioral_budgets(&mut self) {
        self.max_model_turns_per_step = self.effective_max_model_turns_per_step();
        self.max_tool_calls_per_step = self.effective_max_tool_calls_per_step();
    }

    pub(super) fn execution_phenotype(&self) -> PromptGenomeExecutionPhenotype {
        PromptGenomeExecutionPhenotype {
            graph_depth: self.graph_depth,
            verification: self.verification,
            context_policy: self.context_policy,
            max_parallel_branches: self.max_parallel_branches,
            tool_policy: self.tool_policy,
            retry_policy: self.retry_policy,
            topology_strategy: self.topology_strategy,
            role_strategy: self.role_strategy,
            commit_strategy: self.commit_strategy,
            max_step_attempts: self.max_step_attempts,
            max_model_turns_per_step: self.effective_max_model_turns_per_step(),
            max_tool_calls_per_step: self.effective_max_tool_calls_per_step(),
            require_final_synthesis: self.require_final_synthesis,
            custom_directive: self.custom_directive.trim().to_string(),
        }
    }
}

#[cfg(test)]
mod direct_finalizer_tests {
    use super::*;

    #[test]
    fn legacy_genomes_keep_the_byte_stable_default_surface() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let encoded = serde_json::to_string(&genome).expect("seed should serialize");
        assert!(!encoded.contains("direct_finalizer_verification"));

        let decoded = serde_json::from_str::<ConductorPromptGenome>(&encoded)
            .expect("legacy-compatible seed should deserialize");
        assert_eq!(
            decoded.direct_finalizer_phenotype().verification,
            PromptVerification::Evidence
        );
        assert_eq!(decoded.direct_finalizer_phenotype().directive(), None);
    }

    #[test]
    fn direct_finalizer_is_orthogonal_to_workflow_genes() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut workflow_variant = parent.clone();
        workflow_variant.graph_depth = PromptGraphDepth::Deep;
        workflow_variant.verification = PromptVerification::Adversarial;
        workflow_variant.context_policy = PromptContextPolicy::Comprehensive;
        assert_eq!(
            parent.direct_finalizer_phenotype(),
            workflow_variant.direct_finalizer_phenotype()
        );

        let mut direct_variant = parent.clone();
        direct_variant.direct_finalizer_verification = PromptVerification::Adversarial;
        assert_ne!(
            parent.direct_finalizer_phenotype(),
            direct_variant.direct_finalizer_phenotype()
        );
        assert!(direct_variant
            .direct_finalizer_phenotype()
            .directive()
            .is_some());
    }
}
