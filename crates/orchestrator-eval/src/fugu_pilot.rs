use crate::fugu_evaluation::{
    is_sha256, sha256_hex, validate_safety_policy, FuguEvaluationObservation,
    FuguEvaluationRunPlan, FuguEvidenceProvenance, FuguFrozenSource, FuguNetworkMode,
    FuguPlannedRun, FuguSafetyAttestation, FuguSafetyPolicy, FUGU_EVALUATION_OBSERVATION_SCHEMA,
    FUGU_EVALUATION_RUN_PLAN_SCHEMA,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const FUGU_PILOT_SUITE_SCHEMA: &str = "cindx.fugu-pilot-suite.v1";
pub const FUGU_PILOT_PROTOCOL_SCHEMA: &str = "cindx.fugu-pilot-protocol.v1";
pub const FUGU_PILOT_BENCHMARK_ID: &str = "gpqa_diamond";
pub const FUGU_PILOT_REPORT_SCHEMA: &str = "cindx.fugu-pilot-report.v1";
pub const FUGU_PILOT_EXTERNAL_REPORT_SCHEMA: &str = "cindx.external_effect_eval.raw.v3";

/// Frozen bounded Fugu pilot suite: one pinned benchmark sample, a small
/// treatment set, and a single replicate. Harness-link verification only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotSuite {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub description: String,
    pub frozen_source: FuguFrozenSource,
    pub safety_policy: FuguSafetyPolicy,
    pub benchmark: FuguPilotBenchmark,
    pub treatments: Vec<FuguPilotTreatment>,
    pub track: FuguPilotTrack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotBenchmark {
    pub id: String,
    pub display_name: String,
    pub capability: String,
    pub metric: String,
    pub metric_min: f64,
    pub metric_max: f64,
    pub adapter: String,
    pub case_authority: FuguPilotCaseAuthority,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotCaseAuthority {
    pub source_url: String,
    pub revision: String,
    pub file_sha256: String,
    pub baseline_manifest_sha256: String,
    pub cases_per_domain: u32,
    pub case_count: u32,
    pub selection_rule: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotTreatment {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub control: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotTrack {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub seeds: Vec<u64>,
}

/// Frozen pilot protocol manifest. Binds the suite digest, case authority,
/// prompt profiles, harness entrypoint, run order, and budgets. Carries no
/// execution authority of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotProtocol {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub suite: FuguPilotProtocolSuiteBinding,
    pub case_authority: FuguPilotProtocolCaseBinding,
    pub prompt_profiles: FuguPilotPromptProfiles,
    pub harness: FuguPilotHarness,
    pub run_order: Vec<String>,
    pub run_budget: FuguPilotRunBudget,
    pub campaign_budget: FuguPilotCampaignBudget,
    pub execution_authorized: bool,
    pub claims: FuguPilotClaims,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotProtocolSuiteBinding {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotProtocolCaseBinding {
    pub source_revision: String,
    pub file_sha256: String,
    pub baseline_manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotPromptProfiles {
    pub direct_sha256: String,
    pub auto_sha256: String,
    pub pro_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotHarness {
    pub entrypoint: String,
    pub mode: String,
    pub rotation: String,
    pub treatment_bindings: Vec<FuguPilotTreatmentBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotTreatmentBinding {
    pub treatment_id: String,
    pub harness_label: String,
    pub control: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotRunBudget {
    pub max_wall_time_ms: u64,
    pub treatment_deadline_per_case_ms: u64,
    pub model_call_timeout_ms: u64,
    pub max_output_tokens_per_call: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotCampaignBudget {
    pub runs: u32,
    pub cases_per_run: u32,
    pub max_wall_time_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotClaims {
    pub purpose: String,
    pub parity_claim_allowed: bool,
    pub uplift_claim_allowed: bool,
    pub promotion_allowed: bool,
    pub notes: Vec<String>,
}

/// Per-run aggregate fed into the projection from the harness-side report
/// conversion. One cell per planned run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuguPilotCellAggregate {
    pub run_id: String,
    pub treatment_id: String,
    pub seed: u64,
    pub cases_total: u32,
    pub cases_correct: u32,
    pub completed: bool,
    pub wall_time_ms: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub total_tokens: u64,
    pub recoveries: u64,
    pub safety_violations: u64,
    pub failure_reason: Option<String>,
    pub evidence: FuguEvidenceProvenance,
    pub safety: FuguSafetyAttestation,
}

/// Projection-facing mirror of the desktop external-effect raw report v3 JSON.
/// The field names are the wire contract; a drifted writer fails to decode or
/// bind here.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FuguPilotExternalReport {
    pub schema: String,
    pub git_commit: String,
    #[serde(default)]
    pub provider_endpoint: String,
    pub sources: Vec<FuguPilotExternalSource>,
    pub runs: Vec<FuguPilotExternalRun>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FuguPilotExternalSource {
    pub benchmark: String,
    pub source_url: String,
    pub revision: String,
    pub file_sha256: String,
    pub sample_count: usize,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FuguPilotExternalRun {
    pub benchmark: String,
    pub case_id: String,
    pub treatment: String,
    pub succeeded: bool,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub exact_score: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguPilotTreatmentResult {
    pub treatment_id: String,
    pub control: bool,
    pub observed_runs: usize,
    pub completed_runs: usize,
    pub score_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuguPilotReport {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub protocol_fingerprint: String,
    pub planned_runs: usize,
    pub observed_runs: usize,
    pub ready: bool,
    pub readiness_failures: Vec<String>,
    pub treatments: Vec<FuguPilotTreatmentResult>,
    pub claim_guard: String,
}

impl FuguPilotSuite {
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        sha256_hex(&encoded)
    }

    pub fn run_id(&self, treatment_id: &str, seed: u64) -> String {
        format!(
            "{}:{}:{}:{}",
            self.track.id, self.benchmark.id, treatment_id, seed
        )
    }

    pub fn planned_run_ids(&self) -> Vec<String> {
        self.track
            .seeds
            .iter()
            .flat_map(|seed| {
                self.treatments
                    .iter()
                    .map(move |treatment| self.run_id(&treatment.id, *seed))
            })
            .collect()
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema != FUGU_PILOT_SUITE_SCHEMA {
            errors.push(format!("unsupported pilot suite schema {}", self.schema));
        }
        if self.id.trim().is_empty() || self.version == 0 || self.description.trim().is_empty() {
            errors.push("pilot suite identity is incomplete".to_string());
        }
        if self.frozen_source.version != "v1"
            || !self.frozen_source.url.contains("2606.21228v1")
            || self.frozen_source.results_table.trim().is_empty()
        {
            errors.push("pilot source must stay frozen to technical report v1".to_string());
        }
        validate_safety_policy(&self.safety_policy, &mut errors);

        let benchmark = &self.benchmark;
        if benchmark.id != FUGU_PILOT_BENCHMARK_ID
            || benchmark.adapter != "evalscope_gpqa_diamond"
            || benchmark.metric != "accuracy_percent"
        {
            errors.push(format!(
                "pilot benchmark must remain the pinned GPQA-Diamond configuration, found {}",
                benchmark.id
            ));
        }
        if benchmark.display_name.trim().is_empty()
            || benchmark.capability.trim().is_empty()
            || !benchmark.metric_min.is_finite()
            || !benchmark.metric_max.is_finite()
            || benchmark.metric_max <= benchmark.metric_min
        {
            errors.push("pilot benchmark has an incomplete or invalid metric scale".to_string());
        }
        let authority = &benchmark.case_authority;
        if !authority.source_url.contains("idavidrein/gpqa")
            || authority.revision.len() != 40
            || !authority
                .revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !is_sha256(&authority.file_sha256)
            || !is_sha256(&authority.baseline_manifest_sha256)
            || authority.cases_per_domain == 0
            || authority.case_count == 0
            || authority.selection_rule.trim().is_empty()
        {
            errors.push("pilot case authority is incomplete or malformed".to_string());
        }

        let treatment_ids: BTreeSet<&str> = self
            .treatments
            .iter()
            .map(|treatment| treatment.id.as_str())
            .collect();
        let expected: BTreeSet<&str> = ["cindx_fast", "cindx_auto", "cindx_pro"]
            .into_iter()
            .collect();
        if treatment_ids != expected {
            errors.push(format!(
                "pilot treatments drifted; expected cindx_fast/cindx_auto/cindx_pro, found [{}]",
                treatment_ids.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        let controls = self
            .treatments
            .iter()
            .filter(|treatment| treatment.control)
            .count();
        if controls != 1
            || !self
                .treatments
                .iter()
                .any(|treatment| treatment.control && treatment.id == "cindx_fast")
        {
            errors.push(
                "pilot requires exactly one control treatment and it must be cindx_fast"
                    .to_string(),
            );
        }
        for treatment in &self.treatments {
            if treatment.label.trim().is_empty()
                || treatment.kind.trim().is_empty()
                || treatment.description.trim().is_empty()
            {
                errors.push(format!("pilot treatment {} is incomplete", treatment.id));
            }
        }

        if self.track.id != "fugu_pilot_quality_first"
            || self.track.kind != "quality_first"
            || self.track.description.trim().is_empty()
        {
            errors.push("pilot track must be the frozen quality-first track".to_string());
        }
        if self.track.seeds != vec![0] {
            errors.push("pilot track must remain a single-replicate seed [0]".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl FuguPilotProtocol {
    /// Validates the manifest against the exact suite bytes it claims to bind.
    /// Any digest drift, authorization flip, or ordering change fails closed.
    pub fn validate(&self, suite_bytes: &[u8], suite: &FuguPilotSuite) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema != FUGU_PILOT_PROTOCOL_SCHEMA {
            errors.push(format!("unsupported pilot protocol schema {}", self.schema));
        }
        if self.id.trim().is_empty() || self.version == 0 {
            errors.push("pilot protocol identity is incomplete".to_string());
        }
        if sha256_hex(suite_bytes) != self.suite.sha256 {
            errors.push("pilot protocol does not bind the exact suite bytes".to_string());
        }
        if self.suite.path != "benchmarks/fugu/fugu-pilot-v1.json" {
            errors.push(format!("pilot suite path drifted: {}", self.suite.path));
        }
        let authority = &suite.benchmark.case_authority;
        if self.case_authority.file_sha256 != authority.file_sha256
            || self.case_authority.baseline_manifest_sha256 != authority.baseline_manifest_sha256
            || self.case_authority.source_revision != authority.revision
        {
            errors.push("pilot protocol case authority drifted from the suite".to_string());
        }
        if !is_sha256(&self.prompt_profiles.direct_sha256)
            || !is_sha256(&self.prompt_profiles.auto_sha256)
            || !is_sha256(&self.prompt_profiles.pro_sha256)
        {
            errors.push("pilot prompt profile digests are malformed".to_string());
        }
        if self.harness.entrypoint.trim().is_empty() || self.harness.mode.trim().is_empty() {
            errors.push("pilot harness entrypoint is incomplete".to_string());
        }
        let bindings: BTreeSet<&str> = self
            .harness
            .treatment_bindings
            .iter()
            .map(|binding| binding.treatment_id.as_str())
            .collect();
        let treatment_ids: BTreeSet<&str> = suite
            .treatments
            .iter()
            .map(|treatment| treatment.id.as_str())
            .collect();
        if bindings != treatment_ids {
            errors.push(
                "pilot harness treatment bindings do not cover the suite treatments".to_string(),
            );
        }
        let binding_controls = self
            .harness
            .treatment_bindings
            .iter()
            .filter(|binding| binding.control)
            .count();
        if binding_controls != 1 {
            errors.push("pilot harness bindings must mark exactly one control".to_string());
        }
        for binding in &self.harness.treatment_bindings {
            if binding.harness_label.trim().is_empty() {
                errors.push(format!(
                    "pilot binding {} lacks a harness label",
                    binding.treatment_id
                ));
            }
        }
        if self.run_order != suite.planned_run_ids() {
            errors.push("pilot run order must match the planned run ids exactly".to_string());
        }
        if self.campaign_budget.runs as usize != self.run_order.len()
            || self.campaign_budget.cases_per_run != suite.benchmark.case_authority.case_count
        {
            errors.push("pilot campaign budget must match the frozen matrix".to_string());
        }
        if self.execution_authorized {
            errors.push(
                "pilot protocol must stay execution_authorized=false; authorization is a separate one-shot step".to_string(),
            );
        }
        if self.claims.parity_claim_allowed
            || self.claims.uplift_claim_allowed
            || self.claims.promotion_allowed
        {
            errors.push("pilot claims must remain harness-link verification only".to_string());
        }
        if self.claims.purpose != "harness_link_verification" {
            errors.push(format!(
                "pilot claim purpose drifted: {}",
                self.claims.purpose
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

pub fn parse_fugu_pilot_suite(source: &str) -> Result<FuguPilotSuite, String> {
    serde_json::from_str(source).map_err(|error| format!("invalid Fugu pilot suite JSON: {error}"))
}

pub fn parse_fugu_pilot_protocol(source: &str) -> Result<FuguPilotProtocol, String> {
    serde_json::from_str(source)
        .map_err(|error| format!("invalid Fugu pilot protocol JSON: {error}"))
}

pub fn build_fugu_pilot_run_plan(
    suite: &FuguPilotSuite,
) -> Result<FuguEvaluationRunPlan, Vec<String>> {
    suite.validate()?;
    Ok(FuguEvaluationRunPlan {
        schema: FUGU_EVALUATION_RUN_PLAN_SCHEMA.to_string(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        protocol_fingerprint: suite.fingerprint(),
        execution_enabled: false,
        safety_policy: suite.safety_policy.clone(),
        runs: suite
            .planned_run_ids()
            .into_iter()
            .zip(suite.track.seeds.iter().flat_map(|seed| {
                suite
                    .treatments
                    .iter()
                    .map(move |treatment| (treatment.id.clone(), *seed))
            }))
            .map(|(run_id, (treatment_id, seed))| FuguPlannedRun {
                run_id,
                track_id: suite.track.id.clone(),
                benchmark_id: suite.benchmark.id.clone(),
                treatment_id,
                seed,
                budget_fingerprint: None,
                status: "planned".to_string(),
            })
            .collect(),
    })
}

/// Projects per-run harness aggregates into Fugu observations. Fails closed on
/// duplicate, missing, or mis-bound cells instead of scoring partial evidence.
pub fn project_fugu_pilot_cells(
    suite: &FuguPilotSuite,
    cells: &[FuguPilotCellAggregate],
) -> Result<Vec<FuguEvaluationObservation>, Vec<String>> {
    suite.validate()?;
    let mut errors = Vec::new();
    let planned = suite.planned_run_ids();
    let mut seen = BTreeSet::new();
    let mut observations = Vec::new();
    for cell in cells {
        if !planned.contains(&cell.run_id) || !seen.insert(cell.run_id.clone()) {
            errors.push(format!(
                "pilot cell {} is not a planned run or is duplicated",
                cell.run_id
            ));
            continue;
        }
        if !suite.treatments.iter().any(|treatment| {
            treatment.id == cell.treatment_id
                && suite.run_id(&treatment.id, cell.seed) == cell.run_id
        }) {
            errors.push(format!(
                "pilot cell {} binds the wrong treatment or seed",
                cell.run_id
            ));
            continue;
        }
        if cell.cases_total != suite.benchmark.case_authority.case_count {
            errors.push(format!(
                "pilot cell {} observed {} cases; the frozen sample has {}",
                cell.run_id, cell.cases_total, suite.benchmark.case_authority.case_count
            ));
            continue;
        }
        if cell.cases_correct > cell.cases_total {
            errors.push(format!(
                "pilot cell {} reports more correct than observed cases",
                cell.run_id
            ));
            continue;
        }
        if cell.safety_violations != 0 || !cell.completed {
            errors.push(format!(
                "pilot cell {} is ineligible: completed={} safety_violations={}",
                cell.run_id, cell.completed, cell.safety_violations
            ));
        }
        let score_percent = if cell.completed && cell.safety_violations == 0 {
            f64::from(cell.cases_correct) / f64::from(cell.cases_total) * 100.0
        } else {
            0.0
        };
        observations.push(FuguEvaluationObservation {
            schema: FUGU_EVALUATION_OBSERVATION_SCHEMA.to_string(),
            suite_id: suite.id.clone(),
            suite_version: suite.version,
            protocol_fingerprint: suite.fingerprint(),
            run_id: cell.run_id.clone(),
            track_id: suite.track.id.clone(),
            benchmark_id: suite.benchmark.id.clone(),
            treatment_id: cell.treatment_id.clone(),
            seed: cell.seed,
            candidate_id: cell.evidence.harness_commit.clone(),
            provider_backed: true,
            protocol_equivalent_to_fugu_v1: false,
            completed: cell.completed,
            native_score: if cell.completed && cell.safety_violations == 0 {
                Some(score_percent)
            } else {
                None
            },
            wall_time_ms: cell.wall_time_ms,
            ttft_ms: None,
            model_calls: cell.model_calls,
            tool_calls: cell.tool_calls,
            total_tokens: cell.total_tokens,
            recoveries: cell.recoveries,
            safety_violations: cell.safety_violations,
            budget_fingerprint: None,
            evidence: cell.evidence.clone(),
            safety: cell.safety.clone(),
            failure_reason: cell.failure_reason.clone(),
        });
    }
    for run_id in &planned {
        if !seen.contains(run_id) {
            errors.push(format!("pilot cell missing for planned run {run_id}"));
        }
    }
    if errors.is_empty() {
        Ok(observations)
    } else {
        Err(errors)
    }
}

pub fn evaluate_fugu_pilot(
    suite: &FuguPilotSuite,
    observations: &[FuguEvaluationObservation],
) -> Result<FuguPilotReport, Vec<String>> {
    suite.validate()?;
    let fingerprint = suite.fingerprint();
    let planned = suite.planned_run_ids();
    let mut readiness_failures = Vec::new();
    let mut matched = Vec::new();
    for observation in observations {
        if observation.protocol_fingerprint != fingerprint {
            readiness_failures.push(format!(
                "observation {} carries a drifted protocol fingerprint",
                observation.run_id
            ));
            continue;
        }
        if !planned.contains(&observation.run_id) {
            readiness_failures.push(format!(
                "observation {} is not a planned pilot run",
                observation.run_id
            ));
            continue;
        }
        matched.push(observation);
    }
    if matched.len() != observations.len() {
        readiness_failures.push("some observations were rejected before matching".to_string());
    }
    let mut observed_runs = 0usize;
    let mut treatments = suite
        .treatments
        .iter()
        .map(|treatment| {
            let runs: Vec<&FuguEvaluationObservation> = matched
                .iter()
                .copied()
                .filter(|observation| observation.treatment_id == treatment.id)
                .collect();
            let completed = runs
                .iter()
                .filter(|run| run.completed && run.safety_violations == 0)
                .count();
            let scored = runs
                .iter()
                .filter(|run| run.completed && run.safety_violations == 0)
                .filter_map(|run| run.native_score)
                .collect::<Vec<_>>();
            let score_percent = if scored.is_empty() {
                0.0
            } else {
                scored.iter().sum::<f64>() / scored.len() as f64
            };
            if runs.is_empty() {
                readiness_failures.push(format!(
                    "no observation for planned treatment {}",
                    treatment.id
                ));
            } else if completed != runs.len() {
                readiness_failures.push(format!(
                    "treatment {} has incomplete or unsafe observations ({}/{})",
                    treatment.id,
                    completed,
                    runs.len()
                ));
            }
            observed_runs += runs.len();
            FuguPilotTreatmentResult {
                treatment_id: treatment.id.clone(),
                control: treatment.control,
                observed_runs: runs.len(),
                completed_runs: completed,
                score_percent,
            }
        })
        .collect::<Vec<_>>();
    let ready = readiness_failures.is_empty() && observed_runs == planned.len();
    if !ready && readiness_failures.is_empty() {
        readiness_failures.push(format!(
            "expected {} matched runs, observed {}",
            planned.len(),
            observed_runs
        ));
    }
    treatments.sort_by(|a, b| a.treatment_id.cmp(&b.treatment_id));
    Ok(FuguPilotReport {
        schema: FUGU_PILOT_REPORT_SCHEMA.to_string(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        protocol_fingerprint: fingerprint,
        planned_runs: planned.len(),
        observed_runs,
        ready,
        readiness_failures,
        treatments,
        claim_guard: "harness_link_verification_only".to_string(),
    })
}

pub fn render_fugu_pilot_card(report: &FuguPilotReport) -> String {
    let mut lines = vec![format!(
        "Fugu pilot {} v{}: planned={} observed={} ready={}",
        report.suite_id,
        report.suite_version,
        report.planned_runs,
        report.observed_runs,
        report.ready
    )];
    for treatment in &report.treatments {
        lines.push(format!(
            "  {:<12} control={:<5} runs={}/{} score={:.1}%",
            treatment.treatment_id,
            treatment.control,
            treatment.completed_runs,
            treatment.observed_runs,
            treatment.score_percent
        ));
    }
    lines.push(format!("  claim guard: {}", report.claim_guard));
    for failure in &report.readiness_failures {
        lines.push(format!("  readiness failure: {failure}"));
    }
    lines.join("\n")
}

fn is_git_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Converts a raw external-effect report into one aggregate cell per pilot
/// run, validating the report source binding against the frozen suite case
/// authority and the treatment labels against the protocol harness bindings.
/// Model-call accounting is a best-effort lower bound derived from recorded
/// usage because the report does not persist per-call counts; tool calls are
/// provably zero on the no-tool GPQA path.
pub fn fugu_pilot_cells_from_external_effect_report(
    suite: &FuguPilotSuite,
    protocol: &FuguPilotProtocol,
    report_bytes: &[u8],
) -> Result<Vec<FuguPilotCellAggregate>, Vec<String>> {
    let report: FuguPilotExternalReport = serde_json::from_slice(report_bytes)
        .map_err(|error| vec![format!("invalid external effect report: {error}")])?;
    let mut errors = Vec::new();
    if report.schema != FUGU_PILOT_EXTERNAL_REPORT_SCHEMA {
        errors.push(format!("unexpected report schema {}", report.schema));
    }
    if !is_git_commit(&report.git_commit) {
        errors.push("report git commit is not a full 40-character SHA".to_string());
    }
    let authority = &suite.benchmark.case_authority;
    let source = report
        .sources
        .iter()
        .find(|source| source.benchmark == suite.benchmark.id);
    let Some(source) = source else {
        errors.push(format!(
            "report is missing the {} source binding",
            suite.benchmark.id
        ));
        return Err(errors);
    };
    if source.file_sha256 != authority.file_sha256
        || source.revision != authority.revision
        || source.source_url != authority.source_url
        || source.sample_count != authority.case_count as usize
    {
        errors.push("report case authority drifted from the frozen pilot suite".to_string());
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut bindings = BTreeMap::new();
    for binding in &protocol.harness.treatment_bindings {
        bindings.insert(
            binding.harness_label.as_str(),
            binding.treatment_id.as_str(),
        );
    }

    let planned = suite.planned_run_ids();
    let mut groups: BTreeMap<&str, Vec<&FuguPilotExternalRun>> = BTreeMap::new();
    for run in &report.runs {
        if run.benchmark != suite.benchmark.id {
            continue;
        }
        match bindings.get(run.treatment.as_str()) {
            Some(treatment_id) => groups.entry(treatment_id).or_default().push(run),
            None => errors.push(format!(
                "unexpected pilot harness treatment label {}",
                run.treatment
            )),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let evidence = FuguEvidenceProvenance {
        adapter_version: suite.benchmark.adapter.clone(),
        dataset_version: authority.revision.clone(),
        harness_commit: report.git_commit.clone(),
        raw_result_sha256: sha256_hex(report_bytes),
        synthetic: false,
    };
    let safety = FuguSafetyAttestation {
        sandbox_id: "external_effect_eval_harness".to_string(),
        ephemeral_root: false,
        source_workspace_read_only: true,
        network_mode: FuguNetworkMode::Disabled,
        network_destinations: Vec::new(),
        writes_outside_sandbox: 0,
        destructive_actions: 0,
        privileged_actions: 0,
        symlink_escapes: 0,
        protected_path_accesses: 0,
    };

    let mut cells = Vec::new();
    for (treatment_id, runs) in groups {
        let case_ids = runs
            .iter()
            .map(|run| run.case_id.as_str())
            .collect::<BTreeSet<_>>();
        if runs.len() != authority.case_count as usize
            || case_ids.len() != authority.case_count as usize
        {
            errors.push(format!(
                "treatment {treatment_id} must cover the {} frozen cases exactly once",
                authority.case_count
            ));
            continue;
        }
        let cases_correct = runs
            .iter()
            .filter(|run| run.succeeded && run.exact_score == Some(1.0) && run.error.is_none())
            .count() as u32;
        let failed = runs
            .iter()
            .find(|run| !run.succeeded || run.error.is_some() || run.exact_score.is_none());
        let wall_time_ms = runs.iter().map(|run| run.latency_ms).sum();
        let total_tokens = runs.iter().map(|run| run.total_tokens).sum();
        let model_calls = runs.iter().filter(|run| run.total_tokens > 0).count() as u64;
        let run_id = suite.run_id(treatment_id, 0);
        if !planned.contains(&run_id) {
            errors.push(format!(
                "treatment {treatment_id} is not a planned pilot run"
            ));
            continue;
        }
        cells.push(FuguPilotCellAggregate {
            run_id,
            treatment_id: treatment_id.to_string(),
            seed: 0,
            cases_total: authority.case_count,
            cases_correct,
            completed: failed.is_none(),
            wall_time_ms,
            model_calls,
            tool_calls: 0,
            total_tokens,
            recoveries: 0,
            safety_violations: 0,
            failure_reason: failed.and_then(|run| {
                run.error.clone().or_else(|| {
                    Some(format!(
                        "case {} did not yield a scored answer",
                        run.case_id
                    ))
                })
            }),
            evidence: evidence.clone(),
            safety: safety.clone(),
        });
    }
    if errors.is_empty() && cells.len() != suite.treatments.len() {
        errors.push(format!(
            "expected {} pilot treatment cells, found {}",
            suite.treatments.len(),
            cells.len()
        ));
    }
    if errors.is_empty() {
        Ok(cells)
    } else {
        Err(errors)
    }
}

/// Deterministic self-test fixture: a raw external-effect report over the
/// frozen 12-case sample with the historical matched-diagnostic outcome shape
/// (cindx_fast 10/12, cindx_auto 8/12, cindx_pro 9/12).
pub fn fugu_pilot_synthetic_external_report_json() -> Vec<u8> {
    use serde_json::json;
    let mut runs = Vec::new();
    for case_index in 0..12u32 {
        let case_id = format!("gpqa:case-{case_index:02}");
        for (treatment, position_offset, correct) in [
            ("direct_default", 0usize, case_index < 10),
            ("cindx_auto", 1usize, case_index < 8),
            ("cindx_pro", 2usize, case_index < 9),
        ] {
            runs.push(json!({
                "benchmark": "gpqa_diamond",
                "case_id": case_id,
                "category": "Physics",
                "treatment": treatment,
                "treatment_position": (case_index as usize + position_offset) % 3,
                "requested_policy": treatment,
                "effective_policy": treatment,
                "models": ["configured-model"],
                "prompt_profile": "frozen-profile",
                "prompt_profile_origin": "seeded",
                "prompt_profile_sha256": "ab".repeat(32),
                "prompt_profile_artifact_sha256": null,
                "gepa_frozen": true,
                "succeeded": true,
                "latency_ms": 1000u64,
                "prompt_tokens": 500u64,
                "completion_tokens": 100u64,
                "total_tokens": 600u64,
                "first_token_latency_ms": 200u64,
                "input_sha256": "cd".repeat(32),
                "expected_sha256": "ef".repeat(32),
                "output_sha256": "01".repeat(32),
                "parsed_answer": if correct { "A" } else { "B" },
                "exact_score": if correct { 1.0 } else { 0.0 },
                "prefix_valid": null,
                "n_chars": null,
                "n_needles": null,
                "total_messages": null,
                "expected": "A",
                "output": format!("The correct answer is ({})", if correct { "A" } else { "B" }),
                "error": null,
                "diagnostics": {}
            }));
        }
    }
    let report = json!({
        "schema": FUGU_PILOT_EXTERNAL_REPORT_SCHEMA,
        "generated_at_ms": 0u64,
        "git_commit": "0".repeat(40),
        "app_version": "selftest",
        "provider_endpoint": "https://provider.internal/v1",
        "configured_models": {},
        "evaluation_limits": {},
        "sources": [{
            "benchmark": "gpqa_diamond",
            "source_url": "https://github.com/idavidrein/gpqa",
            "revision": "56686c06f5e19865c153de0fdb11be3890014df7",
            "file_sha256": "41d1213cd7a4998605a26c2798500652572007161b3a92817ba46b35befcd305",
            "sample_count": 12u32,
            "protocol": "frozen pilot selftest"
        }],
        "runs": runs
    });
    serde_json::to_vec(&report).expect("synthetic pilot report should serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fugu_evaluation::FuguNetworkMode;

    const PILOT_SUITE: &str = include_str!("../../../benchmarks/fugu/fugu-pilot-v1.json");
    const PILOT_PROTOCOL: &str =
        include_str!("../../../benchmarks/fugu/fugu-pilot-protocol-v1.json");

    fn suite() -> FuguPilotSuite {
        parse_fugu_pilot_suite(PILOT_SUITE).expect("tracked pilot suite should parse")
    }

    fn protocol() -> FuguPilotProtocol {
        parse_fugu_pilot_protocol(PILOT_PROTOCOL).expect("tracked pilot protocol should parse")
    }

    fn evidence() -> FuguEvidenceProvenance {
        FuguEvidenceProvenance {
            adapter_version: "evalscope_gpqa_diamond".to_string(),
            dataset_version: "56686c06f5e19865c153de0fdb11be3890014df7".to_string(),
            harness_commit: "0".repeat(40),
            raw_result_sha256: "a".repeat(64),
            synthetic: false,
        }
    }

    fn safety() -> FuguSafetyAttestation {
        FuguSafetyAttestation {
            sandbox_id: "pilot-sandbox".to_string(),
            ephemeral_root: true,
            source_workspace_read_only: true,
            network_mode: FuguNetworkMode::Disabled,
            network_destinations: Vec::new(),
            writes_outside_sandbox: 0,
            destructive_actions: 0,
            privileged_actions: 0,
            symlink_escapes: 0,
            protected_path_accesses: 0,
        }
    }

    fn cell(treatment_id: &str, correct: u32) -> FuguPilotCellAggregate {
        let pilot = suite();
        FuguPilotCellAggregate {
            run_id: pilot.run_id(treatment_id, 0),
            treatment_id: treatment_id.to_string(),
            seed: 0,
            cases_total: 12,
            cases_correct: correct,
            completed: true,
            wall_time_ms: 60_000,
            model_calls: 12,
            tool_calls: 0,
            total_tokens: 10_000,
            recoveries: 0,
            safety_violations: 0,
            failure_reason: None,
            evidence: evidence(),
            safety: safety(),
        }
    }

    #[test]
    fn fugu_pilot_suite_is_frozen_and_valid() {
        let pilot = suite();
        pilot
            .validate()
            .expect("tracked pilot suite should validate");
        assert_eq!(pilot.schema, FUGU_PILOT_SUITE_SCHEMA);
        assert_eq!(pilot.benchmark.case_authority.case_count, 12);
        assert_eq!(
            pilot.planned_run_ids(),
            vec![
                "fugu_pilot_quality_first:gpqa_diamond:cindx_fast:0",
                "fugu_pilot_quality_first:gpqa_diamond:cindx_auto:0",
                "fugu_pilot_quality_first:gpqa_diamond:cindx_pro:0",
            ]
        );
    }

    #[test]
    fn fugu_pilot_suite_rejects_drift() {
        let mut pilot = suite();
        pilot.track.seeds = vec![0, 1];
        assert!(pilot.validate().is_err());
        let mut pilot = suite();
        pilot.treatments[0].control = false;
        assert!(pilot.validate().is_err());
        let mut pilot = suite();
        pilot.benchmark.id = "mrcr_v2".to_string();
        assert!(pilot.validate().is_err());
        println!("{FUGU_PILOT_SUITE_SCHEMA}");
    }

    #[test]
    fn fugu_pilot_protocol_binds_the_suite_digest_and_stays_unauthorized() {
        let pilot = suite();
        let manifest = protocol();
        manifest
            .validate(PILOT_SUITE.as_bytes(), &pilot)
            .expect("tracked pilot protocol should bind the tracked suite");
        assert!(!manifest.execution_authorized);

        let mut drifted = manifest.clone();
        drifted.execution_authorized = true;
        assert!(drifted.validate(PILOT_SUITE.as_bytes(), &pilot).is_err());

        let mut drifted = manifest.clone();
        drifted.suite.sha256 = "b".repeat(64);
        assert!(drifted.validate(PILOT_SUITE.as_bytes(), &pilot).is_err());

        let mut drifted = manifest.clone();
        drifted.run_order.pop();
        assert!(drifted.validate(PILOT_SUITE.as_bytes(), &pilot).is_err());

        let mut drifted_suite = pilot.clone();
        drifted_suite.benchmark.case_authority.case_count = 24;
        assert!(manifest
            .validate(PILOT_SUITE.as_bytes(), &drifted_suite)
            .is_err());
        println!("{FUGU_PILOT_PROTOCOL_SCHEMA}");
    }

    #[test]
    fn fugu_pilot_run_plan_has_exactly_three_planned_runs() {
        let pilot = suite();
        let plan = build_fugu_pilot_run_plan(&pilot).expect("pilot plan should build");
        assert!(!plan.execution_enabled);
        assert_eq!(plan.runs.len(), 3);
        assert_eq!(plan.protocol_fingerprint, pilot.fingerprint());
        assert!(plan.runs.iter().all(|run| run.status == "planned"));
    }

    #[test]
    fn fugu_pilot_projection_maps_cells_to_observations() {
        let pilot = suite();
        let cells = vec![
            cell("cindx_fast", 10),
            cell("cindx_auto", 8),
            cell("cindx_pro", 9),
        ];
        let observations =
            project_fugu_pilot_cells(&pilot, &cells).expect("valid cells should project");
        assert_eq!(observations.len(), 3);
        let fast = observations
            .iter()
            .find(|observation| observation.treatment_id == "cindx_fast")
            .expect("fast observation");
        assert_eq!(fast.native_score, Some(10.0 / 12.0 * 100.0));
        assert!(observations
            .iter()
            .all(|observation| !observation.protocol_equivalent_to_fugu_v1));
        assert!(observations
            .iter()
            .all(|observation| observation.protocol_fingerprint == pilot.fingerprint()));
    }

    #[test]
    fn fugu_pilot_projection_fails_closed_on_partial_or_unsafe_cells() {
        let pilot = suite();
        let mut cells = vec![
            cell("cindx_fast", 10),
            cell("cindx_auto", 8),
            cell("cindx_pro", 9),
        ];
        assert!(project_fugu_pilot_cells(&pilot, &cells[..2]).is_err());

        cells.push(cell("cindx_fast", 10));
        assert!(project_fugu_pilot_cells(&pilot, &cells).is_err());

        let mut violating = vec![
            cell("cindx_fast", 10),
            cell("cindx_auto", 8),
            cell("cindx_pro", 9),
        ];
        violating[1].safety_violations = 1;
        assert!(project_fugu_pilot_cells(&pilot, &violating).is_err());

        let mut wrong_cases = vec![
            cell("cindx_fast", 10),
            cell("cindx_auto", 8),
            cell("cindx_pro", 9),
        ];
        wrong_cases[2].cases_total = 11;
        assert!(project_fugu_pilot_cells(&pilot, &wrong_cases).is_err());
    }

    #[test]
    fn fugu_pilot_evaluation_reports_readiness_and_guards_claims() {
        let pilot = suite();
        let cells = vec![
            cell("cindx_fast", 10),
            cell("cindx_auto", 8),
            cell("cindx_pro", 9),
        ];
        let observations = project_fugu_pilot_cells(&pilot, &cells).expect("projection");
        let report = evaluate_fugu_pilot(&pilot, &observations).expect("evaluation");
        assert!(report.ready);
        assert_eq!(report.planned_runs, 3);
        assert_eq!(report.observed_runs, 3);
        assert_eq!(report.claim_guard, "harness_link_verification_only");
        let fast = report
            .treatments
            .iter()
            .find(|treatment| treatment.treatment_id == "cindx_fast")
            .expect("fast result");
        assert!(fast.control);
        assert!((fast.score_percent - 10.0 / 12.0 * 100.0).abs() < 1e-9);
        assert!(render_fugu_pilot_card(&report).contains("claim guard"));
    }

    #[test]
    fn fugu_pilot_evaluation_fails_closed_on_missing_or_foreign_observations() {
        let pilot = suite();
        let cells = vec![
            cell("cindx_fast", 10),
            cell("cindx_auto", 8),
            cell("cindx_pro", 9),
        ];
        let mut observations = project_fugu_pilot_cells(&pilot, &cells).expect("projection");
        observations.pop();
        let report = evaluate_fugu_pilot(&pilot, &observations).expect("evaluation");
        assert!(!report.ready);
        assert!(!report.readiness_failures.is_empty());

        let mut foreign = project_fugu_pilot_cells(&pilot, &cells).expect("projection");
        foreign[0].protocol_fingerprint = "c".repeat(64);
        let report = evaluate_fugu_pilot(&pilot, &foreign).expect("evaluation");
        assert!(!report.ready);
    }

    #[test]
    fn fugu_pilot_external_report_projects_to_a_ready_pilot() {
        let pilot = suite();
        let manifest = protocol();
        let report_bytes = fugu_pilot_synthetic_external_report_json();
        let cells = fugu_pilot_cells_from_external_effect_report(&pilot, &manifest, &report_bytes)
            .expect("synthetic report should convert");
        assert_eq!(cells.len(), 3);
        let fast = cells
            .iter()
            .find(|cell| cell.treatment_id == "cindx_fast")
            .expect("fast cell");
        assert_eq!(fast.cases_correct, 10);
        assert_eq!(fast.model_calls, 12);
        assert_eq!(fast.tool_calls, 0);
        assert!(fast.completed);
        let observations = project_fugu_pilot_cells(&pilot, &cells).expect("projection");
        let report = evaluate_fugu_pilot(&pilot, &observations).expect("evaluation");
        assert!(report.ready);
        assert!(
            (report
                .treatments
                .iter()
                .find(|t| t.treatment_id == "cindx_auto")
                .unwrap()
                .score_percent
                - 8.0 / 12.0 * 100.0)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn fugu_pilot_external_report_fails_closed_on_drift() {
        let pilot = suite();
        let manifest = protocol();
        let report_bytes = fugu_pilot_synthetic_external_report_json();

        let mut value: serde_json::Value =
            serde_json::from_slice(&report_bytes).expect("report value");
        value["runs"].as_array_mut().expect("runs").pop();
        let drifted = serde_json::to_vec(&value).expect("encode");
        assert!(fugu_pilot_cells_from_external_effect_report(&pilot, &manifest, &drifted).is_err());

        let mut value: serde_json::Value =
            serde_json::from_slice(&report_bytes).expect("report value");
        value["sources"][0]["file_sha256"] = serde_json::json!("0".repeat(64));
        let drifted = serde_json::to_vec(&value).expect("encode");
        assert!(fugu_pilot_cells_from_external_effect_report(&pilot, &manifest, &drifted).is_err());

        let mut value: serde_json::Value =
            serde_json::from_slice(&report_bytes).expect("report value");
        value["runs"][0]["treatment"] = serde_json::json!("unknown_arm");
        let drifted = serde_json::to_vec(&value).expect("encode");
        assert!(fugu_pilot_cells_from_external_effect_report(&pilot, &manifest, &drifted).is_err());

        let mut value: serde_json::Value =
            serde_json::from_slice(&report_bytes).expect("report value");
        value["git_commit"] = serde_json::json!("e9beb4a");
        let drifted = serde_json::to_vec(&value).expect("encode");
        assert!(fugu_pilot_cells_from_external_effect_report(&pilot, &manifest, &drifted).is_err());
    }
}
