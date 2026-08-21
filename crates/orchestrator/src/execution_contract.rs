//! The execution-contract types live in `agent-core`; this module keeps the
//! routing-context-derived construction and its contract tests.
pub use agent_core::execution_contract::{
    minimum_team_uplift_bps, minimum_workflow_steps, normalize_effort, ConductorExecutionContract,
    ConductorFallbackPolicy, ConductorStopPolicy, AUTO_COLLABORATION_MIN_CONFIDENCE_BPS,
    AUTO_COLLABORATION_MIN_UPLIFT_BPS, MAX_PLANNING_STEPS, PRO_MIN_TEAM_UPLIFT_BPS,
};

use crate::{OrchestrationPolicy, RoutingContext};
use agent_core::TaskClass;

pub fn execution_contract_from_routing(
    context: &RoutingContext,
    effort: &str,
    policy: OrchestrationPolicy,
) -> ConductorExecutionContract {
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
    let verification_required = context.verification_required || context.high_stakes;
    let min_team_uplift_bps = minimum_team_uplift_bps(&effort);
    let requires_synthesis = min_distinct_contributions >= 2;
    let minimum_workflow_steps =
        minimum_workflow_steps(min_distinct_contributions, verification_required);
    let max_workflow_steps = usize::from(context.estimated_steps)
        .max(minimum_workflow_steps)
        .clamp(1, MAX_PLANNING_STEPS);

    ConductorExecutionContract {
        task_class: context.task_class.clone(),
        effort,
        policy,
        expected_uplift_bps: uplift.min(10_000),
        confidence_bps: confidence,
        max_parallelism,
        max_workflow_steps,
        min_successful_branches,
        verification_required,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelCandidate, ModelCapabilitySource};
    use agent_core::execution_contract::ConductorExecutionContract as Contract;
    use agent_core::ModelRole;
    use agent_core::PromptCommitStrategy;

    fn context(prompt: &str) -> RoutingContext {
        RoutingContext::from_prompt(
            prompt,
            vec![ModelCandidate {
                name: "worker".to_string(),
                role: ModelRole::Executor,
                supports_tools: true,
                supports_vision: true,
                tools_capability_source: ModelCapabilitySource::Configured,
                vision_capability_source: ModelCapabilitySource::Configured,
                cost_tier: 1,
                latency_tier: 1,
            }],
        )
    }

    #[test]
    fn expected_uplift_keeps_simple_auto_direct_and_routes_parallel_work() {
        let simple = execution_contract_from_routing(
            &context("What is a Rust enum?"),
            "auto",
            OrchestrationPolicy::Single,
        );
        assert!(!simple.should_auto_collaborate());

        let complex = execution_contract_from_routing(
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
        let contract = execution_contract_from_routing(
            &context("Investigate three independent hypotheses and verify the strongest answer"),
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        );

        assert_eq!(contract.stop_policy, ConductorStopPolicy::Quorum);
        assert_eq!(contract.max_parallelism, 3);
        assert_eq!(contract.max_workflow_steps, 4);
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
        let contract = execution_contract_from_routing(
            &context("What is a Rust enum?"),
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        );

        assert_eq!(contract.max_parallelism, 3);
        assert_eq!(contract.max_workflow_steps, 1);
        assert_eq!(contract.min_successful_branches, 1);
        assert_eq!(contract.min_distinct_contributions, 0);
        assert!(!contract.requires_synthesis);
        assert_eq!(contract.min_team_uplift_bps, PRO_MIN_TEAM_UPLIFT_BPS);
    }

    #[test]
    fn legacy_contract_without_task_step_limit_keeps_the_outer_ceiling() {
        let contract = execution_contract_from_routing(
            &context("Compare two independent implementation strategies"),
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 2 },
        );
        let mut value = serde_json::to_value(contract).unwrap();
        value.as_object_mut().unwrap().remove("max_workflow_steps");

        let restored: Contract = serde_json::from_value(value).unwrap();
        assert_eq!(restored.max_workflow_steps, MAX_PLANNING_STEPS);
    }

    #[test]
    fn evolved_commit_strategy_controls_the_execution_contract() {
        let routing = context("Compare independent implementation alternatives in parallel");
        let auto = execution_contract_from_routing(
            &routing,
            "auto",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Exhaustive);
        assert_eq!(auto.stop_policy, ConductorStopPolicy::Exhaustive);
        assert_eq!(auto.quorum_grace_ms(), 20_000);

        let pro = execution_contract_from_routing(
            &routing,
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Quorum);
        assert_eq!(pro.stop_policy, ConductorStopPolicy::Quorum);
        assert_eq!(pro.quorum_grace_ms(), 1_000);

        let exhaustive_pro = execution_contract_from_routing(
            &routing,
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Exhaustive);
        assert_eq!(exhaustive_pro.stop_policy, ConductorStopPolicy::Exhaustive);

        let fast = execution_contract_from_routing(
            &routing,
            "fast",
            OrchestrationPolicy::BestOfN { candidates: 3 },
        )
        .with_prompt_commit_strategy(PromptCommitStrategy::Exhaustive);
        assert_eq!(fast.stop_policy, ConductorStopPolicy::FirstVerified);
        assert_eq!(fast.quorum_grace_ms(), 100);
    }
}
