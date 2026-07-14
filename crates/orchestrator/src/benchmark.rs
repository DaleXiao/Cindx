use crate::{
    classify_task, role_label, ModelCandidate, OrchestrationPolicy, QualityRubricScore,
    RoutingContext, RoutingDecision, RuleBasedRouter,
};
use agent_core::ModelRole;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub const AGENT_BENCHMARK_SCHEMA: &str = "cindx.agent-benchmark.v1";
pub const AGENT_BENCHMARK_BASELINE_SCHEMA: &str = "cindx.agent-benchmark-baseline.v1";
pub const AGENT_BENCHMARK_OBSERVATION_SCHEMA: &str = "cindx.agent-benchmark-observation.v1";
pub const AGENT_BENCHMARK_REPORT_SCHEMA: &str = "cindx.agent-benchmark-report.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBenchmarkSuite {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub description: String,
    pub cases: Vec<AgentBenchmarkCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBenchmarkCase {
    pub id: String,
    pub category: String,
    pub prompt: String,
    pub expected: AgentBenchmarkExpectation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBenchmarkExpectation {
    pub task_class: String,
    pub auto_policy: String,
    #[serde(default)]
    pub auto_candidates: Option<usize>,
    pub retrieval_mode: String,
    pub primary_model_role: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentBenchmarkBaseline {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub minimum_cases: usize,
    pub maximum_cases: usize,
    pub minimum_auto_contract_pass_rate: f64,
    pub maximum_auto_over_orchestrated: usize,
    pub maximum_auto_under_orchestrated: usize,
    pub required_categories: BTreeMap<String, usize>,
    #[serde(default)]
    pub observed_mode_thresholds: BTreeMap<AgentBenchmarkMode, AgentBenchmarkObservedThreshold>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentBenchmarkObservedThreshold {
    pub minimum_runs: usize,
    pub minimum_completion_rate: f64,
    pub minimum_quality_pass_rate: f64,
    #[serde(default)]
    pub maximum_average_latency_ms: Option<f64>,
    #[serde(default)]
    pub maximum_average_total_tokens: Option<f64>,
    #[serde(default)]
    pub maximum_average_cost_microusd: Option<f64>,
    pub maximum_safety_violations: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBenchmarkMode {
    SingleModel,
    Fast,
    Auto,
    Pro,
}

impl AgentBenchmarkMode {
    pub const ALL: [Self; 4] = [Self::SingleModel, Self::Fast, Self::Auto, Self::Pro];

    pub fn label(self) -> &'static str {
        match self {
            Self::SingleModel => "single_model",
            Self::Fast => "fast",
            Self::Auto => "auto",
            Self::Pro => "pro",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBenchmarkObservation {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub case_id: String,
    pub mode: AgentBenchmarkMode,
    pub completed: bool,
    pub quality: QualityRubricScore,
    pub latency_ms: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub estimated_cost_microusd: u64,
    pub tool_calls: u64,
    pub retrievals: u64,
    pub evidence_citations: u64,
    pub safety_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentBenchmarkReport {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub generated_at_unix_ms: u128,
    pub baseline_passed: bool,
    pub baseline_failures: Vec<String>,
    pub contract: AgentBenchmarkContractReport,
    pub modes: Vec<AgentBenchmarkModeSummary>,
    pub observations: Vec<AgentBenchmarkObservedSummary>,
    pub observation_records: Vec<AgentBenchmarkObservation>,
    pub cases: Vec<AgentBenchmarkCaseReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentBenchmarkContractReport {
    pub cases: usize,
    pub passed: usize,
    pub pass_rate: f64,
    pub over_orchestrated: usize,
    pub under_orchestrated: usize,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentBenchmarkModeSummary {
    pub mode: AgentBenchmarkMode,
    pub cases: usize,
    pub policy_counts: BTreeMap<String, usize>,
    pub average_estimated_model_calls: f64,
    pub average_estimated_latency_units: f64,
    pub average_estimated_cost_units: f64,
    pub less_orchestration_than_contract: usize,
    pub same_orchestration_as_contract: usize,
    pub more_orchestration_than_contract: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentBenchmarkObservedSummary {
    pub mode: AgentBenchmarkMode,
    pub runs: usize,
    pub completed_runs: usize,
    pub completion_rate: Option<f64>,
    pub quality_pass_rate: Option<f64>,
    pub average_correctness: Option<f64>,
    pub average_evidence: Option<f64>,
    pub average_completion: Option<f64>,
    pub average_safety: Option<f64>,
    pub average_latency_ms: Option<f64>,
    pub average_total_tokens: Option<f64>,
    pub average_estimated_cost_microusd: Option<f64>,
    pub average_tool_calls: Option<f64>,
    pub average_retrievals: Option<f64>,
    pub total_evidence_citations: u64,
    pub total_safety_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentBenchmarkCaseReport {
    pub id: String,
    pub category: String,
    pub prompt: String,
    pub expected_task_class: String,
    pub classified_task_class: String,
    pub expected_auto_policy: String,
    pub expected_auto_candidates: Option<usize>,
    pub expected_retrieval_mode: String,
    pub expected_primary_model_role: String,
    pub auto_contract_passed: bool,
    pub failures: Vec<String>,
    pub modes: Vec<AgentBenchmarkModeProjection>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentBenchmarkModeProjection {
    pub mode: AgentBenchmarkMode,
    pub policy: String,
    pub candidates: Option<usize>,
    pub retrieval_mode: String,
    pub primary_model: String,
    pub primary_model_role: String,
    pub estimated_model_calls: usize,
    pub estimated_latency_units: u64,
    pub estimated_cost_units: u64,
    pub orchestration_delta_from_contract: i32,
}

pub fn parse_agent_benchmark_suite(source: &str) -> Result<AgentBenchmarkSuite, String> {
    serde_json::from_str(source).map_err(|error| format!("invalid benchmark suite JSON: {error}"))
}

pub fn parse_agent_benchmark_baseline(source: &str) -> Result<AgentBenchmarkBaseline, String> {
    serde_json::from_str(source)
        .map_err(|error| format!("invalid benchmark baseline JSON: {error}"))
}

pub fn parse_agent_benchmark_observations(
    source: &str,
) -> Result<Vec<AgentBenchmarkObservation>, String> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "invalid benchmark observation on line {}: {error}",
                    index + 1
                )
            })
        })
        .collect()
}

pub fn evaluate_agent_benchmark(
    suite: &AgentBenchmarkSuite,
    baseline: &AgentBenchmarkBaseline,
    observations: &[AgentBenchmarkObservation],
) -> Result<AgentBenchmarkReport, Vec<String>> {
    let mut validation_errors = validate_suite(suite, baseline);
    validation_errors.extend(validate_observations(suite, observations));
    if !validation_errors.is_empty() {
        return Err(validation_errors);
    }

    let router = RuleBasedRouter;
    let mut contract = AgentBenchmarkContractReport {
        cases: suite.cases.len(),
        passed: 0,
        pass_rate: 0.0,
        over_orchestrated: 0,
        under_orchestrated: 0,
        failures: Vec::new(),
    };
    let mut case_reports = Vec::with_capacity(suite.cases.len());

    for case in &suite.cases {
        let context = RoutingContext::from_prompt(&case.prompt, benchmark_candidates());
        let auto_decision = router.route(&context);
        let expected_policy = expected_policy(&case.expected);
        let expected_load = policy_load(&expected_policy);
        let classified_task_class = classify_task(&case.prompt).label().to_string();
        let auto_role = decision_model_role(&context, &auto_decision);
        let mut failures = Vec::new();

        if classified_task_class != case.expected.task_class {
            failures.push(format!(
                "task class expected {} but got {}",
                case.expected.task_class, classified_task_class
            ));
        }
        if auto_decision.policy != expected_policy {
            failures.push(format!(
                "Auto policy expected {} but got {}",
                policy_key(&expected_policy),
                policy_key(&auto_decision.policy)
            ));
        }
        if auto_decision.retrieval_mode != case.expected.retrieval_mode {
            failures.push(format!(
                "retrieval expected {} but got {}",
                case.expected.retrieval_mode, auto_decision.retrieval_mode
            ));
        }
        if auto_role != case.expected.primary_model_role {
            failures.push(format!(
                "primary role expected {} but got {}",
                case.expected.primary_model_role, auto_role
            ));
        }

        if failures.is_empty() {
            contract.passed += 1;
        } else {
            let actual_load = policy_load(&auto_decision.policy);
            if actual_load > expected_load {
                contract.over_orchestrated += 1;
            } else if actual_load < expected_load {
                contract.under_orchestrated += 1;
            }
            contract
                .failures
                .push(format!("{}: {}", case.id, failures.join("; ")));
        }

        let modes = AgentBenchmarkMode::ALL
            .into_iter()
            .map(|mode| project_mode(mode, &case.prompt, &router, expected_load))
            .collect();
        case_reports.push(AgentBenchmarkCaseReport {
            id: case.id.clone(),
            category: case.category.clone(),
            prompt: case.prompt.clone(),
            expected_task_class: case.expected.task_class.clone(),
            classified_task_class,
            expected_auto_policy: case.expected.auto_policy.clone(),
            expected_auto_candidates: case.expected.auto_candidates,
            expected_retrieval_mode: case.expected.retrieval_mode.clone(),
            expected_primary_model_role: case.expected.primary_model_role.clone(),
            auto_contract_passed: failures.is_empty(),
            failures,
            modes,
        });
    }

    contract.pass_rate = contract.passed as f64 / contract.cases.max(1) as f64;
    let modes = summarize_mode_projections(&case_reports);
    let observation_summaries: Vec<AgentBenchmarkObservedSummary> = AgentBenchmarkMode::ALL
        .into_iter()
        .map(|mode| summarize_observations(mode, observations))
        .collect();
    let baseline_failures = evaluate_baseline(suite, baseline, &contract, &observation_summaries);

    Ok(AgentBenchmarkReport {
        schema: AGENT_BENCHMARK_REPORT_SCHEMA.to_string(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        generated_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
        baseline_passed: baseline_failures.is_empty(),
        baseline_failures,
        contract,
        modes,
        observations: observation_summaries,
        observation_records: observations.to_vec(),
        cases: case_reports,
    })
}

fn validate_suite(suite: &AgentBenchmarkSuite, baseline: &AgentBenchmarkBaseline) -> Vec<String> {
    let mut errors = Vec::new();
    if suite.schema != AGENT_BENCHMARK_SCHEMA {
        errors.push(format!("unsupported benchmark schema {}", suite.schema));
    }
    if baseline.schema != AGENT_BENCHMARK_BASELINE_SCHEMA {
        errors.push(format!("unsupported baseline schema {}", baseline.schema));
    }
    if suite.id != baseline.suite_id || suite.version != baseline.suite_version {
        errors.push("benchmark suite does not match its baseline".to_string());
    }
    if suite.cases.len() < baseline.minimum_cases || suite.cases.len() > baseline.maximum_cases {
        errors.push(format!(
            "benchmark case count {} must be between {} and {}",
            suite.cases.len(),
            baseline.minimum_cases,
            baseline.maximum_cases
        ));
    }

    let mut ids = BTreeSet::new();
    let mut categories = BTreeMap::<String, usize>::new();
    for case in &suite.cases {
        if case.id.trim().is_empty() || !ids.insert(case.id.clone()) {
            errors.push(format!(
                "benchmark case id is empty or duplicated: {}",
                case.id
            ));
        }
        if case.prompt.trim().is_empty() {
            errors.push(format!("{} has an empty prompt", case.id));
        }
        *categories.entry(case.category.clone()).or_default() += 1;
        if !matches!(
            case.expected.task_class.as_str(),
            "general" | "coding" | "research" | "retrieval" | "browser" | "computer"
        ) {
            errors.push(format!("{} has an invalid task class", case.id));
        }
        if expected_policy_checked(&case.expected).is_none() {
            errors.push(format!("{} has an invalid Auto policy contract", case.id));
        }
        if !matches!(
            case.expected.retrieval_mode.as_str(),
            "none" | "semantic_literal_parallel" | "four_way_parallel"
        ) {
            errors.push(format!("{} has an invalid retrieval mode", case.id));
        }
        if !matches!(
            case.expected.primary_model_role.as_str(),
            "planner" | "executor" | "reviewer"
        ) {
            errors.push(format!("{} has an invalid primary model role", case.id));
        }
    }
    for (category, minimum) in &baseline.required_categories {
        let actual = categories.get(category).copied().unwrap_or_default();
        if actual < *minimum {
            errors.push(format!(
                "category {category} requires at least {minimum} cases but has {actual}"
            ));
        }
    }
    for (mode, threshold) in &baseline.observed_mode_thresholds {
        if !(0.0..=1.0).contains(&threshold.minimum_completion_rate)
            || !(0.0..=1.0).contains(&threshold.minimum_quality_pass_rate)
        {
            errors.push(format!(
                "{} observation rates must be between 0 and 1",
                mode.label()
            ));
        }
    }
    errors
}

fn validate_observations(
    suite: &AgentBenchmarkSuite,
    observations: &[AgentBenchmarkObservation],
) -> Vec<String> {
    let case_ids = suite
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut errors = Vec::new();
    for (index, observation) in observations.iter().enumerate() {
        if observation.schema != AGENT_BENCHMARK_OBSERVATION_SCHEMA {
            errors.push(format!(
                "observation {} has an unsupported schema",
                index + 1
            ));
        }
        if observation.suite_id != suite.id || observation.suite_version != suite.version {
            errors.push(format!(
                "observation {} targets a different suite",
                index + 1
            ));
        }
        if !case_ids.contains(observation.case_id.as_str()) {
            errors.push(format!(
                "observation {} references unknown case {}",
                index + 1,
                observation.case_id
            ));
        }
        if let Err(error) = observation.quality.validate() {
            errors.push(format!("observation {}: {error}", index + 1));
        }
    }
    errors
}

fn expected_policy_checked(expectation: &AgentBenchmarkExpectation) -> Option<OrchestrationPolicy> {
    match expectation.auto_policy.as_str() {
        "single" if expectation.auto_candidates.is_none() => Some(OrchestrationPolicy::Single),
        "plan_execute_review" if expectation.auto_candidates.is_none() => {
            Some(OrchestrationPolicy::PlanExecuteReview)
        }
        "best_of_n" if matches!(expectation.auto_candidates, Some(2 | 3)) => {
            Some(OrchestrationPolicy::BestOfN {
                candidates: expectation.auto_candidates.unwrap_or(3),
            })
        }
        _ => None,
    }
}

fn expected_policy(expectation: &AgentBenchmarkExpectation) -> OrchestrationPolicy {
    expected_policy_checked(expectation).expect("suite is validated before evaluation")
}

fn project_mode(
    mode: AgentBenchmarkMode,
    prompt: &str,
    router: &RuleBasedRouter,
    expected_load: usize,
) -> AgentBenchmarkModeProjection {
    let mut context = if mode == AgentBenchmarkMode::SingleModel {
        RoutingContext::from_prompt(prompt, single_model_candidate())
    } else {
        RoutingContext::from_prompt(prompt, benchmark_candidates())
    };
    context.user_policy_override = match mode {
        AgentBenchmarkMode::SingleModel | AgentBenchmarkMode::Fast => {
            Some(OrchestrationPolicy::Single)
        }
        AgentBenchmarkMode::Auto => None,
        AgentBenchmarkMode::Pro => Some(OrchestrationPolicy::BestOfN { candidates: 3 }),
    };
    let decision = router.route(&context);
    let model = context
        .model_candidates
        .iter()
        .find(|candidate| candidate.name == decision.model);
    let estimated_model_calls = policy_load(&decision.policy);
    let sequential_rounds = match decision.policy {
        OrchestrationPolicy::Single | OrchestrationPolicy::AutoRouter => 1,
        OrchestrationPolicy::PlanExecuteReview => 3,
        OrchestrationPolicy::BestOfN { .. } => 3,
    };
    let latency_tier = model.map(|candidate| candidate.latency_tier).unwrap_or(1) as u64;
    let cost_tier = model.map(|candidate| candidate.cost_tier).unwrap_or(1) as u64;
    AgentBenchmarkModeProjection {
        mode,
        policy: decision.policy.label().to_string(),
        candidates: policy_candidates(&decision.policy),
        retrieval_mode: decision.retrieval_mode.clone(),
        primary_model: decision.model.clone(),
        primary_model_role: decision_model_role(&context, &decision),
        estimated_model_calls,
        estimated_latency_units: latency_tier * sequential_rounds,
        estimated_cost_units: cost_tier * estimated_model_calls as u64,
        orchestration_delta_from_contract: estimated_model_calls as i32 - expected_load as i32,
    }
}

fn summarize_mode_projections(
    cases: &[AgentBenchmarkCaseReport],
) -> Vec<AgentBenchmarkModeSummary> {
    AgentBenchmarkMode::ALL
        .into_iter()
        .map(|mode| {
            let projections = cases
                .iter()
                .flat_map(|case| case.modes.iter())
                .filter(|projection| projection.mode == mode)
                .collect::<Vec<_>>();
            let mut policy_counts = BTreeMap::new();
            let mut model_calls = 0usize;
            let mut latency_units = 0u64;
            let mut cost_units = 0u64;
            let mut less = 0usize;
            let mut same = 0usize;
            let mut more = 0usize;
            for projection in &projections {
                *policy_counts
                    .entry(policy_key_parts(&projection.policy, projection.candidates))
                    .or_default() += 1;
                model_calls += projection.estimated_model_calls;
                latency_units += projection.estimated_latency_units;
                cost_units += projection.estimated_cost_units;
                match projection.orchestration_delta_from_contract.cmp(&0) {
                    std::cmp::Ordering::Less => less += 1,
                    std::cmp::Ordering::Equal => same += 1,
                    std::cmp::Ordering::Greater => more += 1,
                }
            }
            let divisor = projections.len().max(1) as f64;
            AgentBenchmarkModeSummary {
                mode,
                cases: projections.len(),
                policy_counts,
                average_estimated_model_calls: model_calls as f64 / divisor,
                average_estimated_latency_units: latency_units as f64 / divisor,
                average_estimated_cost_units: cost_units as f64 / divisor,
                less_orchestration_than_contract: less,
                same_orchestration_as_contract: same,
                more_orchestration_than_contract: more,
            }
        })
        .collect()
}

fn summarize_observations(
    mode: AgentBenchmarkMode,
    observations: &[AgentBenchmarkObservation],
) -> AgentBenchmarkObservedSummary {
    let entries = observations
        .iter()
        .filter(|observation| observation.mode == mode)
        .collect::<Vec<_>>();
    let runs = entries.len();
    let completed_runs = entries.iter().filter(|entry| entry.completed).count();
    let quality_passes = entries
        .iter()
        .filter(|entry| entry.quality.passes())
        .count();
    let divisor = (runs > 0).then_some(runs as f64);
    let average = |total: u64| divisor.map(|value| total as f64 / value);
    AgentBenchmarkObservedSummary {
        mode,
        runs,
        completed_runs,
        completion_rate: divisor.map(|value| completed_runs as f64 / value),
        quality_pass_rate: divisor.map(|value| quality_passes as f64 / value),
        average_correctness: average(
            entries
                .iter()
                .map(|entry| entry.quality.correctness as u64)
                .sum(),
        ),
        average_evidence: average(
            entries
                .iter()
                .map(|entry| entry.quality.evidence as u64)
                .sum(),
        ),
        average_completion: average(
            entries
                .iter()
                .map(|entry| entry.quality.completion as u64)
                .sum(),
        ),
        average_safety: average(
            entries
                .iter()
                .map(|entry| entry.quality.safety as u64)
                .sum(),
        ),
        average_latency_ms: average(entries.iter().map(|entry| entry.latency_ms).sum()),
        average_total_tokens: average(
            entries
                .iter()
                .map(|entry| entry.prompt_tokens + entry.completion_tokens)
                .sum(),
        ),
        average_estimated_cost_microusd: average(
            entries
                .iter()
                .map(|entry| entry.estimated_cost_microusd)
                .sum(),
        ),
        average_tool_calls: average(entries.iter().map(|entry| entry.tool_calls).sum()),
        average_retrievals: average(entries.iter().map(|entry| entry.retrievals).sum()),
        total_evidence_citations: entries.iter().map(|entry| entry.evidence_citations).sum(),
        total_safety_violations: entries.iter().map(|entry| entry.safety_violations).sum(),
    }
}

fn evaluate_baseline(
    suite: &AgentBenchmarkSuite,
    baseline: &AgentBenchmarkBaseline,
    contract: &AgentBenchmarkContractReport,
    observations: &[AgentBenchmarkObservedSummary],
) -> Vec<String> {
    let mut failures = Vec::new();
    if contract.pass_rate + f64::EPSILON < baseline.minimum_auto_contract_pass_rate {
        failures.push(format!(
            "Auto contract pass rate {:.3} is below baseline {:.3}",
            contract.pass_rate, baseline.minimum_auto_contract_pass_rate
        ));
    }
    if contract.over_orchestrated > baseline.maximum_auto_over_orchestrated {
        failures.push(format!(
            "Auto over-orchestrated {} cases; baseline allows {}",
            contract.over_orchestrated, baseline.maximum_auto_over_orchestrated
        ));
    }
    if contract.under_orchestrated > baseline.maximum_auto_under_orchestrated {
        failures.push(format!(
            "Auto under-orchestrated {} cases; baseline allows {}",
            contract.under_orchestrated, baseline.maximum_auto_under_orchestrated
        ));
    }
    if suite.cases.len() < baseline.minimum_cases || suite.cases.len() > baseline.maximum_cases {
        failures.push("benchmark suite size moved outside the regression baseline".to_string());
    }
    for (mode, threshold) in &baseline.observed_mode_thresholds {
        let Some(observed) = observations.iter().find(|summary| summary.mode == *mode) else {
            failures.push(format!("{} observation summary is missing", mode.label()));
            continue;
        };
        if observed.runs < threshold.minimum_runs {
            failures.push(format!(
                "{} has {} observed runs; baseline requires {}",
                mode.label(),
                observed.runs,
                threshold.minimum_runs
            ));
            continue;
        }
        if observed.completion_rate.unwrap_or_default() + f64::EPSILON
            < threshold.minimum_completion_rate
        {
            failures.push(format!("{} completion rate regressed", mode.label()));
        }
        if observed.quality_pass_rate.unwrap_or_default() + f64::EPSILON
            < threshold.minimum_quality_pass_rate
        {
            failures.push(format!("{} quality pass rate regressed", mode.label()));
        }
        check_optional_maximum(
            &mut failures,
            *mode,
            "average latency",
            observed.average_latency_ms,
            threshold.maximum_average_latency_ms,
        );
        check_optional_maximum(
            &mut failures,
            *mode,
            "average tokens",
            observed.average_total_tokens,
            threshold.maximum_average_total_tokens,
        );
        check_optional_maximum(
            &mut failures,
            *mode,
            "average cost",
            observed.average_estimated_cost_microusd,
            threshold.maximum_average_cost_microusd,
        );
        if observed.total_safety_violations > threshold.maximum_safety_violations {
            failures.push(format!("{} safety violations regressed", mode.label()));
        }
    }
    failures
}

fn check_optional_maximum(
    failures: &mut Vec<String>,
    mode: AgentBenchmarkMode,
    metric: &str,
    actual: Option<f64>,
    maximum: Option<f64>,
) {
    if let Some(maximum) = maximum {
        if actual.unwrap_or(f64::INFINITY) > maximum {
            failures.push(format!("{} {metric} regressed", mode.label()));
        }
    }
}

fn benchmark_candidates() -> Vec<ModelCandidate> {
    vec![
        candidate("executor-fast", ModelRole::Executor, 1, 1),
        candidate("planner-fast", ModelRole::Planner, 2, 1),
        candidate("reviewer-fast", ModelRole::Reviewer, 2, 1),
        candidate("executor-strong", ModelRole::Executor, 4, 3),
        candidate("planner-strong", ModelRole::Planner, 4, 3),
        candidate("reviewer-strong", ModelRole::Reviewer, 4, 3),
    ]
}

fn single_model_candidate() -> Vec<ModelCandidate> {
    vec![candidate("frontier-single", ModelRole::Executor, 4, 3)]
}

fn candidate(name: &str, role: ModelRole, cost_tier: u8, latency_tier: u8) -> ModelCandidate {
    ModelCandidate {
        name: name.to_string(),
        role,
        supports_tools: true,
        supports_vision: true,
        cost_tier,
        latency_tier,
    }
}

fn decision_model_role(context: &RoutingContext, decision: &RoutingDecision) -> String {
    context
        .model_candidates
        .iter()
        .find(|candidate| candidate.name == decision.model)
        .map(|candidate| role_label(&candidate.role).to_string())
        .unwrap_or_else(|| "executor".to_string())
}

fn policy_candidates(policy: &OrchestrationPolicy) -> Option<usize> {
    match policy {
        OrchestrationPolicy::BestOfN { candidates } => Some(*candidates),
        _ => None,
    }
}

fn policy_key(policy: &OrchestrationPolicy) -> String {
    policy_key_parts(policy.label(), policy_candidates(policy))
}

fn policy_key_parts(label: &str, candidates: Option<usize>) -> String {
    candidates
        .map(|count| format!("{label}_{count}"))
        .unwrap_or_else(|| label.to_string())
}

fn policy_load(policy: &OrchestrationPolicy) -> usize {
    match policy {
        OrchestrationPolicy::Single | OrchestrationPolicy::AutoRouter => 1,
        OrchestrationPolicy::PlanExecuteReview => 3,
        OrchestrationPolicy::BestOfN { candidates } => candidates + 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE_SUITE: &str = include_str!("../../../benchmarks/agent/core-v1.json");
    const CORE_BASELINE: &str = include_str!("../../../benchmarks/agent/core-v1-baseline.json");

    #[test]
    fn core_suite_is_versioned_and_meets_the_regression_baseline() {
        let suite = parse_agent_benchmark_suite(CORE_SUITE).unwrap();
        let baseline = parse_agent_benchmark_baseline(CORE_BASELINE).unwrap();
        let report = evaluate_agent_benchmark(&suite, &baseline, &[]).unwrap();
        assert_eq!(suite.cases.len(), 72);
        assert!(report.baseline_passed, "{:?}", report.baseline_failures);
        assert_eq!(report.contract.passed, 72, "{:?}", report.contract.failures);
        assert_eq!(report.modes.len(), 4);
    }

    #[test]
    fn observations_are_aggregated_without_fabricating_missing_modes() {
        let suite = parse_agent_benchmark_suite(CORE_SUITE).unwrap();
        let baseline = parse_agent_benchmark_baseline(CORE_BASELINE).unwrap();
        let observations = vec![AgentBenchmarkObservation {
            schema: AGENT_BENCHMARK_OBSERVATION_SCHEMA.to_string(),
            suite_id: suite.id.clone(),
            suite_version: suite.version,
            case_id: suite.cases[0].id.clone(),
            mode: AgentBenchmarkMode::Auto,
            completed: true,
            quality: QualityRubricScore {
                correctness: 5,
                evidence: 4,
                completion: 5,
                safety: 5,
            },
            latency_ms: 1_200,
            prompt_tokens: 100,
            completion_tokens: 200,
            estimated_cost_microusd: 80,
            tool_calls: 1,
            retrievals: 0,
            evidence_citations: 1,
            safety_violations: 0,
        }];
        let report = evaluate_agent_benchmark(&suite, &baseline, &observations).unwrap();
        let auto = report
            .observations
            .iter()
            .find(|summary| summary.mode == AgentBenchmarkMode::Auto)
            .unwrap();
        let fast = report
            .observations
            .iter()
            .find(|summary| summary.mode == AgentBenchmarkMode::Fast)
            .unwrap();
        assert_eq!(auto.quality_pass_rate, Some(1.0));
        assert_eq!(auto.average_total_tokens, Some(300.0));
        assert_eq!(fast.runs, 0);
        assert_eq!(fast.quality_pass_rate, None);
        assert_eq!(fast.average_latency_ms, None);
    }

    #[test]
    fn unknown_observation_case_is_rejected() {
        let suite = parse_agent_benchmark_suite(CORE_SUITE).unwrap();
        let baseline = parse_agent_benchmark_baseline(CORE_BASELINE).unwrap();
        let observation = AgentBenchmarkObservation {
            schema: AGENT_BENCHMARK_OBSERVATION_SCHEMA.to_string(),
            suite_id: suite.id.clone(),
            suite_version: suite.version,
            case_id: "missing".to_string(),
            mode: AgentBenchmarkMode::Fast,
            completed: false,
            quality: QualityRubricScore {
                correctness: 0,
                evidence: 0,
                completion: 0,
                safety: 5,
            },
            latency_ms: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            estimated_cost_microusd: 0,
            tool_calls: 0,
            retrievals: 0,
            evidence_citations: 0,
            safety_violations: 0,
        };
        let errors = evaluate_agent_benchmark(&suite, &baseline, &[observation]).unwrap_err();
        assert!(errors.iter().any(|error| error.contains("unknown case")));
    }

    #[test]
    fn observed_quality_baseline_fails_when_required_runs_are_missing() {
        let suite = parse_agent_benchmark_suite(CORE_SUITE).unwrap();
        let mut baseline = parse_agent_benchmark_baseline(CORE_BASELINE).unwrap();
        baseline.observed_mode_thresholds.insert(
            AgentBenchmarkMode::Auto,
            AgentBenchmarkObservedThreshold {
                minimum_runs: 2,
                minimum_completion_rate: 0.9,
                minimum_quality_pass_rate: 0.8,
                maximum_average_latency_ms: Some(10_000.0),
                maximum_average_total_tokens: None,
                maximum_average_cost_microusd: None,
                maximum_safety_violations: 0,
            },
        );

        let report = evaluate_agent_benchmark(&suite, &baseline, &[]).unwrap();

        assert!(!report.baseline_passed);
        assert!(report
            .baseline_failures
            .iter()
            .any(|failure| failure.contains("requires 2")));
    }
}
