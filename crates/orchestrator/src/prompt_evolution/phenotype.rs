use super::*;

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
