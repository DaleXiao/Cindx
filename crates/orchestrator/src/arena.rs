use crate::{AgentEvaluationEvidenceSource, QualityRubricScore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const AGENT_ARENA_SUITE_SCHEMA: &str = "cindx.agent-arena-suite.v1";
pub const AGENT_ARENA_OBSERVATION_SCHEMA: &str = "cindx.agent-arena-observation.v1";
pub const AGENT_ARENA_REPORT_SCHEMA: &str = "cindx.agent-arena-report.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentArenaMode {
    SingleModel,
    Fast,
    Auto,
    Pro,
}

impl AgentArenaMode {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentArenaCapability {
    General,
    Coding,
    Files,
    Research,
    Browser,
    Computer,
    Retrieval,
    Memory,
    LongHorizon,
    Recovery,
    Safety,
    MultiModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentArenaBudget {
    pub max_wall_time_ms: u64,
    pub max_model_calls: u64,
    pub max_tool_calls: u64,
    pub max_total_tokens: u64,
}

impl AgentArenaBudget {
    fn validate(&self) -> Result<(), String> {
        if self.max_wall_time_ms == 0 || self.max_model_calls == 0 || self.max_total_tokens == 0 {
            return Err("arena budget time, model calls, and tokens must be positive".to_string());
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        format!("{:x}", Sha256::digest(encoded))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentArenaSuite {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub description: String,
    pub repeats_per_case: usize,
    pub modes: Vec<AgentArenaMode>,
    pub budget: AgentArenaBudget,
    pub cases: Vec<AgentArenaCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentArenaCase {
    pub id: String,
    pub category: String,
    pub objective: String,
    pub capabilities: Vec<AgentArenaCapability>,
    pub verifier: AgentArenaVerifierContract,
    #[serde(default)]
    pub fixture: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentArenaVerifierContract {
    pub kind: String,
    pub deterministic: bool,
    pub description: String,
}

impl AgentArenaSuite {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema != AGENT_ARENA_SUITE_SCHEMA {
            errors.push(format!("unsupported arena suite schema {}", self.schema));
        }
        if self.id.trim().is_empty() || self.version == 0 || self.description.trim().is_empty() {
            errors.push("arena suite identity is incomplete".to_string());
        }
        if self.repeats_per_case < 3 {
            errors.push("arena suite requires at least three repeats per case".to_string());
        }
        if let Err(error) = self.budget.validate() {
            errors.push(error);
        }
        let modes = self.modes.iter().copied().collect::<BTreeSet<_>>();
        if modes != AgentArenaMode::ALL.into_iter().collect() {
            errors.push("arena suite must compare single_model, fast, auto, and pro".to_string());
        }
        if self.cases.len() < 100 {
            errors.push("arena suite requires at least 100 versioned tasks".to_string());
        }
        let mut ids = BTreeSet::new();
        let mut categories = BTreeSet::new();
        let mut capabilities = BTreeSet::new();
        for case in &self.cases {
            if case.id.trim().is_empty() || !ids.insert(case.id.as_str()) {
                errors.push(format!("arena case id is empty or duplicated: {}", case.id));
            }
            if case.category.trim().is_empty() || case.objective.trim().is_empty() {
                errors.push(format!("arena case {} is incomplete", case.id));
            }
            if case.capabilities.is_empty() {
                errors.push(format!("arena case {} declares no capabilities", case.id));
            }
            if case.verifier.kind.trim().is_empty() || case.verifier.description.trim().is_empty() {
                errors.push(format!("arena case {} has no verifier contract", case.id));
            }
            categories.insert(case.category.as_str());
            capabilities.extend(case.capabilities.iter().copied());
        }
        if categories.len() < 10 {
            errors.push("arena suite requires at least ten task categories".to_string());
        }
        let missing_capabilities = AgentArenaCapability::ALL
            .into_iter()
            .filter(|capability| !capabilities.contains(capability))
            .map(|capability| format!("{capability:?}").to_lowercase())
            .collect::<Vec<_>>();
        if !missing_capabilities.is_empty() {
            errors.push(format!(
                "arena suite is missing capabilities: {}",
                missing_capabilities.join(", ")
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl AgentArenaCapability {
    pub const ALL: [Self; 12] = [
        Self::General,
        Self::Coding,
        Self::Files,
        Self::Research,
        Self::Browser,
        Self::Computer,
        Self::Retrieval,
        Self::Memory,
        Self::LongHorizon,
        Self::Recovery,
        Self::Safety,
        Self::MultiModel,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentArenaObservation {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub case_id: String,
    pub category: String,
    pub seed: u64,
    pub mode: AgentArenaMode,
    pub candidate_id: String,
    pub budget_fingerprint: String,
    pub provider_backed: bool,
    pub completed: bool,
    pub verifier_passed: bool,
    pub verifier_score: f64,
    pub evidence_source: AgentEvaluationEvidenceSource,
    pub quality: QualityRubricScore,
    pub wall_time_ms: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub total_tokens: u64,
    #[serde(default)]
    pub recoveries: u64,
    #[serde(default)]
    pub safety_violations: u64,
    #[serde(default)]
    pub failure_reason: Option<String>,
}

impl AgentArenaObservation {
    fn validate(&self, suite: &AgentArenaSuite) -> Result<(), String> {
        if self.schema != AGENT_ARENA_OBSERVATION_SCHEMA {
            return Err(format!(
                "unsupported arena observation schema {}",
                self.schema
            ));
        }
        if self.suite_id != suite.id || self.suite_version != suite.version {
            return Err("arena observation targets a different suite".to_string());
        }
        if self.candidate_id.trim().is_empty() || self.case_id.trim().is_empty() {
            return Err("arena observation identity is incomplete".to_string());
        }
        if !self.verifier_score.is_finite() || !(0.0..=1.0).contains(&self.verifier_score) {
            return Err("arena verifier score must be between zero and one".to_string());
        }
        if self.budget_fingerprint != suite.budget.fingerprint() {
            return Err("arena observation was produced with a different budget".to_string());
        }
        Ok(())
    }

    fn is_over_budget(&self, budget: &AgentArenaBudget) -> bool {
        self.wall_time_ms > budget.max_wall_time_ms
            || self.model_calls > budget.max_model_calls
            || self.tool_calls > budget.max_tool_calls
            || self.total_tokens > budget.max_total_tokens
    }

    fn quality_passed(&self) -> bool {
        self.quality.correctness >= 3
            && self.quality.evidence >= 3
            && self.quality.completion >= 3
            && self.quality.safety >= 3
            && self.quality.total() >= 15
    }

    fn successful(&self, budget: &AgentArenaBudget) -> bool {
        self.completed
            && self.verifier_passed
            && self.quality_passed()
            && self.safety_violations == 0
            && !self.is_over_budget(budget)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentArenaReport {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub cases: usize,
    pub repeats_per_case: usize,
    pub required_observations: usize,
    pub observed_runs: usize,
    pub provider_backed_runs: usize,
    pub missing_runs: usize,
    pub over_budget_runs: usize,
    pub invalid_runs: usize,
    pub category_counts: BTreeMap<String, usize>,
    pub capability_counts: BTreeMap<AgentArenaCapability, usize>,
    pub modes: Vec<AgentArenaModeReport>,
    pub pairwise: Vec<AgentArenaPairwiseReport>,
    pub readiness_failures: Vec<String>,
    pub ready_for_scientific_comparison: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentArenaModeReport {
    pub mode: AgentArenaMode,
    pub runs: usize,
    pub successful_runs: usize,
    pub success_rate: Option<f64>,
    pub verifier_pass_rate: Option<f64>,
    pub average_quality: Option<f64>,
    pub p50_wall_time_ms: Option<u64>,
    pub p95_wall_time_ms: Option<u64>,
    pub average_model_calls: Option<f64>,
    pub average_tool_calls: Option<f64>,
    pub average_total_tokens: Option<f64>,
    pub recoveries: u64,
    pub safety_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentArenaPairwiseReport {
    pub baseline: AgentArenaMode,
    pub challenger: AgentArenaMode,
    pub paired_runs: usize,
    pub challenger_wins: usize,
    pub ties: usize,
    pub challenger_losses: usize,
    pub win_rate: Option<f64>,
}

pub fn parse_agent_arena_suite(source: &str) -> Result<AgentArenaSuite, String> {
    serde_json::from_str(source).map_err(|error| format!("invalid arena suite JSON: {error}"))
}

pub fn parse_agent_arena_observations(source: &str) -> Result<Vec<AgentArenaObservation>, String> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|error| {
                format!("invalid arena observation on line {}: {error}", index + 1)
            })
        })
        .collect()
}

pub fn evaluate_agent_arena(
    suite: &AgentArenaSuite,
    observations: &[AgentArenaObservation],
) -> Result<AgentArenaReport, Vec<String>> {
    suite.validate()?;
    let case_by_id = suite
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let allowed_modes = suite.modes.iter().copied().collect::<BTreeSet<_>>();
    let mut validation_errors = Vec::new();
    let mut invalid_runs = 0usize;
    let mut seen = BTreeSet::new();
    for observation in observations {
        if let Err(error) = observation.validate(suite) {
            validation_errors.push(format!("{}: {error}", observation.case_id));
            invalid_runs += 1;
            continue;
        }
        let Some(case) = case_by_id.get(observation.case_id.as_str()) else {
            validation_errors.push(format!("unknown arena case {}", observation.case_id));
            invalid_runs += 1;
            continue;
        };
        if observation.category != case.category {
            validation_errors.push(format!(
                "arena observation {} has category drift",
                observation.case_id
            ));
            invalid_runs += 1;
        }
        if !allowed_modes.contains(&observation.mode) {
            validation_errors.push(format!(
                "arena observation {} uses an unsupported mode",
                observation.case_id
            ));
            invalid_runs += 1;
        }
        if !seen.insert((
            observation.case_id.as_str(),
            observation.seed,
            observation.mode,
        )) {
            validation_errors.push(format!(
                "duplicate arena observation {} seed {} mode {}",
                observation.case_id,
                observation.seed,
                observation.mode.label()
            ));
            invalid_runs += 1;
        }
    }

    let required_observations = suite
        .cases
        .len()
        .saturating_mul(suite.repeats_per_case)
        .saturating_mul(suite.modes.len());
    let expected_keys = suite
        .cases
        .iter()
        .flat_map(|case| {
            (0..suite.repeats_per_case).flat_map(move |seed| {
                suite
                    .modes
                    .iter()
                    .copied()
                    .map(move |mode| (case.id.as_str(), seed as u64, mode))
            })
        })
        .collect::<BTreeSet<_>>();
    let missing_runs = expected_keys.difference(&seen).count();
    let provider_backed_runs = observations
        .iter()
        .filter(|run| run.provider_backed)
        .count();
    let over_budget_runs = observations
        .iter()
        .filter(|run| run.is_over_budget(&suite.budget))
        .count();
    let category_counts = suite
        .cases
        .iter()
        .fold(BTreeMap::new(), |mut counts, case| {
            *counts.entry(case.category.clone()).or_default() += 1;
            counts
        });
    let capability_counts = suite
        .cases
        .iter()
        .fold(BTreeMap::new(), |mut counts, case| {
            for capability in &case.capabilities {
                *counts.entry(*capability).or_default() += 1;
            }
            counts
        });
    let modes = suite
        .modes
        .iter()
        .copied()
        .map(|mode| summarize_mode(mode, observations, &suite.budget))
        .collect::<Vec<_>>();
    let pairwise = [
        AgentArenaMode::Fast,
        AgentArenaMode::Auto,
        AgentArenaMode::Pro,
    ]
    .into_iter()
    .map(|challenger| {
        summarize_pairwise(
            AgentArenaMode::SingleModel,
            challenger,
            observations,
            &suite.budget,
        )
    })
    .collect::<Vec<_>>();
    let mut readiness_failures = validation_errors.clone();
    if missing_runs != 0 {
        readiness_failures.push(format!(
            "{missing_runs} required paired arena runs are missing"
        ));
    }
    if observations.len() != required_observations {
        readiness_failures.push(format!(
            "arena requires {required_observations} observations, found {}",
            observations.len()
        ));
    }
    if provider_backed_runs != observations.len() || observations.is_empty() {
        readiness_failures.push("all arena runs must be provider-backed".to_string());
    }
    if over_budget_runs != 0 {
        readiness_failures.push(format!(
            "{over_budget_runs} arena runs exceeded the shared budget"
        ));
    }
    if observations
        .iter()
        .any(|run| run.evidence_source != AgentEvaluationEvidenceSource::Deterministic)
    {
        readiness_failures
            .push("arena release evidence must use deterministic verifiers".to_string());
    }
    Ok(AgentArenaReport {
        schema: AGENT_ARENA_REPORT_SCHEMA.to_string(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        cases: suite.cases.len(),
        repeats_per_case: suite.repeats_per_case,
        required_observations,
        observed_runs: observations.len(),
        provider_backed_runs,
        missing_runs,
        over_budget_runs,
        invalid_runs,
        category_counts,
        capability_counts,
        modes,
        pairwise,
        ready_for_scientific_comparison: readiness_failures.is_empty(),
        readiness_failures,
    })
}

fn summarize_mode(
    mode: AgentArenaMode,
    observations: &[AgentArenaObservation],
    budget: &AgentArenaBudget,
) -> AgentArenaModeReport {
    let runs = observations
        .iter()
        .filter(|observation| observation.mode == mode)
        .collect::<Vec<_>>();
    let count = runs.len();
    let successful_runs = runs.iter().filter(|run| run.successful(budget)).count();
    let ratio = |value: usize| (count != 0).then_some(value as f64 / count as f64);
    let average = |value: fn(&AgentArenaObservation) -> u64| {
        (count != 0).then_some(runs.iter().map(|run| value(run) as f64).sum::<f64>() / count as f64)
    };
    let mut latencies = runs.iter().map(|run| run.wall_time_ms).collect::<Vec<_>>();
    latencies.sort_unstable();
    AgentArenaModeReport {
        mode,
        runs: count,
        successful_runs,
        success_rate: ratio(successful_runs),
        verifier_pass_rate: ratio(runs.iter().filter(|run| run.verifier_passed).count()),
        average_quality: (count != 0).then_some(
            runs.iter()
                .map(|run| f64::from(run.quality.total()))
                .sum::<f64>()
                / count as f64,
        ),
        p50_wall_time_ms: percentile(&latencies, 0.50),
        p95_wall_time_ms: percentile(&latencies, 0.95),
        average_model_calls: average(|run| run.model_calls),
        average_tool_calls: average(|run| run.tool_calls),
        average_total_tokens: average(|run| run.total_tokens),
        recoveries: runs.iter().map(|run| run.recoveries).sum(),
        safety_violations: runs.iter().map(|run| run.safety_violations).sum(),
    }
}

fn summarize_pairwise(
    baseline: AgentArenaMode,
    challenger: AgentArenaMode,
    observations: &[AgentArenaObservation],
    budget: &AgentArenaBudget,
) -> AgentArenaPairwiseReport {
    let by_key = observations
        .iter()
        .map(|run| ((run.case_id.as_str(), run.seed, run.mode), run))
        .collect::<BTreeMap<_, _>>();
    let keys = observations
        .iter()
        .filter(|run| run.mode == baseline)
        .map(|run| (run.case_id.as_str(), run.seed))
        .collect::<BTreeSet<_>>();
    let mut wins = 0usize;
    let mut ties = 0usize;
    let mut losses = 0usize;
    for (case_id, seed) in keys {
        let Some(left) = by_key.get(&(case_id, seed, baseline)) else {
            continue;
        };
        let Some(right) = by_key.get(&(case_id, seed, challenger)) else {
            continue;
        };
        let left_score = run_pairwise_score(left, budget);
        let right_score = run_pairwise_score(right, budget);
        if right_score > left_score + f64::EPSILON * 8.0 {
            wins += 1;
        } else if left_score > right_score + f64::EPSILON * 8.0 {
            losses += 1;
        } else {
            ties += 1;
        }
    }
    let paired_runs = wins + ties + losses;
    AgentArenaPairwiseReport {
        baseline,
        challenger,
        paired_runs,
        challenger_wins: wins,
        ties,
        challenger_losses: losses,
        win_rate: (paired_runs != 0)
            .then_some((wins as f64 + ties as f64 * 0.5) / paired_runs as f64),
    }
}

fn run_pairwise_score(run: &AgentArenaObservation, budget: &AgentArenaBudget) -> f64 {
    if run.successful(budget) {
        run.verifier_score * 0.6 + f64::from(run.quality.total()) / 20.0 * 0.4
    } else {
        0.0
    }
}

fn percentile(values: &[u64], percentile: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    values.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite(case_count: usize) -> AgentArenaSuite {
        AgentArenaSuite {
            schema: AGENT_ARENA_SUITE_SCHEMA.to_string(),
            id: "arena-test".to_string(),
            version: 1,
            description: "Test arena".to_string(),
            repeats_per_case: 3,
            modes: AgentArenaMode::ALL.to_vec(),
            budget: AgentArenaBudget {
                max_wall_time_ms: 1_000,
                max_model_calls: 8,
                max_tool_calls: 8,
                max_total_tokens: 20_000,
            },
            cases: (0..case_count)
                .map(|index| AgentArenaCase {
                    id: format!("case-{index}"),
                    category: format!("category-{}", index % 10),
                    objective: format!("Complete task {index}"),
                    capabilities: vec![
                        AgentArenaCapability::ALL[index % AgentArenaCapability::ALL.len()],
                    ],
                    verifier: AgentArenaVerifierContract {
                        kind: "fixture".to_string(),
                        deterministic: true,
                        description: "Compare fixture output".to_string(),
                    },
                    fixture: None,
                    metadata: BTreeMap::new(),
                })
                .collect(),
        }
    }

    fn observation(
        suite: &AgentArenaSuite,
        case: &AgentArenaCase,
        seed: u64,
        mode: AgentArenaMode,
    ) -> AgentArenaObservation {
        AgentArenaObservation {
            schema: AGENT_ARENA_OBSERVATION_SCHEMA.to_string(),
            suite_id: suite.id.clone(),
            suite_version: suite.version,
            case_id: case.id.clone(),
            category: case.category.clone(),
            seed,
            mode,
            candidate_id: mode.label().to_string(),
            budget_fingerprint: suite.budget.fingerprint(),
            provider_backed: true,
            completed: true,
            verifier_passed: true,
            verifier_score: 1.0,
            evidence_source: AgentEvaluationEvidenceSource::Deterministic,
            quality: QualityRubricScore {
                correctness: 5,
                evidence: 5,
                completion: 5,
                safety: 5,
            },
            wall_time_ms: 100,
            model_calls: 1,
            tool_calls: 0,
            total_tokens: 100,
            recoveries: 0,
            safety_violations: 0,
            failure_reason: None,
        }
    }

    #[test]
    fn arena_requires_real_scale_and_full_capability_coverage() {
        let valid_suite = suite(120);
        assert!(valid_suite.validate().is_ok());
        let errors = suite(12).validate().unwrap_err();
        assert!(errors.iter().any(|error| error.contains("at least 100")));
    }

    #[test]
    fn arena_is_not_ready_without_provider_observations() {
        let suite = suite(120);
        let report = evaluate_agent_arena(&suite, &[]).unwrap();
        assert!(!report.ready_for_scientific_comparison);
        assert_eq!(report.required_observations, 1_440);
        assert_eq!(report.missing_runs, 1_440);
    }

    #[test]
    fn arena_requires_fair_paired_repeats() {
        let suite = suite(120);
        let mut observations = Vec::new();
        for case in &suite.cases {
            for seed in 0..suite.repeats_per_case {
                for mode in suite.modes.iter().copied() {
                    observations.push(observation(&suite, case, seed as u64, mode));
                }
            }
        }
        let report = evaluate_agent_arena(&suite, &observations).unwrap();
        assert!(report.ready_for_scientific_comparison);
        assert_eq!(report.missing_runs, 0);
        assert_eq!(report.pairwise[2].paired_runs, 360);
    }

    #[test]
    fn over_budget_runs_fail_readiness_and_success() {
        let suite = suite(120);
        let mut run = observation(&suite, &suite.cases[0], 0, AgentArenaMode::Auto);
        run.model_calls = suite.budget.max_model_calls + 1;
        let report = evaluate_agent_arena(&suite, &[run]).unwrap();
        assert_eq!(report.over_budget_runs, 1);
        assert!(!report.ready_for_scientific_comparison);
        assert_eq!(report.modes[2].successful_runs, 0);
    }
}
