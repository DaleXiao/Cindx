use crate::{AgentExecutionMode, AgentRunDecision, AgentVerificationPolicy, ConductorStopPolicy};

impl AgentRunDecision {
    /// Constrains an already-routed Auto decision to the single-agent product path.
    ///
    /// Model, tool, retrieval, memory, vision, risk, task-class, and estimation
    /// decisions remain unchanged so provider-backed evaluation isolates workflow
    /// collaboration instead of silently changing the surrounding harness.
    pub fn constrained_to_grounded_direct(mut self) -> Self {
        self.execution = AgentExecutionMode::Direct;
        self.verification = match self.verification {
            AgentVerificationPolicy::Independent => AgentVerificationPolicy::SelfCheck,
            verification => verification,
        };
        self.max_parallelism = 1;
        self.min_successful_branches = 1;
        self.distinct_contributions = 0;
        self.expected_uplift_bps = 0;
        self.stop_policy = ConductorStopPolicy::FirstVerified;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentRiskLevel, AgentRouteTier, AgentToolRequirement, MemoryRecallPlan, MemoryRecallPolicy,
        TaskClass, WorkspaceRetrievalChannel, WorkspaceRetrievalPlan,
    };

    #[test]
    fn grounded_direct_preserves_the_routed_execution_surface() {
        let mut workflow = AgentRunDecision::direct("executor");
        workflow.task_class = TaskClass::Coding;
        workflow.execution = AgentExecutionMode::Workflow;
        workflow.tool_requirement = AgentToolRequirement::Effects;
        workflow.vision_required = true;
        workflow.risk_level = AgentRiskLevel::High;
        workflow.retrieval = WorkspaceRetrievalPlan {
            query: "workspace implementation evidence".to_string(),
            channels: [WorkspaceRetrievalChannel::Semantic].into_iter().collect(),
            max_results: 6,
        };
        workflow.memory = MemoryRecallPlan {
            policy: MemoryRecallPolicy::Relevant,
            query: "prior project constraints".to_string(),
        };
        workflow.verification = AgentVerificationPolicy::Independent;
        workflow.max_parallelism = 2;
        workflow.min_successful_branches = 2;
        workflow.distinct_contributions = 2;
        workflow.estimated_steps = 4;
        workflow.expected_uplift_bps = 6_000;
        workflow.confidence_bps = 8_000;
        workflow.stop_policy = ConductorStopPolicy::Quorum;

        let constrained = workflow.clone().constrained_to_grounded_direct();

        assert_eq!(constrained.execution, AgentExecutionMode::Direct);
        assert_eq!(constrained.primary_model, workflow.primary_model);
        assert_eq!(constrained.task_class, workflow.task_class);
        assert_eq!(constrained.tool_requirement, workflow.tool_requirement);
        assert_eq!(constrained.retrieval, workflow.retrieval);
        assert_eq!(constrained.memory, workflow.memory);
        assert_eq!(constrained.vision_required, workflow.vision_required);
        assert_eq!(constrained.risk_level, workflow.risk_level);
        assert_eq!(constrained.estimated_steps, workflow.estimated_steps);
        assert_eq!(constrained.rationale, workflow.rationale);
        assert_eq!(constrained.verification, AgentVerificationPolicy::SelfCheck);
        assert_eq!(constrained.max_parallelism, 1);
        assert_eq!(constrained.min_successful_branches, 1);
        assert_eq!(constrained.distinct_contributions, 0);
        assert_eq!(constrained.expected_uplift_bps, 0);
        assert_eq!(constrained.stop_policy, ConductorStopPolicy::FirstVerified);
        assert_eq!(constrained.route_tier(), AgentRouteTier::GroundedDirect);
        constrained
            .validate(&["executor".to_string()])
            .expect("the constrained decision should satisfy direct invariants");
    }
}
