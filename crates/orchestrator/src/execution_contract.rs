use crate::{
    OrchestrationPolicy, PromptCommitStrategy, RoutingContext, TaskClass, WorkflowOutputKind,
    WorkflowPlanIr,
};
use serde::{Deserialize, Serialize};

pub const AUTO_COLLABORATION_MIN_UPLIFT_BPS: u16 = 3_000;
pub const AUTO_COLLABORATION_MIN_CONFIDENCE_BPS: u16 = 5_500;
pub const PRO_MIN_TEAM_UPLIFT_BPS: u16 = 250;

pub fn minimum_team_uplift_bps(effort: &str) -> u16 {
    if normalize_effort(effort) == "pro" {
        PRO_MIN_TEAM_UPLIFT_BPS
    } else {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConductorStopPolicy {
    FirstVerified,
    Quorum,
    Exhaustive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConductorFallbackPolicy {
    BestKnownResult,
    SinglePath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConductorExecutionContract {
    pub task_class: TaskClass,
    pub effort: String,
    pub policy: OrchestrationPolicy,
    pub expected_uplift_bps: u16,
    pub confidence_bps: u16,
    pub max_parallelism: usize,
    pub min_successful_branches: usize,
    pub verification_required: bool,
    pub terminal_model_call_reserve: usize,
    pub stop_policy: ConductorStopPolicy,
    pub fallback_policy: ConductorFallbackPolicy,
    #[serde(default)]
    pub min_team_uplift_bps: u16,
    #[serde(default)]
    pub min_distinct_contributions: usize,
    #[serde(default)]
    pub requires_synthesis: bool,
}

impl ConductorExecutionContract {
    pub fn from_routing(
        context: &RoutingContext,
        effort: &str,
        policy: OrchestrationPolicy,
    ) -> Self {
        let effort = normalize_effort(effort);
        let explicit_collaboration = matches!(policy, OrchestrationPolicy::BestOfN { .. });
        let mut uplift = u16::from(context.needs_multi_model) * 4_000
            + u16::from(context.parallelizable) * 1_800
            + u16::from(context.verification_required) * 700
            + u16::from(context.high_stakes) * 900
            + u16::from(context.needs_retrieval) * 350
            + u16::from(context.needs_tools && context.needs_retrieval) * 250
            + u16::from(context.estimated_steps >= 4) * 600
            + u16::from(context.complexity_score).saturating_mul(400);
        if context.latency_sensitive {
            uplift = uplift.saturating_sub(1_800);
        }
        if matches!(context.task_class, TaskClass::Browser | TaskClass::Computer) {
            uplift = uplift.saturating_sub(1_000);
        }
        if explicit_collaboration {
            uplift = uplift.max(3_500);
        }

        let confidence = 4_800u16
            .saturating_add(u16::from(context.needs_multi_model) * 1_400)
            .saturating_add(u16::from(context.parallelizable) * 700)
            .saturating_add(u16::from(context.high_stakes) * 400)
            .saturating_add(u16::from(context.complexity_score >= 4) * 500)
            .saturating_add(u16::from(explicit_collaboration) * 1_000)
            .min(10_000);

        let requested_parallelism = match policy {
            OrchestrationPolicy::BestOfN { candidates } => candidates.max(1),
            OrchestrationPolicy::PlanExecuteReview => 1,
            OrchestrationPolicy::Single | OrchestrationPolicy::AutoRouter => {
                usize::from(context.parallelizable) + 1
            }
        };
        let max_parallelism = requested_parallelism.clamp(1, 3);
        let stop_policy = match effort.as_str() {
            "fast" => ConductorStopPolicy::FirstVerified,
            "pro" => ConductorStopPolicy::Quorum,
            _ => ConductorStopPolicy::Quorum,
        };
        let terminal_model_call_reserve = match effort.as_str() {
            "fast" => 1,
            "pro" => 3,
            _ => 2,
        };
        let collaboration_path = !matches!(policy, OrchestrationPolicy::Single);
        let min_distinct_contributions = if collaboration_path
            && max_parallelism >= 2
            && (context.needs_multi_model || context.parallelizable)
        {
            2
        } else if collaboration_path
            && (context.high_stakes
                || context.verification_required
                || context.needs_tools
                || context.needs_retrieval
                || context.complexity_score >= 3)
        {
            1
        } else {
            0
        };
        let min_successful_branches = match stop_policy {
            ConductorStopPolicy::FirstVerified => 1,
            ConductorStopPolicy::Quorum | ConductorStopPolicy::Exhaustive => {
                min_distinct_contributions.max(1)
            }
        };
        let min_team_uplift_bps = minimum_team_uplift_bps(&effort);
        let requires_synthesis = min_distinct_contributions >= 2;

        Self {
            task_class: context.task_class.clone(),
            effort,
            policy,
            expected_uplift_bps: uplift.min(10_000),
            confidence_bps: confidence,
            max_parallelism,
            min_successful_branches,
            verification_required: context.verification_required || context.high_stakes,
            terminal_model_call_reserve,
            stop_policy,
            fallback_policy: if context.needs_tools || context.needs_retrieval {
                ConductorFallbackPolicy::BestKnownResult
            } else {
                ConductorFallbackPolicy::SinglePath
            },
            min_team_uplift_bps,
            min_distinct_contributions,
            requires_synthesis,
        }
    }

    pub fn should_auto_collaborate(&self) -> bool {
        self.expected_uplift_bps >= AUTO_COLLABORATION_MIN_UPLIFT_BPS
            && self.confidence_bps >= AUTO_COLLABORATION_MIN_CONFIDENCE_BPS
    }

    pub fn with_prompt_commit_strategy(mut self, strategy: PromptCommitStrategy) -> Self {
        let task_quorum = self.min_successful_branches.max(1);
        self.stop_policy = match (self.effort.as_str(), strategy) {
            ("fast", _) => ConductorStopPolicy::FirstVerified,
            ("pro", PromptCommitStrategy::Exhaustive) => ConductorStopPolicy::Exhaustive,
            ("pro", _) => ConductorStopPolicy::Quorum,
            (_, PromptCommitStrategy::Adaptive) => self.stop_policy,
            (_, PromptCommitStrategy::Quorum) => ConductorStopPolicy::Quorum,
            (_, PromptCommitStrategy::Exhaustive) => ConductorStopPolicy::Exhaustive,
        };
        self.min_successful_branches = match self.stop_policy {
            ConductorStopPolicy::FirstVerified => 1,
            ConductorStopPolicy::Quorum => task_quorum,
            ConductorStopPolicy::Exhaustive => {
                task_quorum.max(self.max_parallelism.saturating_mul(2).saturating_add(2) / 3)
            }
        };
        self
    }

    pub fn required_successes_for_layer(&self, branch_count: usize) -> usize {
        self.min_successful_branches.min(branch_count).max(1)
    }

    pub fn quorum_grace_ms(&self) -> u64 {
        match self.stop_policy {
            ConductorStopPolicy::FirstVerified => 100,
            ConductorStopPolicy::Quorum => 1_000,
            ConductorStopPolicy::Exhaustive => 20_000,
        }
    }

    pub fn validate_plan(&self, plan: &WorkflowPlanIr) -> Result<(), String> {
        if normalize_effort(&plan.effort) != self.effort {
            return Err("workflow effort does not match its execution contract".to_string());
        }
        let plan_policy = match plan.policy.as_str() {
            "direct" => Some(OrchestrationPolicy::Single),
            value => crate::parse_policy(value),
        };
        if plan_policy.as_ref().map(OrchestrationPolicy::label) != Some(self.policy.label()) {
            return Err("workflow policy does not match its execution contract".to_string());
        }
        let root_branches = plan
            .steps
            .iter()
            .take(plan.steps.len().saturating_sub(1))
            .filter(|step| step.access.is_empty())
            .count();
        if root_branches > self.max_parallelism {
            return Err(format!(
                "workflow has {root_branches} root branches but its execution contract allows {}",
                self.max_parallelism
            ));
        }
        if self.verification_required
            && plan.steps.len() > 1
            && !plan
                .steps
                .iter()
                .any(|step| step.contract.output_kind == WorkflowOutputKind::Verification)
        {
            return Err("workflow execution contract requires a verification path".to_string());
        }
        let contributors = plan
            .steps
            .iter()
            .take(plan.steps.len().saturating_sub(1))
            .filter(|step| {
                step.contract.output_kind != WorkflowOutputKind::Verification
                    && step.access.is_empty()
            })
            .collect::<Vec<_>>();
        let distinct_contributors = contributors
            .iter()
            .map(|step| step.independent_contribution_key())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if distinct_contributors < self.min_distinct_contributions {
            return Err(format!(
                "collaboration workflow has {distinct_contributors} independent contribution(s), but {} are required by the task",
                self.min_distinct_contributions
            ));
        }
        if self.requires_synthesis {
            let Some(final_step) = plan.steps.last() else {
                return Err("collaboration workflow requires a synthesis step".to_string());
            };
            if final_step.contract.output_kind != WorkflowOutputKind::Synthesis {
                return Err(
                    "collaboration workflow must end with an explicit synthesis contract"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("execution contract serialization failed: {error}"))
    }

    pub fn from_json(value: &str) -> Result<Self, String> {
        serde_json::from_str(value)
            .map_err(|error| format!("execution contract JSON is invalid: {error}"))
    }
}

fn normalize_effort(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "fast" => "fast",
        "pro" => "pro",
        _ => "auto",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdaptiveWorkflow, AdaptiveWorkflowStep, ModelCandidate, WorkflowBudget};
    use agent_core::ModelRole;

    fn context(prompt: &str) -> RoutingContext {
        RoutingContext::from_prompt(
            prompt,
            vec![ModelCandidate {
                name: "worker".to_string(),
                role: ModelRole::Executor,
                supports_tools: true,
                supports_vision: true,
                cost_tier: 1,
                latency_tier: 1,
            }],
        )
    }

    #[test]
    fn expected_uplift_keeps_simple_auto_direct_and_routes_parallel_work() {
        let simple = ConductorExecutionContract::from_routing(
            &context("What is a Rust enum?"),
            "auto",
            OrchestrationPolicy::Single,
        );
        assert!(!simple.should_auto_collaborate());

        let complex = ConductorExecutionContract::from_routing(
            &context("Investigate this production architecture in parallel, compare alternatives, and cross-check the root cause with evidence"),
            "auto",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        );
        assert!(complex.should_auto_collaborate());
        assert_eq!(complex.stop_policy, ConductorStopPolicy::Quorum);
        assert_eq!(complex.min_successful_branches, 2);
    }

    #[test]
    fn pro_commits_only_after_its_quality_gated_quorum() {
        let contract = ConductorExecutionContract::from_routing(
            &context("Investigate three independent hypotheses and verify the strongest answer"),
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        );

        assert_eq!(contract.stop_policy, ConductorStopPolicy::Quorum);
        assert_eq!(contract.max_parallelism, 3);
        assert_eq!(contract.min_successful_branches, 2);
        assert_eq!(contract.required_successes_for_layer(2), 2);
        assert_eq!(contract.required_successes_for_layer(3), 2);
        assert_eq!(contract.quorum_grace_ms(), 1_000);
        assert_eq!(contract.min_team_uplift_bps, PRO_MIN_TEAM_UPLIFT_BPS);
        assert_eq!(contract.min_distinct_contributions, 2);
        assert!(contract.requires_synthesis);
    }

    #[test]
    fn simple_pro_uses_quality_comparison_without_forcing_decorative_branches() {
        let contract = ConductorExecutionContract::from_routing(
            &context("What is a Rust enum?"),
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        );

        assert_eq!(contract.max_parallelism, 3);
        assert_eq!(contract.min_successful_branches, 1);
        assert_eq!(contract.min_distinct_contributions, 0);
        assert!(!contract.requires_synthesis);
        assert_eq!(contract.min_team_uplift_bps, PRO_MIN_TEAM_UPLIFT_BPS);
    }

    #[test]
    fn evolved_commit_strategy_controls_the_execution_contract() {
        let routing = context("Compare independent implementation alternatives in parallel");
        let auto = ConductorExecutionContract::from_routing(
            &routing,
            "auto",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Exhaustive);
        assert_eq!(auto.stop_policy, ConductorStopPolicy::Exhaustive);
        assert_eq!(auto.quorum_grace_ms(), 20_000);

        let pro = ConductorExecutionContract::from_routing(
            &routing,
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Quorum);
        assert_eq!(pro.stop_policy, ConductorStopPolicy::Quorum);
        assert_eq!(pro.quorum_grace_ms(), 1_000);

        let exhaustive_pro = ConductorExecutionContract::from_routing(
            &routing,
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Exhaustive);
        assert_eq!(exhaustive_pro.stop_policy, ConductorStopPolicy::Exhaustive);

        let fast = ConductorExecutionContract::from_routing(
            &routing,
            "fast",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Exhaustive);
        assert_eq!(fast.stop_policy, ConductorStopPolicy::FirstVerified);
        assert_eq!(fast.quorum_grace_ms(), 100);
    }

    #[test]
    fn contract_rejects_plan_that_exceeds_parallelism() {
        let routing = context("Compare independent implementation alternatives in parallel");
        let mut contract = ConductorExecutionContract::from_routing(
            &routing,
            "auto",
            OrchestrationPolicy::BestOfN { candidates: 2 },
        );
        contract.verification_required = false;
        let plan = WorkflowPlanIr::from_adaptive(
            "workflow",
            "compare",
            "auto",
            "best_of_n",
            "conductor",
            &AdaptiveWorkflow {
                steps: vec![
                    AdaptiveWorkflowStep {
                        id: "a".into(),
                        role: "worker".into(),
                        model: "worker-b".into(),
                        subtask: "a".into(),
                        access: vec![],
                    },
                    AdaptiveWorkflowStep {
                        id: "b".into(),
                        role: "worker".into(),
                        model: "worker".into(),
                        subtask: "b".into(),
                        access: vec![],
                    },
                    AdaptiveWorkflowStep {
                        id: "s".into(),
                        role: "synthesizer".into(),
                        model: "worker".into(),
                        subtask: "s".into(),
                        access: vec!["a".into(), "b".into()],
                    },
                ],
            },
            WorkflowBudget {
                max_steps: 3,
                max_models: 2,
                max_model_turns_per_step: 2,
                max_tool_calls_per_step: 1,
                max_output_tokens_per_step: 2_048,
            },
        );
        contract.max_parallelism = 2;
        assert!(contract.validate_plan(&plan).is_ok());

        let mut one_model_plan = plan.clone();
        one_model_plan.steps[0].model = "worker".into();
        assert!(contract.validate_plan(&one_model_plan).is_ok());

        let mut duplicate_plan = one_model_plan;
        duplicate_plan.steps[1].subtask = "a".into();
        let error = contract
            .validate_plan(&duplicate_plan)
            .expect_err("duplicate assignments are not independent contributions");
        assert!(error.contains("1 independent contribution"), "{error}");

        contract.max_parallelism = 1;
        assert!(contract.validate_plan(&plan).is_err());
    }
}
