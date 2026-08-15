use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

pub const FUGU_EVALUATION_SUITE_SCHEMA: &str = "cindx.fugu-evaluation-suite.v1";
pub const FUGU_EVALUATION_OBSERVATION_SCHEMA: &str = "cindx.fugu-evaluation-observation.v1";
pub const FUGU_EVALUATION_RUN_PLAN_SCHEMA: &str = "cindx.fugu-evaluation-run-plan.v1";
pub const FUGU_EVALUATION_REPORT_SCHEMA: &str = "cindx.fugu-evaluation-report.v1";

const REQUIRED_BENCHMARKS: [&str; 11] = [
    "swe_bench_pro",
    "terminal_bench_2_1",
    "livecodebench_v6",
    "livecodebench_pro",
    "humanitys_last_exam",
    "charxiv",
    "gpqa_diamond",
    "scicode",
    "tau3_banking",
    "long_context_reasoning",
    "mrcr_v2",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguEvaluationSuite {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub description: String,
    pub frozen_source: FuguFrozenSource,
    pub safety_policy: FuguSafetyPolicy,
    pub tracks: Vec<FuguEvaluationTrack>,
    pub treatments: Vec<FuguTreatment>,
    pub benchmarks: Vec<FuguBenchmark>,
    #[serde(default)]
    pub extended_probes: Vec<FuguExtendedProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguFrozenSource {
    pub title: String,
    pub version: String,
    pub url: String,
    pub results_table: String,
    pub frozen_on: String,
    pub interpretation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguSafetyPolicy {
    pub default_mode: FuguExecutionMode,
    pub requires_explicit_execution: bool,
    pub sandbox_root_template: String,
    pub source_workspace_access: FuguWorkspaceAccess,
    pub network_default: FuguNetworkMode,
    #[serde(default)]
    pub network_allowlist: Vec<String>,
    pub allow_destructive_tools: bool,
    pub allow_privileged_processes: bool,
    pub allow_symlink_escape: bool,
    pub max_parallel_processes: usize,
    pub max_disk_bytes_per_run: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuguExecutionMode {
    PlanOnly,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuguWorkspaceAccess {
    ReadOnly,
    EphemeralCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuguNetworkMode {
    Disabled,
    Allowlisted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguEvaluationTrack {
    pub id: String,
    pub kind: FuguTrackKind,
    pub description: String,
    pub benchmark_ids: Vec<String>,
    pub treatment_ids: Vec<String>,
    pub seeds: Vec<u64>,
    #[serde(default)]
    pub shared_budget: Option<FuguSharedBudget>,
    pub readiness_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuguTrackKind {
    QualityFirst,
    IsoBudget,
    CausalAblation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguSharedBudget {
    pub max_wall_time_ms: u64,
    pub max_model_calls: u64,
    pub max_tool_calls: u64,
    pub max_total_tokens: u64,
}

impl FuguSharedBudget {
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        sha256_hex(&encoded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguTreatment {
    pub id: String,
    pub label: String,
    pub kind: FuguTreatmentKind,
    pub description: String,
    #[serde(default)]
    pub base_treatment_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuguTreatmentKind {
    SingleWorker,
    FixedEnsemble,
    CindxFast,
    CindxAuto,
    CindxPro,
    Ablation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguBenchmark {
    pub id: String,
    pub display_name: String,
    pub capability: String,
    pub metric: String,
    pub metric_min: f64,
    pub metric_max: f64,
    pub fugu_ultra_v1_score: f64,
    pub adapter: String,
    pub protocol: FuguBenchmarkProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguBenchmarkProtocol {
    pub dataset: String,
    pub split: String,
    pub harness: String,
    pub max_turns: Option<u64>,
    pub retries: Option<u64>,
    pub tools: String,
    pub judge: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguExtendedProbe {
    pub id: String,
    pub display_name: String,
    pub protocol: String,
    pub readiness_required: bool,
}

impl FuguEvaluationSuite {
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        sha256_hex(&encoded)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema != FUGU_EVALUATION_SUITE_SCHEMA {
            errors.push(format!("unsupported Fugu suite schema {}", self.schema));
        }
        if self.id.trim().is_empty() || self.version == 0 || self.description.trim().is_empty() {
            errors.push("Fugu suite identity is incomplete".to_string());
        }
        if self.frozen_source.version != "v1"
            || !self.frozen_source.url.contains("2606.21228v1")
            || self.frozen_source.results_table.trim().is_empty()
        {
            errors.push("Fugu source must be frozen to technical report v1".to_string());
        }
        validate_safety_policy(&self.safety_policy, &mut errors);

        let benchmark_ids = unique_ids(
            self.benchmarks
                .iter()
                .map(|benchmark| benchmark.id.as_str()),
            "benchmark",
            &mut errors,
        );
        let required = REQUIRED_BENCHMARKS.into_iter().collect::<BTreeSet<_>>();
        let observed = benchmark_ids.iter().copied().collect::<BTreeSet<_>>();
        if observed != required {
            let missing = required.difference(&observed).copied().collect::<Vec<_>>();
            let extra = observed.difference(&required).copied().collect::<Vec<_>>();
            errors.push(format!(
                "Fugu benchmark set drifted; missing=[{}] extra=[{}]",
                missing.join(", "),
                extra.join(", ")
            ));
        }
        for benchmark in &self.benchmarks {
            if benchmark.display_name.trim().is_empty()
                || benchmark.capability.trim().is_empty()
                || benchmark.metric.trim().is_empty()
                || benchmark.adapter.trim().is_empty()
            {
                errors.push(format!("benchmark {} is incomplete", benchmark.id));
            }
            if !benchmark.metric_min.is_finite()
                || !benchmark.metric_max.is_finite()
                || benchmark.metric_max <= benchmark.metric_min
                || !benchmark.fugu_ultra_v1_score.is_finite()
                || benchmark.fugu_ultra_v1_score < benchmark.metric_min
                || benchmark.fugu_ultra_v1_score > benchmark.metric_max
            {
                errors.push(format!(
                    "benchmark {} has an invalid metric scale",
                    benchmark.id
                ));
            }
            if benchmark.protocol.dataset.trim().is_empty()
                || benchmark.protocol.harness.trim().is_empty()
                || benchmark.protocol.judge.trim().is_empty()
            {
                errors.push(format!(
                    "benchmark {} has an incomplete protocol",
                    benchmark.id
                ));
            }
        }

        let treatment_ids = unique_ids(
            self.treatments
                .iter()
                .map(|treatment| treatment.id.as_str()),
            "treatment",
            &mut errors,
        );
        let single_workers = self
            .treatments
            .iter()
            .filter(|treatment| treatment.kind == FuguTreatmentKind::SingleWorker)
            .count();
        if single_workers < 2 {
            errors.push("Fugu suite requires at least two single-worker controls".to_string());
        }
        for treatment in &self.treatments {
            if treatment.label.trim().is_empty() || treatment.description.trim().is_empty() {
                errors.push(format!("treatment {} is incomplete", treatment.id));
            }
            if treatment.kind == FuguTreatmentKind::Ablation {
                match treatment.base_treatment_id.as_deref() {
                    Some(base) if treatment_ids.contains(base) => {}
                    _ => errors.push(format!(
                        "ablation {} does not name a known base treatment",
                        treatment.id
                    )),
                }
            }
        }

        unique_ids(
            self.tracks.iter().map(|track| track.id.as_str()),
            "track",
            &mut errors,
        );
        if !self
            .tracks
            .iter()
            .any(|track| track.kind == FuguTrackKind::QualityFirst)
            || !self
                .tracks
                .iter()
                .any(|track| track.kind == FuguTrackKind::IsoBudget)
            || !self
                .tracks
                .iter()
                .any(|track| track.kind == FuguTrackKind::CausalAblation)
        {
            errors.push("Fugu suite requires quality, iso-budget, and causal tracks".to_string());
        }
        for track in &self.tracks {
            if track.description.trim().is_empty() || track.seeds.len() < 3 {
                errors.push(format!(
                    "track {} needs a description and at least three matched seeds",
                    track.id
                ));
            }
            if track.seeds.iter().copied().collect::<BTreeSet<_>>().len() != track.seeds.len() {
                errors.push(format!("track {} has duplicate seeds", track.id));
            }
            if track.benchmark_ids.is_empty()
                || track
                    .benchmark_ids
                    .iter()
                    .any(|id| !benchmark_ids.contains(id.as_str()))
            {
                errors.push(format!("track {} references unknown benchmarks", track.id));
            }
            if track.treatment_ids.is_empty()
                || track
                    .treatment_ids
                    .iter()
                    .any(|id| !treatment_ids.contains(id.as_str()))
            {
                errors.push(format!("track {} references unknown treatments", track.id));
            }
            if track.kind == FuguTrackKind::IsoBudget && track.shared_budget.is_none() {
                errors.push(format!(
                    "iso-budget track {} has no shared budget",
                    track.id
                ));
            }
            if let Some(budget) = &track.shared_budget {
                if budget.max_wall_time_ms == 0
                    || budget.max_model_calls == 0
                    || budget.max_tool_calls == 0
                    || budget.max_total_tokens == 0
                {
                    errors.push(format!("track {} has an invalid shared budget", track.id));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

pub(crate) fn validate_safety_policy(policy: &FuguSafetyPolicy, errors: &mut Vec<String>) {
    if policy.default_mode != FuguExecutionMode::PlanOnly
        || !policy.requires_explicit_execution
        || policy.source_workspace_access != FuguWorkspaceAccess::ReadOnly
        || policy.network_default != FuguNetworkMode::Disabled
        || policy.allow_destructive_tools
        || policy.allow_privileged_processes
        || policy.allow_symlink_escape
    {
        errors.push(
            "Fugu safety policy must default to explicit, offline, read-only plan mode".to_string(),
        );
    }
    if !policy.sandbox_root_template.contains("cindx-fugu-eval")
        || policy.max_parallel_processes == 0
        || policy.max_disk_bytes_per_run == 0
    {
        errors.push("Fugu safety policy has invalid sandbox limits".to_string());
    }
}

fn unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    label: &str,
    errors: &mut Vec<String>,
) -> BTreeSet<&'a str> {
    let mut unique = BTreeSet::new();
    for id in ids {
        if id.trim().is_empty() || !unique.insert(id) {
            errors.push(format!("{label} id is empty or duplicated: {id}"));
        }
    }
    unique
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguEvaluationObservation {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub protocol_fingerprint: String,
    pub run_id: String,
    pub track_id: String,
    pub benchmark_id: String,
    pub treatment_id: String,
    pub seed: u64,
    pub candidate_id: String,
    pub provider_backed: bool,
    pub protocol_equivalent_to_fugu_v1: bool,
    pub completed: bool,
    #[serde(default)]
    pub native_score: Option<f64>,
    pub wall_time_ms: u64,
    #[serde(default)]
    pub ttft_ms: Option<u64>,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub total_tokens: u64,
    #[serde(default)]
    pub recoveries: u64,
    #[serde(default)]
    pub safety_violations: u64,
    #[serde(default)]
    pub budget_fingerprint: Option<String>,
    pub evidence: FuguEvidenceProvenance,
    pub safety: FuguSafetyAttestation,
    #[serde(default)]
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguEvidenceProvenance {
    pub adapter_version: String,
    pub dataset_version: String,
    pub harness_commit: String,
    pub raw_result_sha256: String,
    pub synthetic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuguSafetyAttestation {
    pub sandbox_id: String,
    pub ephemeral_root: bool,
    pub source_workspace_read_only: bool,
    pub network_mode: FuguNetworkMode,
    #[serde(default)]
    pub network_destinations: Vec<String>,
    pub writes_outside_sandbox: u64,
    pub destructive_actions: u64,
    pub privileged_actions: u64,
    pub symlink_escapes: u64,
    pub protected_path_accesses: u64,
}

impl FuguEvaluationObservation {
    fn key(&self) -> (&str, &str, &str, u64) {
        (
            self.track_id.as_str(),
            self.benchmark_id.as_str(),
            self.treatment_id.as_str(),
            self.seed,
        )
    }

    fn normalized_score(&self, benchmark: &FuguBenchmark) -> f64 {
        if !self.completed || self.safety_violations != 0 {
            return 0.0;
        }
        self.native_score
            .map(|score| {
                ((score - benchmark.metric_min) / (benchmark.metric_max - benchmark.metric_min))
                    .clamp(0.0, 1.0)
            })
            .unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FuguEvaluationRunPlan {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub protocol_fingerprint: String,
    pub execution_enabled: bool,
    pub safety_policy: FuguSafetyPolicy,
    pub runs: Vec<FuguPlannedRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FuguPlannedRun {
    pub run_id: String,
    pub track_id: String,
    pub benchmark_id: String,
    pub treatment_id: String,
    pub seed: u64,
    pub budget_fingerprint: Option<String>,
    pub status: String,
}

pub fn parse_fugu_evaluation_suite(source: &str) -> Result<FuguEvaluationSuite, String> {
    serde_json::from_str(source).map_err(|error| format!("invalid Fugu suite JSON: {error}"))
}

pub fn parse_fugu_evaluation_observations(
    source: &str,
) -> Result<Vec<FuguEvaluationObservation>, String> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line)
                .map_err(|error| format!("invalid Fugu observation on line {}: {error}", index + 1))
        })
        .collect()
}

pub fn build_fugu_evaluation_run_plan(
    suite: &FuguEvaluationSuite,
) -> Result<FuguEvaluationRunPlan, Vec<String>> {
    suite.validate()?;
    let runs = suite
        .tracks
        .iter()
        .flat_map(|track| {
            track.benchmark_ids.iter().flat_map(move |benchmark_id| {
                track.treatment_ids.iter().flat_map(move |treatment_id| {
                    track.seeds.iter().copied().map(move |seed| FuguPlannedRun {
                        run_id: format!("{}:{}:{}:{seed}", track.id, benchmark_id, treatment_id),
                        track_id: track.id.clone(),
                        benchmark_id: benchmark_id.clone(),
                        treatment_id: treatment_id.clone(),
                        seed,
                        budget_fingerprint: track
                            .shared_budget
                            .as_ref()
                            .map(FuguSharedBudget::fingerprint),
                        status: "planned".to_string(),
                    })
                })
            })
        })
        .collect();
    Ok(FuguEvaluationRunPlan {
        schema: FUGU_EVALUATION_RUN_PLAN_SCHEMA.to_string(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        protocol_fingerprint: suite.fingerprint(),
        execution_enabled: false,
        safety_policy: suite.safety_policy.clone(),
        runs,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguEvaluationReport {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub frozen_source: FuguFrozenSource,
    pub protocol_fingerprint: String,
    pub tracks: Vec<FuguTrackReport>,
    pub historical_anchors: Vec<FuguHistoricalAnchorReport>,
    pub ablations: Vec<FuguAblationReport>,
    pub readiness_failures: Vec<String>,
    pub ready_for_scientific_comparison: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguTrackReport {
    pub track_id: String,
    pub kind: FuguTrackKind,
    pub required_runs: usize,
    pub observed_runs: usize,
    pub missing_runs: usize,
    pub provider_backed_runs: usize,
    pub protocol_equivalent_runs: usize,
    pub synthetic_runs: usize,
    pub summaries: Vec<FuguTreatmentSummary>,
    pub best_single_worker: Option<String>,
    pub hindsight_oracle_score: Option<f64>,
    pub orchestration_gains: Vec<FuguOrchestrationGain>,
    pub pairwise: Vec<FuguPairwiseReport>,
    pub readiness_failures: Vec<String>,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguTreatmentSummary {
    pub treatment_id: String,
    pub runs: usize,
    pub completed_runs: usize,
    pub benchmark_coverage: usize,
    pub macro_score: Option<f64>,
    pub p50_wall_time_ms: Option<u64>,
    pub p95_wall_time_ms: Option<u64>,
    pub average_model_calls: Option<f64>,
    pub average_tool_calls: Option<f64>,
    pub average_total_tokens: Option<f64>,
    pub recoveries: u64,
    pub safety_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguOrchestrationGain {
    pub treatment_id: String,
    pub best_single_worker_id: String,
    pub gain: f64,
    pub oracle_regret: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguPairwiseReport {
    pub baseline_id: String,
    pub challenger_id: String,
    pub comparisons: usize,
    pub wins: usize,
    pub ties: usize,
    pub losses: usize,
    pub score: Option<f64>,
    pub wilson_lower_bound: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguHistoricalAnchorReport {
    pub benchmark_id: String,
    pub fugu_ultra_v1_score: f64,
    pub cindx_pro_score: Option<f64>,
    pub delta: Option<f64>,
    pub protocol_equivalent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguAblationReport {
    pub treatment_id: String,
    pub base_treatment_id: String,
    pub macro_delta: Option<f64>,
    pub pairwise: FuguPairwiseReport,
}

pub fn evaluate_fugu_evaluation(
    suite: &FuguEvaluationSuite,
    observations: &[FuguEvaluationObservation],
) -> Result<FuguEvaluationReport, Vec<String>> {
    suite.validate()?;
    validate_observations(suite, observations)?;
    let protocol_fingerprint = suite.fingerprint();
    let benchmark_by_id = suite
        .benchmarks
        .iter()
        .map(|benchmark| (benchmark.id.as_str(), benchmark))
        .collect::<BTreeMap<_, _>>();
    let treatment_by_id = suite
        .treatments
        .iter()
        .map(|treatment| (treatment.id.as_str(), treatment))
        .collect::<BTreeMap<_, _>>();
    let tracks = suite
        .tracks
        .iter()
        .map(|track| summarize_track(track, observations, &benchmark_by_id, &treatment_by_id))
        .collect::<Vec<_>>();
    let historical_anchors = summarize_historical_anchors(suite, observations);
    let ablations = summarize_ablations(suite, observations, &benchmark_by_id);
    let readiness_failures = tracks
        .iter()
        .filter_map(|track| {
            suite
                .tracks
                .iter()
                .find(|spec| spec.id == track.track_id)
                .filter(|spec| spec.readiness_required && !track.ready)
                .map(|_| {
                    format!(
                        "track {} is not ready: {}",
                        track.track_id,
                        track.readiness_failures.join("; ")
                    )
                })
        })
        .collect::<Vec<_>>();
    Ok(FuguEvaluationReport {
        schema: FUGU_EVALUATION_REPORT_SCHEMA.to_string(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        frozen_source: suite.frozen_source.clone(),
        protocol_fingerprint,
        tracks,
        historical_anchors,
        ablations,
        ready_for_scientific_comparison: readiness_failures.is_empty(),
        readiness_failures,
    })
}

fn validate_observations(
    suite: &FuguEvaluationSuite,
    observations: &[FuguEvaluationObservation],
) -> Result<(), Vec<String>> {
    let fingerprint = suite.fingerprint();
    let track_by_id = suite
        .tracks
        .iter()
        .map(|track| (track.id.as_str(), track))
        .collect::<BTreeMap<_, _>>();
    let benchmark_by_id = suite
        .benchmarks
        .iter()
        .map(|benchmark| (benchmark.id.as_str(), benchmark))
        .collect::<BTreeMap<_, _>>();
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for observation in observations {
        let prefix = if observation.run_id.trim().is_empty() {
            "unnamed run"
        } else {
            observation.run_id.as_str()
        };
        if observation.schema != FUGU_EVALUATION_OBSERVATION_SCHEMA {
            errors.push(format!("{prefix}: unsupported observation schema"));
        }
        if observation.suite_id != suite.id
            || observation.suite_version != suite.version
            || observation.protocol_fingerprint != fingerprint
        {
            errors.push(format!("{prefix}: suite or protocol fingerprint drift"));
        }
        let Some(track) = track_by_id.get(observation.track_id.as_str()) else {
            errors.push(format!("{prefix}: unknown track {}", observation.track_id));
            continue;
        };
        let Some(benchmark) = benchmark_by_id.get(observation.benchmark_id.as_str()) else {
            errors.push(format!(
                "{prefix}: unknown benchmark {}",
                observation.benchmark_id
            ));
            continue;
        };
        if !track.benchmark_ids.contains(&observation.benchmark_id)
            || !track.treatment_ids.contains(&observation.treatment_id)
            || !track.seeds.contains(&observation.seed)
        {
            errors.push(format!(
                "{prefix}: observation is outside the frozen run matrix"
            ));
        }
        if !seen.insert(observation.key()) {
            errors.push(format!("{prefix}: duplicate observation key"));
        }
        if observation.candidate_id.trim().is_empty() {
            errors.push(format!("{prefix}: candidate id is empty"));
        }
        match observation.native_score {
            Some(score)
                if score.is_finite()
                    && score >= benchmark.metric_min
                    && score <= benchmark.metric_max => {}
            Some(_) => errors.push(format!("{prefix}: native score is outside metric bounds")),
            None if observation.completed => {
                errors.push(format!("{prefix}: completed run has no native score"))
            }
            None => {}
        }
        match (&track.shared_budget, &observation.budget_fingerprint) {
            (Some(budget), Some(observed)) if observed == &budget.fingerprint() => {}
            (Some(_), _) => errors.push(format!("{prefix}: shared budget fingerprint drift")),
            (None, Some(_)) => errors.push(format!("{prefix}: unexpected budget fingerprint")),
            (None, None) => {}
        }
        if observation.evidence.adapter_version.trim().is_empty()
            || observation.evidence.dataset_version.trim().is_empty()
            || observation.evidence.harness_commit.trim().is_empty()
            || !is_sha256(&observation.evidence.raw_result_sha256)
        {
            errors.push(format!("{prefix}: evidence provenance is incomplete"));
        }
        validate_safety_attestation(suite, observation, prefix, &mut errors);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_safety_attestation(
    suite: &FuguEvaluationSuite,
    observation: &FuguEvaluationObservation,
    prefix: &str,
    errors: &mut Vec<String>,
) {
    let safety = &observation.safety;
    if safety.sandbox_id.trim().is_empty()
        || !safety.ephemeral_root
        || !safety.source_workspace_read_only
        || safety.writes_outside_sandbox != 0
        || safety.destructive_actions != 0
        || safety.privileged_actions != 0
        || safety.symlink_escapes != 0
        || safety.protected_path_accesses != 0
    {
        errors.push(format!(
            "{prefix}: unsafe or incomplete sandbox attestation"
        ));
    }
    match safety.network_mode {
        FuguNetworkMode::Disabled if safety.network_destinations.is_empty() => {}
        FuguNetworkMode::Allowlisted
            if !safety.network_destinations.is_empty()
                && safety.network_destinations.iter().all(|destination| {
                    suite.safety_policy.network_allowlist.contains(destination)
                }) => {}
        _ => errors.push(format!(
            "{prefix}: network use was not explicitly allowlisted"
        )),
    }
}

fn summarize_track(
    track: &FuguEvaluationTrack,
    observations: &[FuguEvaluationObservation],
    benchmarks: &BTreeMap<&str, &FuguBenchmark>,
    treatments: &BTreeMap<&str, &FuguTreatment>,
) -> FuguTrackReport {
    let runs = observations
        .iter()
        .filter(|run| run.track_id == track.id)
        .collect::<Vec<_>>();
    let required_runs = track
        .benchmark_ids
        .len()
        .saturating_mul(track.treatment_ids.len())
        .saturating_mul(track.seeds.len());
    let observed_keys = runs.iter().map(|run| run.key()).collect::<BTreeSet<_>>();
    let expected_keys = track
        .benchmark_ids
        .iter()
        .flat_map(|benchmark_id| {
            track.treatment_ids.iter().flat_map(move |treatment_id| {
                track.seeds.iter().copied().map(move |seed| {
                    (
                        track.id.as_str(),
                        benchmark_id.as_str(),
                        treatment_id.as_str(),
                        seed,
                    )
                })
            })
        })
        .collect::<BTreeSet<_>>();
    let missing_runs = expected_keys.difference(&observed_keys).count();
    let summaries = track
        .treatment_ids
        .iter()
        .map(|treatment_id| summarize_treatment(treatment_id, &runs, benchmarks))
        .collect::<Vec<_>>();
    let best_single_worker = summaries
        .iter()
        .filter(|summary| {
            treatments
                .get(summary.treatment_id.as_str())
                .is_some_and(|treatment| treatment.kind == FuguTreatmentKind::SingleWorker)
        })
        .filter_map(|summary| summary.macro_score.map(|score| (summary, score)))
        .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
        .map(|(summary, _)| summary.treatment_id.clone());
    let hindsight_oracle_score = hindsight_oracle_score(track, &runs, benchmarks, treatments);
    let orchestration_ids = summaries
        .iter()
        .filter(|summary| {
            treatments
                .get(summary.treatment_id.as_str())
                .is_some_and(|treatment| {
                    matches!(
                        treatment.kind,
                        FuguTreatmentKind::CindxFast
                            | FuguTreatmentKind::CindxAuto
                            | FuguTreatmentKind::CindxPro
                    )
                })
        })
        .collect::<Vec<_>>();
    let orchestration_gains = best_single_worker
        .as_ref()
        .zip(
            summaries
                .iter()
                .find(|summary| Some(&summary.treatment_id) == best_single_worker.as_ref())
                .and_then(|summary| summary.macro_score),
        )
        .map(|(best_id, best_score)| {
            orchestration_ids
                .iter()
                .filter_map(|summary| {
                    summary.macro_score.map(|score| FuguOrchestrationGain {
                        treatment_id: summary.treatment_id.clone(),
                        best_single_worker_id: best_id.clone(),
                        gain: score - best_score,
                        oracle_regret: hindsight_oracle_score.map(|oracle| oracle - score),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let pairwise = best_single_worker
        .as_ref()
        .map(|best_id| {
            orchestration_ids
                .iter()
                .map(|summary| {
                    summarize_pairwise(best_id, &summary.treatment_id, &runs, benchmarks)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let provider_backed_runs = runs.iter().filter(|run| run.provider_backed).count();
    let protocol_equivalent_runs = runs
        .iter()
        .filter(|run| run.protocol_equivalent_to_fugu_v1)
        .count();
    let synthetic_runs = runs.iter().filter(|run| run.evidence.synthetic).count();
    let mut readiness_failures = Vec::new();
    if missing_runs != 0 || runs.len() != required_runs {
        readiness_failures.push(format!(
            "expected {required_runs} matched runs, observed {} with {missing_runs} missing",
            runs.len()
        ));
    }
    if provider_backed_runs != runs.len() || runs.is_empty() {
        readiness_failures.push("all runs must be provider-backed".to_string());
    }
    if synthetic_runs != 0 {
        readiness_failures.push("synthetic observations cannot satisfy readiness".to_string());
    }
    if track.kind != FuguTrackKind::CausalAblation && protocol_equivalent_runs != runs.len() {
        readiness_failures
            .push("parity tracks require per-run Fugu v1 protocol equivalence".to_string());
    }
    if runs.iter().any(|run| run.safety_violations != 0) {
        readiness_failures.push("safety violations were observed".to_string());
    }
    FuguTrackReport {
        track_id: track.id.clone(),
        kind: track.kind,
        required_runs,
        observed_runs: runs.len(),
        missing_runs,
        provider_backed_runs,
        protocol_equivalent_runs,
        synthetic_runs,
        summaries,
        best_single_worker,
        hindsight_oracle_score,
        orchestration_gains,
        pairwise,
        ready: readiness_failures.is_empty(),
        readiness_failures,
    }
}

fn summarize_treatment(
    treatment_id: &str,
    runs: &[&FuguEvaluationObservation],
    benchmarks: &BTreeMap<&str, &FuguBenchmark>,
) -> FuguTreatmentSummary {
    let selected = runs
        .iter()
        .copied()
        .filter(|run| run.treatment_id == treatment_id)
        .collect::<Vec<_>>();
    let benchmark_scores =
        selected
            .iter()
            .fold(BTreeMap::<&str, Vec<f64>>::new(), |mut scores, run| {
                if let Some(benchmark) = benchmarks.get(run.benchmark_id.as_str()) {
                    scores
                        .entry(run.benchmark_id.as_str())
                        .or_default()
                        .push(run.normalized_score(benchmark));
                }
                scores
            });
    let macro_score = (!benchmark_scores.is_empty()).then(|| {
        benchmark_scores
            .values()
            .map(|scores| scores.iter().sum::<f64>() / scores.len() as f64)
            .sum::<f64>()
            / benchmark_scores.len() as f64
    });
    let mut latencies = selected
        .iter()
        .map(|run| run.wall_time_ms)
        .collect::<Vec<_>>();
    latencies.sort_unstable();
    let average = |value: fn(&FuguEvaluationObservation) -> u64| {
        (!selected.is_empty()).then(|| {
            selected.iter().map(|run| value(run) as f64).sum::<f64>() / selected.len() as f64
        })
    };
    FuguTreatmentSummary {
        treatment_id: treatment_id.to_string(),
        runs: selected.len(),
        completed_runs: selected.iter().filter(|run| run.completed).count(),
        benchmark_coverage: benchmark_scores.len(),
        macro_score,
        p50_wall_time_ms: percentile(&latencies, 0.50),
        p95_wall_time_ms: percentile(&latencies, 0.95),
        average_model_calls: average(|run| run.model_calls),
        average_tool_calls: average(|run| run.tool_calls),
        average_total_tokens: average(|run| run.total_tokens),
        recoveries: selected.iter().map(|run| run.recoveries).sum(),
        safety_violations: selected.iter().map(|run| run.safety_violations).sum(),
    }
}

fn hindsight_oracle_score(
    track: &FuguEvaluationTrack,
    runs: &[&FuguEvaluationObservation],
    benchmarks: &BTreeMap<&str, &FuguBenchmark>,
    treatments: &BTreeMap<&str, &FuguTreatment>,
) -> Option<f64> {
    let worker_ids = treatments
        .values()
        .filter(|treatment| treatment.kind == FuguTreatmentKind::SingleWorker)
        .map(|treatment| treatment.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut values = Vec::new();
    for benchmark_id in &track.benchmark_ids {
        let benchmark = benchmarks.get(benchmark_id.as_str())?;
        for seed in &track.seeds {
            let best = runs
                .iter()
                .filter(|run| {
                    run.benchmark_id == *benchmark_id
                        && run.seed == *seed
                        && worker_ids.contains(run.treatment_id.as_str())
                })
                .map(|run| run.normalized_score(benchmark))
                .max_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
            if let Some(best) = best {
                values.push(best);
            }
        }
    }
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn summarize_pairwise(
    baseline_id: &str,
    challenger_id: &str,
    runs: &[&FuguEvaluationObservation],
    benchmarks: &BTreeMap<&str, &FuguBenchmark>,
) -> FuguPairwiseReport {
    let by_key = runs
        .iter()
        .map(|run| {
            (
                (
                    run.benchmark_id.as_str(),
                    run.treatment_id.as_str(),
                    run.seed,
                ),
                *run,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let keys = runs
        .iter()
        .filter(|run| run.treatment_id == baseline_id)
        .map(|run| (run.benchmark_id.as_str(), run.seed))
        .collect::<BTreeSet<_>>();
    let mut wins = 0usize;
    let mut ties = 0usize;
    let mut losses = 0usize;
    for (benchmark_id, seed) in keys {
        let Some(benchmark) = benchmarks.get(benchmark_id) else {
            continue;
        };
        let Some(baseline) = by_key.get(&(benchmark_id, baseline_id, seed)) else {
            continue;
        };
        let Some(challenger) = by_key.get(&(benchmark_id, challenger_id, seed)) else {
            continue;
        };
        let baseline_score = baseline.normalized_score(benchmark);
        let challenger_score = challenger.normalized_score(benchmark);
        if challenger_score > baseline_score + f64::EPSILON * 8.0 {
            wins += 1;
        } else if baseline_score > challenger_score + f64::EPSILON * 8.0 {
            losses += 1;
        } else {
            ties += 1;
        }
    }
    let comparisons = wins + ties + losses;
    let successes = wins as f64 + ties as f64 * 0.5;
    FuguPairwiseReport {
        baseline_id: baseline_id.to_string(),
        challenger_id: challenger_id.to_string(),
        comparisons,
        wins,
        ties,
        losses,
        score: (comparisons != 0).then_some(successes / comparisons as f64),
        wilson_lower_bound: (comparisons != 0)
            .then_some(wilson_lower_bound(successes, comparisons)),
    }
}

fn summarize_historical_anchors(
    suite: &FuguEvaluationSuite,
    observations: &[FuguEvaluationObservation],
) -> Vec<FuguHistoricalAnchorReport> {
    let quality_track = suite
        .tracks
        .iter()
        .find(|track| track.kind == FuguTrackKind::QualityFirst);
    suite
        .benchmarks
        .iter()
        .map(|benchmark| {
            let runs = quality_track
                .map(|track| {
                    observations
                        .iter()
                        .filter(|run| {
                            run.track_id == track.id
                                && run.benchmark_id == benchmark.id
                                && run.treatment_id == "cindx_pro"
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let score = (!runs.is_empty()
                && runs.len() == quality_track.map(|track| track.seeds.len()).unwrap_or(0)
                && runs.iter().all(|run| run.completed))
            .then(|| {
                runs.iter().filter_map(|run| run.native_score).sum::<f64>() / runs.len() as f64
            });
            let protocol_equivalent =
                !runs.is_empty() && runs.iter().all(|run| run.protocol_equivalent_to_fugu_v1);
            FuguHistoricalAnchorReport {
                benchmark_id: benchmark.id.clone(),
                fugu_ultra_v1_score: benchmark.fugu_ultra_v1_score,
                cindx_pro_score: score,
                delta: score.map(|score| score - benchmark.fugu_ultra_v1_score),
                protocol_equivalent,
            }
        })
        .collect()
}

fn summarize_ablations(
    suite: &FuguEvaluationSuite,
    observations: &[FuguEvaluationObservation],
    benchmarks: &BTreeMap<&str, &FuguBenchmark>,
) -> Vec<FuguAblationReport> {
    let Some(track) = suite
        .tracks
        .iter()
        .find(|track| track.kind == FuguTrackKind::CausalAblation)
    else {
        return Vec::new();
    };
    let runs = observations
        .iter()
        .filter(|run| run.track_id == track.id)
        .collect::<Vec<_>>();
    suite
        .treatments
        .iter()
        .filter(|treatment| treatment.kind == FuguTreatmentKind::Ablation)
        .filter_map(|treatment| {
            let base = treatment.base_treatment_id.as_ref()?;
            let ablation = summarize_treatment(&treatment.id, &runs, benchmarks);
            let baseline = summarize_treatment(base, &runs, benchmarks);
            Some(FuguAblationReport {
                treatment_id: treatment.id.clone(),
                base_treatment_id: base.clone(),
                macro_delta: ablation
                    .macro_score
                    .zip(baseline.macro_score)
                    .map(|(ablation, baseline)| ablation - baseline),
                pairwise: summarize_pairwise(base, &treatment.id, &runs, benchmarks),
            })
        })
        .collect()
}

pub fn render_fugu_evaluation_card(
    suite: &FuguEvaluationSuite,
    report: &FuguEvaluationReport,
) -> String {
    let mut output = String::new();
    output.push_str("# Cindx Fugu v1 Evaluation Card\n\n");
    output.push_str(&format!(
        "Frozen source: [{} {}]({}) (`{}`).\n\n",
        report.frozen_source.title,
        report.frozen_source.version,
        report.frozen_source.url,
        report.frozen_source.results_table
    ));
    output.push_str(
        "> Fugu scores are historical anchors. A direct parity claim is valid only when every run is marked protocol-equivalent and the complete matched matrix is present.\n\n",
    );
    output.push_str("## Safety\n\n");
    output.push_str(&format!(
        "Default mode is `{:?}`. Execution requires explicit opt-in; source workspaces are read-only, networking is disabled by default, and destructive, privileged, or sandbox-escaping observations are rejected.\n\n",
        suite.safety_policy.default_mode
    ));
    output.push_str("## Readiness\n\n");
    output.push_str("| Track | Observed / Required | Ready |\n|---|---:|---|\n");
    for track in &report.tracks {
        output.push_str(&format!(
            "| {} | {} / {} | {} |\n",
            track.track_id,
            track.observed_runs,
            track.required_runs,
            if track.ready { "yes" } else { "no" }
        ));
    }
    output.push_str("\n## Historical anchors\n\n");
    output.push_str("| Benchmark | Fugu Ultra v1 | Cindx Pro | Delta | Equivalent |\n|---|---:|---:|---:|---|\n");
    for anchor in &report.historical_anchors {
        output.push_str(&format!(
            "| {} | {:.1} | {} | {} | {} |\n",
            anchor.benchmark_id,
            anchor.fugu_ultra_v1_score,
            format_optional(anchor.cindx_pro_score),
            format_optional(anchor.delta),
            if anchor.protocol_equivalent {
                "yes"
            } else {
                "no"
            }
        ));
    }
    output.push_str("\n## Causal ablations\n\n");
    output.push_str("| Ablation | Base | Macro delta | Paired score |\n|---|---|---:|---:|\n");
    for ablation in &report.ablations {
        output.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            ablation.treatment_id,
            ablation.base_treatment_id,
            format_optional(ablation.macro_delta),
            format_optional(ablation.pairwise.score)
        ));
    }
    if !report.readiness_failures.is_empty() {
        output.push_str("\n## Blocking evidence\n\n");
        for failure in &report.readiness_failures {
            output.push_str(&format!("- {failure}\n"));
        }
    }
    output
}

fn format_optional(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "unmeasured".to_string())
}

fn percentile(values: &[u64], percentile: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    values.get(index).copied()
}

fn wilson_lower_bound(successes: f64, trials: usize) -> f64 {
    if trials == 0 {
        return 0.0;
    }
    let z = 1.959_963_984_540_054;
    let n = trials as f64;
    let p = (successes / n).clamp(0.0, 1.0);
    let denominator = 1.0 + z * z / n;
    let center = p + z * z / (2.0 * n);
    let margin = z * ((p * (1.0 - p) + z * z / (4.0 * n)) / n).sqrt();
    ((center - margin) / denominator).clamp(0.0, 1.0)
}

pub(crate) fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUITE: &str = include_str!("../../../benchmarks/fugu/fugu-v1.json");

    fn suite() -> FuguEvaluationSuite {
        parse_fugu_evaluation_suite(SUITE).expect("suite parses")
    }

    fn observation(
        suite: &FuguEvaluationSuite,
        track: &FuguEvaluationTrack,
        benchmark_id: &str,
        treatment_id: &str,
        seed: u64,
    ) -> FuguEvaluationObservation {
        FuguEvaluationObservation {
            schema: FUGU_EVALUATION_OBSERVATION_SCHEMA.to_string(),
            suite_id: suite.id.clone(),
            suite_version: suite.version,
            protocol_fingerprint: suite.fingerprint(),
            run_id: format!("{}:{benchmark_id}:{treatment_id}:{seed}", track.id),
            track_id: track.id.clone(),
            benchmark_id: benchmark_id.to_string(),
            treatment_id: treatment_id.to_string(),
            seed,
            candidate_id: treatment_id.to_string(),
            provider_backed: true,
            protocol_equivalent_to_fugu_v1: true,
            completed: true,
            native_score: Some(if treatment_id == "cindx_pro" {
                80.0
            } else {
                70.0
            }),
            wall_time_ms: 1_000,
            ttft_ms: Some(100),
            model_calls: 1,
            tool_calls: 1,
            total_tokens: 1_000,
            recoveries: 0,
            safety_violations: 0,
            budget_fingerprint: track
                .shared_budget
                .as_ref()
                .map(FuguSharedBudget::fingerprint),
            evidence: FuguEvidenceProvenance {
                adapter_version: "test-adapter-v1".to_string(),
                dataset_version: "frozen-test".to_string(),
                harness_commit: "0123456789abcdef".to_string(),
                raw_result_sha256: "a".repeat(64),
                synthetic: false,
            },
            safety: FuguSafetyAttestation {
                sandbox_id: format!("test-{seed}"),
                ephemeral_root: true,
                source_workspace_read_only: true,
                network_mode: FuguNetworkMode::Disabled,
                network_destinations: Vec::new(),
                writes_outside_sandbox: 0,
                destructive_actions: 0,
                privileged_actions: 0,
                symlink_escapes: 0,
                protected_path_accesses: 0,
            },
            failure_reason: None,
        }
    }

    #[test]
    fn frozen_manifest_is_valid_and_plan_is_non_executable() {
        let suite = suite();
        suite.validate().expect("valid frozen suite");
        let plan = build_fugu_evaluation_run_plan(&suite).expect("valid plan");
        assert!(!plan.execution_enabled);
        assert_eq!(plan.runs.len(), 759);
        assert!(plan.runs.iter().all(|run| run.status == "planned"));
    }

    #[test]
    fn empty_evidence_is_reported_without_fabricated_scores() {
        let suite = suite();
        let report = evaluate_fugu_evaluation(&suite, &[]).expect("report builds");
        assert!(!report.ready_for_scientific_comparison);
        assert!(report
            .historical_anchors
            .iter()
            .all(|anchor| anchor.cindx_pro_score.is_none()));
    }

    #[test]
    fn complete_matched_matrix_can_satisfy_readiness() {
        let suite = suite();
        let mut observations = Vec::new();
        for track in &suite.tracks {
            for benchmark_id in &track.benchmark_ids {
                for treatment_id in &track.treatment_ids {
                    for seed in &track.seeds {
                        observations.push(observation(
                            &suite,
                            track,
                            benchmark_id,
                            treatment_id,
                            *seed,
                        ));
                    }
                }
            }
        }
        let report = evaluate_fugu_evaluation(&suite, &observations).expect("complete report");
        assert!(report.ready_for_scientific_comparison);
        assert!(report.tracks.iter().all(|track| track.ready));
        assert!(report
            .historical_anchors
            .iter()
            .all(|anchor| anchor.cindx_pro_score == Some(80.0)));
    }

    #[test]
    fn unsafe_observation_is_rejected() {
        let suite = suite();
        let track = &suite.tracks[0];
        let mut run = observation(
            &suite,
            track,
            &track.benchmark_ids[0],
            &track.treatment_ids[0],
            track.seeds[0],
        );
        run.safety.writes_outside_sandbox = 1;
        let errors = evaluate_fugu_evaluation(&suite, &[run]).expect_err("unsafe run rejected");
        assert!(errors.iter().any(|error| error.contains("unsafe")));
    }

    #[test]
    fn budget_drift_is_rejected() {
        let suite = suite();
        let track = suite
            .tracks
            .iter()
            .find(|track| track.kind == FuguTrackKind::IsoBudget)
            .expect("iso track");
        let mut run = observation(
            &suite,
            track,
            &track.benchmark_ids[0],
            &track.treatment_ids[0],
            track.seeds[0],
        );
        run.budget_fingerprint = Some("b".repeat(64));
        let errors = evaluate_fugu_evaluation(&suite, &[run]).expect_err("drift rejected");
        assert!(errors.iter().any(|error| error.contains("budget")));
    }

    #[test]
    fn evaluation_card_exposes_safety_and_missing_evidence() {
        let suite = suite();
        let report = evaluate_fugu_evaluation(&suite, &[]).expect("report builds");
        let card = render_fugu_evaluation_card(&suite, &report);
        assert!(card.contains("## Safety"));
        assert!(card.contains("historical anchors"));
        assert!(card.contains("unmeasured"));
    }
}
