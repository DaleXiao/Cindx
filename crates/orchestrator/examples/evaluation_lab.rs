use agent_core::ModelRole;
use orchestrator::{
    evaluate_routing_cases, ModelCandidate, OrchestrationPolicy, RoutingContext, RoutingEvalCase,
};

fn candidates() -> Vec<ModelCandidate> {
    vec![
        ModelCandidate {
            name: "default-fast".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 1,
            latency_tier: 1,
        },
        ModelCandidate {
            name: "planner-strong".to_string(),
            role: ModelRole::Planner,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 3,
            latency_tier: 2,
        },
        ModelCandidate {
            name: "executor-strong".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 3,
            latency_tier: 2,
        },
        ModelCandidate {
            name: "reviewer-strong".to_string(),
            role: ModelRole::Reviewer,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 4,
            latency_tier: 3,
        },
    ]
}

fn case(
    id: &str,
    prompt: &str,
    expected_policy: OrchestrationPolicy,
    expected_retrieval_mode: &str,
    expected_model: &str,
) -> RoutingEvalCase {
    RoutingEvalCase {
        id: id.to_string(),
        context: RoutingContext::from_prompt(prompt, candidates()),
        expected_policy,
        expected_retrieval_mode: expected_retrieval_mode.to_string(),
        expected_model: Some(expected_model.to_string()),
    }
}

fn main() {
    let cases = vec![
        case(
            "greeting-direct",
            "hello",
            OrchestrationPolicy::Single,
            "none",
            "default-fast",
        ),
        case(
            "coding-explanation-direct",
            "Explain Rust ownership",
            OrchestrationPolicy::Single,
            "none",
            "default-fast",
        ),
        case(
            "coding-workflow",
            "修复这个项目并运行测试",
            OrchestrationPolicy::PlanExecuteReview,
            "semantic_literal_parallel",
            "executor-strong",
        ),
        case(
            "browser-workflow",
            "Use the browser to fill this form",
            OrchestrationPolicy::PlanExecuteReview,
            "none",
            "executor-strong",
        ),
        case(
            "ordinary-comparison",
            "比较两个产品路线的优缺点",
            OrchestrationPolicy::PlanExecuteReview,
            "none",
            "planner-strong",
        ),
        case(
            "latency-sensitive-comparison",
            "Quickly compare implementation alternatives in this project",
            OrchestrationPolicy::PlanExecuteReview,
            "four_way_parallel",
            "planner-strong",
        ),
        case(
            "high-stakes-ultra",
            "Compare production migration architectures, investigate root causes, and propose a safe strategy",
            OrchestrationPolicy::BestOfN { candidates: 3 },
            "none",
            "reviewer-strong",
        ),
        case(
            "explicit-fugu-ultra",
            "Use multiple models to reproduce Fugu Ultra",
            OrchestrationPolicy::BestOfN { candidates: 3 },
            "none",
            "planner-strong",
        ),
        case(
            "bounded-two-worker",
            "Investigate the root cause in this project, edit the files, and run tests",
            OrchestrationPolicy::BestOfN { candidates: 2 },
            "four_way_parallel",
            "planner-strong",
        ),
        case(
            "grounded-retrieval",
            "Search the docs with RAG and cite sources",
            OrchestrationPolicy::PlanExecuteReview,
            "four_way_parallel",
            "planner-strong",
        ),
    ];
    let report = evaluate_routing_cases(&cases);
    println!(
        "Cindx routing evaluation: {}/{} passed ({:.1}%), over={}, under={}",
        report.passed,
        report.cases,
        report.pass_rate() * 100.0,
        report.over_orchestrated,
        report.under_orchestrated
    );
    for failure in &report.failures {
        eprintln!("FAIL: {failure}");
    }
    if !report.failures.is_empty() {
        std::process::exit(1);
    }
}
