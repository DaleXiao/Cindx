use super::{
    LearnedModelRouter, OrchestrationPolicy, RoutingContext, RoutingOutcome, RoutingTelemetry,
    RuleBasedRouter,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingEvaluationReport {
    pub examples: usize,
    pub baseline_policy: String,
    pub router_policy_matches_baseline: usize,
    pub router_policy_differs_from_baseline: usize,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingEvalCase {
    pub id: String,
    pub context: RoutingContext,
    pub expected_policy: OrchestrationPolicy,
    pub expected_retrieval_mode: String,
    pub expected_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingBenchmarkReport {
    pub cases: usize,
    pub passed: usize,
    pub over_orchestrated: usize,
    pub under_orchestrated: usize,
    pub failures: Vec<String>,
}

impl RoutingBenchmarkReport {
    pub fn pass_rate(&self) -> f32 {
        if self.cases == 0 {
            1.0
        } else {
            self.passed as f32 / self.cases as f32
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperationalEvaluationReport {
    pub runs: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub user_rejected: usize,
    pub success_rate: f32,
    pub average_latency_ms: u64,
    pub average_cost_proxy: u64,
    pub average_tool_calls: f32,
    pub average_retrievals: f32,
    pub policy_counts: BTreeMap<String, usize>,
    pub model_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityRubricScore {
    pub correctness: u8,
    pub evidence: u8,
    pub completion: u8,
    pub safety: u8,
}

impl QualityRubricScore {
    pub fn validate(&self) -> Result<(), String> {
        if [
            self.correctness,
            self.evidence,
            self.completion,
            self.safety,
        ]
        .into_iter()
        .all(|score| score <= 5)
        {
            Ok(())
        } else {
            Err("quality rubric dimensions must use a 0-5 score".to_string())
        }
    }

    pub fn total(&self) -> u8 {
        self.correctness
            .saturating_add(self.evidence)
            .saturating_add(self.completion)
            .saturating_add(self.safety)
    }

    pub fn passes(&self) -> bool {
        self.validate().is_ok()
            && self.total() >= 15
            && self.correctness >= 3
            && self.evidence >= 3
            && self.completion >= 3
            && self.safety >= 3
    }
}

pub fn evaluate_router_against_baseline(
    router: &LearnedModelRouter,
    contexts: &[RoutingContext],
    baseline_policy: OrchestrationPolicy,
) -> RoutingEvaluationReport {
    let matches_baseline = contexts
        .iter()
        .filter(|context| router.route(context).policy.label() == baseline_policy.label())
        .count();
    let differs = contexts.len().saturating_sub(matches_baseline);

    RoutingEvaluationReport {
        examples: contexts.len(),
        baseline_policy: baseline_policy.label().to_string(),
        router_policy_matches_baseline: matches_baseline,
        router_policy_differs_from_baseline: differs,
        summary: format!(
            "Compared {} router decisions against baseline {}: {} same, {} different.",
            contexts.len(),
            baseline_policy.label(),
            matches_baseline,
            differs
        ),
    }
}

pub fn evaluate_routing_cases(cases: &[RoutingEvalCase]) -> RoutingBenchmarkReport {
    let router = RuleBasedRouter;
    let mut report = RoutingBenchmarkReport {
        cases: cases.len(),
        passed: 0,
        over_orchestrated: 0,
        under_orchestrated: 0,
        failures: Vec::new(),
    };
    for case in cases {
        let decision = router.route(&case.context);
        let policy_matches = decision.policy == case.expected_policy;
        let retrieval_matches = decision.retrieval_mode == case.expected_retrieval_mode;
        let model_matches = case
            .expected_model
            .as_ref()
            .map(|model| model == &decision.model)
            .unwrap_or(true);
        if policy_matches && retrieval_matches && model_matches {
            report.passed += 1;
            continue;
        }

        let actual_load = orchestration_load(&decision.policy);
        let expected_load = orchestration_load(&case.expected_policy);
        if actual_load > expected_load {
            report.over_orchestrated += 1;
        } else if actual_load < expected_load {
            report.under_orchestrated += 1;
        }
        report.failures.push(format!(
            "{} expected policy={} retrieval={} model={} but got policy={} retrieval={} model={}",
            case.id,
            case.expected_policy.label(),
            case.expected_retrieval_mode,
            case.expected_model.as_deref().unwrap_or("(any)"),
            decision.policy.label(),
            decision.retrieval_mode,
            decision.model
        ));
    }
    report
}

pub fn evaluate_routing_telemetry(telemetry: &[RoutingTelemetry]) -> OperationalEvaluationReport {
    let runs = telemetry.len();
    let succeeded = telemetry
        .iter()
        .filter(|entry| entry.outcome == RoutingOutcome::Succeeded)
        .count();
    let failed = telemetry
        .iter()
        .filter(|entry| entry.outcome == RoutingOutcome::Failed)
        .count();
    let user_rejected = telemetry
        .iter()
        .filter(|entry| entry.outcome == RoutingOutcome::UserRejected)
        .count();
    let total_latency = telemetry.iter().map(|entry| entry.latency_ms).sum::<u64>();
    let total_cost = telemetry.iter().map(|entry| entry.cost_proxy).sum::<u64>();
    let total_tools = telemetry.iter().map(|entry| entry.tool_count).sum::<u64>();
    let total_retrievals = telemetry
        .iter()
        .map(|entry| entry.retrieval_count)
        .sum::<u64>();
    let mut policy_counts = BTreeMap::new();
    let mut model_counts = BTreeMap::new();
    for entry in telemetry {
        *policy_counts
            .entry(entry.selected_policy.label().to_string())
            .or_default() += 1;
        *model_counts
            .entry(entry.selected_model.clone())
            .or_default() += 1;
    }
    let divisor = runs.max(1) as u64;
    OperationalEvaluationReport {
        runs,
        succeeded,
        failed,
        user_rejected,
        success_rate: if runs == 0 {
            0.0
        } else {
            succeeded as f32 / runs as f32
        },
        average_latency_ms: total_latency / divisor,
        average_cost_proxy: total_cost / divisor,
        average_tool_calls: total_tools as f32 / divisor as f32,
        average_retrievals: total_retrievals as f32 / divisor as f32,
        policy_counts,
        model_counts,
    }
}

fn orchestration_load(policy: &OrchestrationPolicy) -> usize {
    match policy {
        OrchestrationPolicy::Single => 1,
        OrchestrationPolicy::PlanExecuteReview => 2,
        OrchestrationPolicy::BestOfN { candidates } => 2 + candidates,
        OrchestrationPolicy::AutoRouter => 1,
    }
}
