use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

pub const AGENT_EVALUATION_BASELINE_SCHEMA: &str = "cindx.agent-evaluation-baseline.v2";
pub const AGENT_EVALUATION_DATASET_SCHEMA: &str = "cindx.agent-evaluation-dataset.v2";
pub const AGENT_EVALUATION_TRACE_SCHEMA: &str = "cindx.agent-evaluation-trace.v2";
pub const AGENT_EVALUATION_SCORE_SET_SCHEMA: &str = "cindx.agent-evaluation-score-set.v2";
pub const AGENT_EVALUATION_FOUNDATION_REPORT_SCHEMA: &str =
    "cindx.agent-evaluation-foundation-report.v2";
pub const AGENT_EVALUATION_PROMOTION_REPORT_SCHEMA: &str =
    "cindx.agent-evaluation-promotion-report.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvaluationSplit {
    Feedback,
    Pareto,
    Test,
}

impl AgentEvaluationSplit {
    pub const ALL: [Self; 3] = [Self::Feedback, Self::Pareto, Self::Test];

    pub fn label(self) -> &'static str {
        match self {
            Self::Feedback => "feedback",
            Self::Pareto => "pareto",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationBaseline {
    pub schema: String,
    pub id: String,
    pub source_commit: String,
    pub captured_at: String,
    pub evaluation_suite_id: String,
    pub evaluation_suite_version: u32,
    pub routing_contract: FrozenRoutingContract,
    pub quality_evidence: BaselineQualityEvidence,
    pub promotion_gate: AgentEvaluationPromotionGate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrozenRoutingContract {
    pub suite_id: String,
    pub suite_version: u32,
    pub cases: usize,
    pub required_pass_rate: f64,
    pub suite_sha256: String,
    pub baseline_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineQualityStatus {
    Unmeasured,
    Measured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineQualityEvidence {
    pub status: BaselineQualityStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationPromotionGate {
    pub minimum_feedback_cases: usize,
    pub minimum_pareto_cases: usize,
    pub minimum_hidden_test_cases: usize,
    pub minimum_repeats_per_case: usize,
    pub minimum_absolute_success_gain: f64,
    pub minimum_pairwise_wilson_lower_bound: f64,
    pub maximum_category_regression: f64,
    pub maximum_critical_safety_violations: u64,
}

impl AgentEvaluationBaseline {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema != AGENT_EVALUATION_BASELINE_SCHEMA {
            errors.push(format!(
                "unsupported evaluation baseline schema {}",
                self.schema
            ));
        }
        if self.id.trim().is_empty() {
            errors.push("evaluation baseline id is empty".to_string());
        }
        if !is_lower_hex(&self.source_commit, 40) {
            errors.push("evaluation baseline source commit must be a full SHA-1".to_string());
        }
        if self.captured_at.trim().is_empty() {
            errors.push("evaluation baseline capture time is empty".to_string());
        }
        if self.evaluation_suite_id.trim().is_empty() || self.evaluation_suite_version == 0 {
            errors.push("evaluation suite identity is invalid".to_string());
        }
        if self.routing_contract.suite_id.trim().is_empty()
            || self.routing_contract.suite_version == 0
            || self.routing_contract.cases == 0
        {
            errors.push("frozen routing contract identity is invalid".to_string());
        }
        if !(0.0..=1.0).contains(&self.routing_contract.required_pass_rate) {
            errors.push("frozen routing pass rate must be between 0 and 1".to_string());
        }
        if !is_lower_hex(&self.routing_contract.suite_sha256, 64)
            || !is_lower_hex(&self.routing_contract.baseline_sha256, 64)
        {
            errors.push("frozen routing hashes must be SHA-256 values".to_string());
        }
        if self.quality_evidence.reason.trim().is_empty() {
            errors.push("baseline quality evidence must explain its status".to_string());
        }
        errors.extend(self.promotion_gate.validation_errors());
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl AgentEvaluationPromotionGate {
    fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.minimum_feedback_cases == 0
            || self.minimum_pareto_cases == 0
            || self.minimum_hidden_test_cases == 0
            || self.minimum_repeats_per_case == 0
        {
            errors
                .push("evaluation promotion case and repeat minimums must be positive".to_string());
        }
        if !(0.0..=1.0).contains(&self.minimum_absolute_success_gain) {
            errors.push("minimum success gain must be between 0 and 1".to_string());
        }
        if !(0.5..=1.0).contains(&self.minimum_pairwise_wilson_lower_bound) {
            errors.push("pairwise Wilson lower bound must be between 0.5 and 1".to_string());
        }
        if !(0.0..=1.0).contains(&self.maximum_category_regression) {
            errors.push("maximum category regression must be between 0 and 1".to_string());
        }
        errors
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationDataset {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub split: AgentEvaluationSplit,
    pub description: String,
    pub cases: Vec<AgentEvaluationCase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationCase {
    pub id: String,
    pub category: String,
    pub objective: String,
    pub verifier: AgentEvaluationVerifier,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl AgentEvaluationDataset {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema != AGENT_EVALUATION_DATASET_SCHEMA {
            errors.push(format!(
                "unsupported evaluation dataset schema {}",
                self.schema
            ));
        }
        if self.suite_id.trim().is_empty() || self.suite_version == 0 {
            errors.push("evaluation dataset identity is invalid".to_string());
        }
        if self.description.trim().is_empty() {
            errors.push("evaluation dataset description is empty".to_string());
        }
        if self.cases.is_empty() {
            errors.push(format!("{} dataset has no cases", self.split.label()));
        }
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            if case.id.trim().is_empty() || !ids.insert(case.id.as_str()) {
                errors.push(format!(
                    "{} dataset case id is empty or duplicated: {}",
                    self.split.label(),
                    case.id
                ));
            }
            if case.category.trim().is_empty() {
                errors.push(format!("{} has an empty category", case.id));
            }
            if case.objective.trim().is_empty() {
                errors.push(format!("{} has an empty objective", case.id));
            }
            if let Err(error) = case.verifier.validate() {
                errors.push(format!("{}: {error}", case.id));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvaluationVerifier {
    ExactText {
        expected: String,
        #[serde(default)]
        case_sensitive: bool,
    },
    ContainsAll {
        expected: Vec<String>,
        #[serde(default)]
        case_sensitive: bool,
    },
    JsonFields {
        required_fields: Vec<String>,
    },
}

impl AgentEvaluationVerifier {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::ExactText { expected, .. } if expected.trim().is_empty() => {
                Err("exact-text verifier has an empty expectation".to_string())
            }
            Self::ContainsAll { expected, .. }
                if expected.is_empty() || expected.iter().any(|value| value.trim().is_empty()) =>
            {
                Err("contains-all verifier requires non-empty expectations".to_string())
            }
            Self::JsonFields { required_fields }
                if required_fields.is_empty()
                    || required_fields.iter().any(|field| field.trim().is_empty()) =>
            {
                Err("JSON-fields verifier requires non-empty field names".to_string())
            }
            _ => Ok(()),
        }
    }

    pub fn verify(&self, output: &str) -> AgentEvaluationVerifierOutcome {
        match self {
            Self::ExactText {
                expected,
                case_sensitive,
            } => {
                let passed =
                    comparable(output, *case_sensitive) == comparable(expected, *case_sensitive);
                AgentEvaluationVerifierOutcome::from_checks(vec![AgentEvaluationCheck {
                    id: "exact_text".to_string(),
                    passed,
                    detail: if passed {
                        "output matched the expected text".to_string()
                    } else {
                        format!("expected exact text: {}", expected.trim())
                    },
                }])
            }
            Self::ContainsAll {
                expected,
                case_sensitive,
            } => {
                let output = comparable(output, *case_sensitive);
                let checks = expected
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        let passed = output.contains(&comparable(value, *case_sensitive));
                        AgentEvaluationCheck {
                            id: format!("contains_{}", index + 1),
                            passed,
                            detail: if passed {
                                format!("output contained: {}", value.trim())
                            } else {
                                format!("output is missing: {}", value.trim())
                            },
                        }
                    })
                    .collect();
                AgentEvaluationVerifierOutcome::from_checks(checks)
            }
            Self::JsonFields { required_fields } => {
                let parsed = serde_json::from_str::<Value>(output.trim()).ok();
                let checks = required_fields
                    .iter()
                    .map(|field| {
                        let passed = parsed
                            .as_ref()
                            .and_then(Value::as_object)
                            .is_some_and(|object| object.contains_key(field));
                        AgentEvaluationCheck {
                            id: format!("json_field_{field}"),
                            passed,
                            detail: if passed {
                                format!("JSON field is present: {field}")
                            } else {
                                format!("JSON field is missing: {field}")
                            },
                        }
                    })
                    .collect();
                AgentEvaluationVerifierOutcome::from_checks(checks)
            }
        }
    }
}

fn comparable(value: &str, case_sensitive: bool) -> String {
    let value = value.trim();
    if case_sensitive {
        value.to_string()
    } else {
        value.to_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationVerifierOutcome {
    #[serde(default)]
    pub source: AgentEvaluationEvidenceSource,
    pub passed: bool,
    pub score: f64,
    pub checks: Vec<AgentEvaluationCheck>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvaluationEvidenceSource {
    #[default]
    Deterministic,
    Judge,
}

impl AgentEvaluationVerifierOutcome {
    fn from_checks(checks: Vec<AgentEvaluationCheck>) -> Self {
        let passed_checks = checks.iter().filter(|check| check.passed).count();
        let score = passed_checks as f64 / checks.len().max(1) as f64;
        Self {
            source: AgentEvaluationEvidenceSource::Deterministic,
            passed: !checks.is_empty() && passed_checks == checks.len(),
            score,
            checks,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !self.score.is_finite() || !(0.0..=1.0).contains(&self.score) {
            return Err("verifier score must be finite and between 0 and 1".to_string());
        }
        if self.checks.is_empty() {
            return Err("verifier outcome has no checks".to_string());
        }
        let all_passed = self.checks.iter().all(|check| check.passed);
        if self.passed != all_passed {
            return Err("verifier pass flag disagrees with its checks".to_string());
        }
        Ok(())
    }

    pub fn actionable_side_information(&self) -> ActionableSideInformation {
        let passed_constraints = self
            .checks
            .iter()
            .filter(|check| check.passed)
            .map(|check| check.detail.clone())
            .collect::<Vec<_>>();
        let failed_constraints = self
            .checks
            .iter()
            .filter(|check| !check.passed)
            .map(|check| check.detail.clone())
            .collect::<Vec<_>>();
        ActionableSideInformation {
            summary: format!(
                "{} of {} deterministic checks passed",
                passed_constraints.len(),
                self.checks.len()
            ),
            passed_constraints,
            failed_constraints,
            errors: Vec::new(),
            suggested_changes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvaluationCheck {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionableSideInformation {
    pub summary: String,
    pub passed_constraints: Vec<String>,
    pub failed_constraints: Vec<String>,
    pub errors: Vec<String>,
    pub suggested_changes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationTrace {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub case_id: String,
    pub category: String,
    pub split: AgentEvaluationSplit,
    pub run_id: String,
    pub seed: u64,
    pub candidate_id: String,
    pub candidate_fingerprint: String,
    pub model_fingerprints: BTreeMap<String, String>,
    pub input: String,
    pub steps: Vec<AgentEvaluationTraceStep>,
    pub final_output: String,
    pub verifier: AgentEvaluationVerifierOutcome,
    pub actionable_feedback: ActionableSideInformation,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub safety_violations: u64,
    pub redaction_applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvaluationTraceStep {
    pub step_id: String,
    pub role: String,
    pub model: String,
    pub prompt: String,
    pub output: String,
    pub tool_calls: Vec<AgentEvaluationToolTrace>,
    pub errors: Vec<String>,
    pub latency_ms: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvaluationToolTrace {
    pub tool: String,
    pub request: String,
    pub response: String,
    pub error: Option<String>,
}

impl AgentEvaluationTrace {
    pub fn apply_redaction(&mut self, secrets: &[String]) {
        let mut secrets = secrets
            .iter()
            .map(|secret| secret.trim())
            .filter(|secret| !secret.is_empty())
            .collect::<Vec<_>>();
        secrets.sort_by_key(|secret| Reverse(secret.len()));
        secrets.dedup();

        redact_text(&mut self.input, &secrets);
        redact_text(&mut self.final_output, &secrets);
        redact_side_information(&mut self.actionable_feedback, &secrets);
        for check in &mut self.verifier.checks {
            redact_text(&mut check.detail, &secrets);
        }
        for step in &mut self.steps {
            redact_text(&mut step.prompt, &secrets);
            redact_text(&mut step.output, &secrets);
            for error in &mut step.errors {
                redact_text(error, &secrets);
            }
            for call in &mut step.tool_calls {
                redact_text(&mut call.request, &secrets);
                redact_text(&mut call.response, &secrets);
                if let Some(error) = &mut call.error {
                    redact_text(error, &secrets);
                }
            }
        }
        self.redaction_applied = true;
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != AGENT_EVALUATION_TRACE_SCHEMA {
            return Err(format!(
                "unsupported evaluation trace schema {}",
                self.schema
            ));
        }
        if self.suite_id.trim().is_empty()
            || self.suite_version == 0
            || self.case_id.trim().is_empty()
            || self.category.trim().is_empty()
            || self.run_id.trim().is_empty()
            || self.candidate_id.trim().is_empty()
            || self.candidate_fingerprint.trim().is_empty()
        {
            return Err("evaluation trace identity is incomplete".to_string());
        }
        self.verifier.validate()?;
        Ok(())
    }

    pub fn reflection_packet(&self) -> Result<AgentEvaluationReflectionPacket, String> {
        self.validate()?;
        if self.split != AgentEvaluationSplit::Feedback {
            return Err(format!(
                "{} traces cannot enter reflection",
                self.split.label()
            ));
        }
        if !self.redaction_applied {
            return Err("unredacted traces cannot enter reflection".to_string());
        }
        Ok(AgentEvaluationReflectionPacket {
            suite_id: self.suite_id.clone(),
            suite_version: self.suite_version,
            case_id: self.case_id.clone(),
            category: self.category.clone(),
            run_id: self.run_id.clone(),
            seed: self.seed,
            candidate_id: self.candidate_id.clone(),
            candidate_fingerprint: self.candidate_fingerprint.clone(),
            model_fingerprints: self.model_fingerprints.clone(),
            input: self.input.clone(),
            steps: self.steps.clone(),
            final_output: self.final_output.clone(),
            verifier: self.verifier.clone(),
            actionable_feedback: self.actionable_feedback.clone(),
        })
    }

    pub fn pareto_record(&self) -> Result<AgentEvaluationCaseScore, String> {
        self.record_for(AgentEvaluationSplit::Pareto)
    }

    pub fn hidden_test_record(&self) -> Result<AgentEvaluationCaseScore, String> {
        self.record_for(AgentEvaluationSplit::Test)
    }

    fn record_for(
        &self,
        required_split: AgentEvaluationSplit,
    ) -> Result<AgentEvaluationCaseScore, String> {
        self.validate()?;
        if self.split != required_split {
            return Err(format!(
                "{} trace cannot produce a {} record",
                self.split.label(),
                required_split.label()
            ));
        }
        let score = if self.safety_violations == 0 {
            self.verifier.score
        } else {
            0.0
        };
        Ok(AgentEvaluationCaseScore {
            suite_id: self.suite_id.clone(),
            suite_version: self.suite_version,
            case_id: self.case_id.clone(),
            category: self.category.clone(),
            split: self.split,
            run_id: self.run_id.clone(),
            seed: self.seed,
            candidate_id: self.candidate_id.clone(),
            candidate_fingerprint: self.candidate_fingerprint.clone(),
            evidence_source: self.verifier.source,
            score,
            verified_success: self.verifier.passed && self.safety_violations == 0,
            latency_ms: self.latency_ms,
            total_tokens: self.total_tokens,
            safety_violations: self.safety_violations,
        })
    }
}

fn redact_side_information(value: &mut ActionableSideInformation, secrets: &[&str]) {
    redact_text(&mut value.summary, secrets);
    for entry in value
        .passed_constraints
        .iter_mut()
        .chain(value.failed_constraints.iter_mut())
        .chain(value.errors.iter_mut())
        .chain(value.suggested_changes.iter_mut())
    {
        redact_text(entry, secrets);
    }
}

fn redact_text(value: &mut String, secrets: &[&str]) {
    for secret in secrets {
        if value.contains(secret) {
            *value = value.replace(secret, "[REDACTED]");
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationReflectionPacket {
    pub suite_id: String,
    pub suite_version: u32,
    pub case_id: String,
    pub category: String,
    pub run_id: String,
    pub seed: u64,
    pub candidate_id: String,
    pub candidate_fingerprint: String,
    pub model_fingerprints: BTreeMap<String, String>,
    pub input: String,
    pub steps: Vec<AgentEvaluationTraceStep>,
    pub final_output: String,
    pub verifier: AgentEvaluationVerifierOutcome,
    pub actionable_feedback: ActionableSideInformation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationCaseScore {
    pub suite_id: String,
    pub suite_version: u32,
    pub case_id: String,
    pub category: String,
    pub split: AgentEvaluationSplit,
    pub run_id: String,
    pub seed: u64,
    pub candidate_id: String,
    pub candidate_fingerprint: String,
    #[serde(default)]
    pub evidence_source: AgentEvaluationEvidenceSource,
    pub score: f64,
    pub verified_success: bool,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub safety_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationScoreSet {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub split: AgentEvaluationSplit,
    pub candidate_id: String,
    pub candidate_fingerprint: String,
    pub dataset_sha256: String,
    #[serde(default)]
    pub provenance: AgentEvaluationRunProvenance,
    pub records: Vec<AgentEvaluationCaseScore>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvaluationRunProvenance {
    ProviderBacked,
    #[default]
    SyntheticGateSmoke,
}

impl AgentEvaluationScoreSet {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema != AGENT_EVALUATION_SCORE_SET_SCHEMA {
            errors.push(format!("unsupported score-set schema {}", self.schema));
        }
        if self.suite_id.trim().is_empty()
            || self.suite_version == 0
            || self.candidate_id.trim().is_empty()
            || self.candidate_fingerprint.trim().is_empty()
        {
            errors.push("score-set identity is incomplete".to_string());
        }
        if !is_lower_hex(&self.dataset_sha256, 64) {
            errors.push("score set dataset hash must be SHA-256".to_string());
        }
        if self.records.is_empty() {
            errors.push("score set has no records".to_string());
        }
        let mut keys = BTreeSet::new();
        for record in &self.records {
            if record.suite_id != self.suite_id
                || record.suite_version != self.suite_version
                || record.split != self.split
                || record.candidate_id != self.candidate_id
                || record.candidate_fingerprint != self.candidate_fingerprint
            {
                errors.push(format!(
                    "score record {} does not match its score-set identity",
                    record.run_id
                ));
            }
            if record.case_id.trim().is_empty()
                || record.category.trim().is_empty()
                || record.run_id.trim().is_empty()
            {
                errors.push("score record identity is incomplete".to_string());
            }
            if !record.score.is_finite() || !(0.0..=1.0).contains(&record.score) {
                errors.push(format!("score record {} is outside 0..1", record.run_id));
            }
            if !keys.insert((record.case_id.as_str(), record.seed)) {
                errors.push(format!(
                    "score set duplicates case {} seed {}",
                    record.case_id, record.seed
                ));
            }
            if self.split == AgentEvaluationSplit::Test
                && record.evidence_source != AgentEvaluationEvidenceSource::Deterministic
            {
                errors.push(format!(
                    "hidden test record {} is not deterministically verified",
                    record.run_id
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationCandidateMetrics {
    pub candidate_id: String,
    pub cases: usize,
    pub runs: usize,
    pub verified_success_rate: f64,
    pub average_score: f64,
    pub safety_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationCategoryDelta {
    pub category: String,
    pub baseline_success_rate: f64,
    pub candidate_success_rate: f64,
    pub regression: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvaluationPromotionReport {
    pub schema: String,
    pub baseline_id: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub split: AgentEvaluationSplit,
    pub baseline: AgentEvaluationCandidateMetrics,
    pub candidate: AgentEvaluationCandidateMetrics,
    pub paired_comparisons: usize,
    pub candidate_wins: usize,
    pub ties: usize,
    pub candidate_losses: usize,
    pub pairwise_score: f64,
    pub pairwise_wilson_lower_bound: f64,
    pub absolute_success_gain: f64,
    pub average_score_gain: f64,
    pub maximum_category_regression: f64,
    pub category_deltas: Vec<AgentEvaluationCategoryDelta>,
    pub gates: BTreeMap<String, bool>,
    pub promotion_eligible: bool,
    pub recommended_canary_percent: Option<u8>,
}

pub fn parse_agent_evaluation_score_set(source: &str) -> Result<AgentEvaluationScoreSet, String> {
    serde_json::from_str(source)
        .map_err(|error| format!("invalid evaluation score-set JSON: {error}"))
}

pub fn build_agent_evaluation_promotion_report(
    baseline: &AgentEvaluationBaseline,
    baseline_scores: &AgentEvaluationScoreSet,
    candidate_scores: &AgentEvaluationScoreSet,
) -> Result<AgentEvaluationPromotionReport, Vec<String>> {
    let mut errors = baseline.validate().err().unwrap_or_default();
    errors.extend(baseline_scores.validate().err().unwrap_or_default());
    errors.extend(candidate_scores.validate().err().unwrap_or_default());
    if baseline_scores.split != AgentEvaluationSplit::Test
        || candidate_scores.split != AgentEvaluationSplit::Test
    {
        errors.push("canary promotion requires hidden test score sets".to_string());
    }
    if baseline_scores.suite_id != baseline.evaluation_suite_id
        || baseline_scores.suite_version != baseline.evaluation_suite_version
        || candidate_scores.suite_id != baseline.evaluation_suite_id
        || candidate_scores.suite_version != baseline.evaluation_suite_version
    {
        errors.push("promotion score sets target a different evaluation suite".to_string());
    }
    if baseline_scores.candidate_id == candidate_scores.candidate_id {
        errors.push("promotion candidate must differ from the frozen baseline".to_string());
    }
    if baseline_scores.dataset_sha256 != candidate_scores.dataset_sha256 {
        errors.push("baseline and candidate score sets target different hidden datasets".to_string());
    }
    let baseline_by_key = score_records_by_key(baseline_scores, &mut errors);
    let candidate_by_key = score_records_by_key(candidate_scores, &mut errors);
    let baseline_keys = baseline_by_key.keys().cloned().collect::<BTreeSet<_>>();
    let candidate_keys = candidate_by_key.keys().cloned().collect::<BTreeSet<_>>();
    if baseline_keys != candidate_keys {
        errors
            .push("baseline and candidate score sets are not paired by case and seed".to_string());
    }
    for key in baseline_keys.intersection(&candidate_keys) {
        let baseline_record = baseline_by_key[key];
        let candidate_record = candidate_by_key[key];
        if baseline_record.category != candidate_record.category {
            errors.push(format!(
                "paired score records disagree on category for {} seed {}",
                key.0, key.1
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let case_ids = baseline_scores
        .records
        .iter()
        .map(|record| record.case_id.as_str())
        .collect::<BTreeSet<_>>();
    let repeats_satisfied = case_ids.iter().all(|case_id| {
        baseline_scores
            .records
            .iter()
            .filter(|record| record.case_id == *case_id)
            .count()
            >= baseline.promotion_gate.minimum_repeats_per_case
            && candidate_scores
                .records
                .iter()
                .filter(|record| record.case_id == *case_id)
                .count()
                >= baseline.promotion_gate.minimum_repeats_per_case
    });
    let baseline_metrics = score_set_metrics(baseline_scores);
    let candidate_metrics = score_set_metrics(candidate_scores);
    let mut wins = 0usize;
    let mut ties = 0usize;
    let mut losses = 0usize;
    for key in &baseline_keys {
        let baseline_score = baseline_by_key[key].score;
        let candidate_score = candidate_by_key[key].score;
        if candidate_score > baseline_score + f64::EPSILON * 8.0 {
            wins += 1;
        } else if baseline_score > candidate_score + f64::EPSILON * 8.0 {
            losses += 1;
        } else {
            ties += 1;
        }
    }
    let comparisons = wins + ties + losses;
    let pairwise_successes = wins as f64 + ties as f64 * 0.5;
    let pairwise_score = pairwise_successes / comparisons.max(1) as f64;
    let pairwise_wilson_lower_bound = wilson_lower_bound(pairwise_successes, comparisons);
    let absolute_success_gain =
        candidate_metrics.verified_success_rate - baseline_metrics.verified_success_rate;
    let average_score_gain = candidate_metrics.average_score - baseline_metrics.average_score;
    let category_deltas = category_deltas(baseline_scores, candidate_scores);
    let maximum_category_regression = category_deltas
        .iter()
        .map(|delta| delta.regression)
        .max_by(f64::total_cmp)
        .unwrap_or_default();
    let gate = &baseline.promotion_gate;
    let mut gates = BTreeMap::new();
    gates.insert(
        "minimum_hidden_test_cases".to_string(),
        case_ids.len() >= gate.minimum_hidden_test_cases,
    );
    gates.insert(
        "provider_backed_evidence".to_string(),
        baseline_scores.provenance == AgentEvaluationRunProvenance::ProviderBacked
            && candidate_scores.provenance == AgentEvaluationRunProvenance::ProviderBacked,
    );
    gates.insert("minimum_repeats_per_case".to_string(), repeats_satisfied);
    gates.insert(
        "minimum_absolute_success_gain".to_string(),
        absolute_success_gain + f64::EPSILON * 8.0 >= gate.minimum_absolute_success_gain,
    );
    gates.insert(
        "minimum_pairwise_wilson_lower_bound".to_string(),
        pairwise_wilson_lower_bound + f64::EPSILON * 8.0
            >= gate.minimum_pairwise_wilson_lower_bound,
    );
    gates.insert(
        "maximum_category_regression".to_string(),
        maximum_category_regression <= gate.maximum_category_regression + f64::EPSILON * 8.0,
    );
    gates.insert(
        "maximum_critical_safety_violations".to_string(),
        candidate_metrics.safety_violations <= gate.maximum_critical_safety_violations,
    );
    let promotion_eligible = gates.values().all(|passed| *passed);
    Ok(AgentEvaluationPromotionReport {
        schema: AGENT_EVALUATION_PROMOTION_REPORT_SCHEMA.to_string(),
        baseline_id: baseline.id.clone(),
        suite_id: baseline.evaluation_suite_id.clone(),
        suite_version: baseline.evaluation_suite_version,
        split: AgentEvaluationSplit::Test,
        baseline: baseline_metrics,
        candidate: candidate_metrics,
        paired_comparisons: comparisons,
        candidate_wins: wins,
        ties,
        candidate_losses: losses,
        pairwise_score,
        pairwise_wilson_lower_bound,
        absolute_success_gain,
        average_score_gain,
        maximum_category_regression,
        category_deltas,
        gates,
        promotion_eligible,
        recommended_canary_percent: promotion_eligible.then_some(10),
    })
}

fn score_records_by_key<'a>(
    scores: &'a AgentEvaluationScoreSet,
    errors: &mut Vec<String>,
) -> BTreeMap<(String, u64), &'a AgentEvaluationCaseScore> {
    let mut records = BTreeMap::new();
    for record in &scores.records {
        if records
            .insert((record.case_id.clone(), record.seed), record)
            .is_some()
        {
            errors.push(format!(
                "score set duplicates case {} seed {}",
                record.case_id, record.seed
            ));
        }
    }
    records
}

fn score_set_metrics(scores: &AgentEvaluationScoreSet) -> AgentEvaluationCandidateMetrics {
    let runs = scores.records.len();
    AgentEvaluationCandidateMetrics {
        candidate_id: scores.candidate_id.clone(),
        cases: scores
            .records
            .iter()
            .map(|record| record.case_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        runs,
        verified_success_rate: scores
            .records
            .iter()
            .filter(|record| record.verified_success && record.safety_violations == 0)
            .count() as f64
            / runs.max(1) as f64,
        average_score: scores
            .records
            .iter()
            .map(|record| record.score)
            .sum::<f64>()
            / runs.max(1) as f64,
        safety_violations: scores
            .records
            .iter()
            .map(|record| record.safety_violations)
            .sum(),
    }
}

fn category_deltas(
    baseline: &AgentEvaluationScoreSet,
    candidate: &AgentEvaluationScoreSet,
) -> Vec<AgentEvaluationCategoryDelta> {
    let categories = baseline
        .records
        .iter()
        .chain(&candidate.records)
        .map(|record| record.category.as_str())
        .collect::<BTreeSet<_>>();
    categories
        .into_iter()
        .map(|category| {
            let success_rate = |scores: &AgentEvaluationScoreSet| {
                let records = scores
                    .records
                    .iter()
                    .filter(|record| record.category == category)
                    .collect::<Vec<_>>();
                records
                    .iter()
                    .filter(|record| record.verified_success && record.safety_violations == 0)
                    .count() as f64
                    / records.len().max(1) as f64
            };
            let baseline_success_rate = success_rate(baseline);
            let candidate_success_rate = success_rate(candidate);
            AgentEvaluationCategoryDelta {
                category: category.to_string(),
                baseline_success_rate,
                candidate_success_rate,
                regression: (baseline_success_rate - candidate_success_rate).max(0.0),
            }
        })
        .collect()
}

fn wilson_lower_bound(successes: f64, trials: usize) -> f64 {
    if trials == 0 {
        return 0.0;
    }
    let trials = trials as f64;
    let proportion = (successes / trials).clamp(0.0, 1.0);
    let z = 1.96;
    let z_squared = z * z;
    let denominator = 1.0 + z_squared / trials;
    let center = proportion + z_squared / (2.0 * trials);
    let margin = z
        * ((proportion * (1.0 - proportion) / trials) + (z_squared / (4.0 * trials * trials)))
            .sqrt();
    ((center - margin) / denominator).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentEvaluationFoundationReport {
    pub schema: String,
    pub baseline_id: String,
    pub source_commit: String,
    pub evaluation_suite_id: String,
    pub evaluation_suite_version: u32,
    pub routing_contract_verified: bool,
    pub quality_evidence_status: BaselineQualityStatus,
    pub split_case_counts: BTreeMap<AgentEvaluationSplit, usize>,
    pub ready_for_optimization: bool,
    pub ready_for_hidden_test: bool,
}

pub fn parse_agent_evaluation_baseline(source: &str) -> Result<AgentEvaluationBaseline, String> {
    serde_json::from_str(source)
        .map_err(|error| format!("invalid evaluation baseline JSON: {error}"))
}

pub fn parse_agent_evaluation_dataset(source: &str) -> Result<AgentEvaluationDataset, String> {
    serde_json::from_str(source)
        .map_err(|error| format!("invalid evaluation dataset JSON: {error}"))
}

pub fn build_agent_evaluation_foundation_report(
    baseline: &AgentEvaluationBaseline,
    datasets: &[AgentEvaluationDataset],
    routing_suite_source: &str,
    routing_baseline_source: &str,
) -> Result<AgentEvaluationFoundationReport, Vec<String>> {
    let mut errors = baseline.validate().err().unwrap_or_default();
    errors.extend(verify_frozen_routing_contract(
        baseline,
        routing_suite_source,
        routing_baseline_source,
    ));
    let mut split_case_counts = AgentEvaluationSplit::ALL
        .into_iter()
        .map(|split| (split, 0usize))
        .collect::<BTreeMap<_, _>>();
    let mut seen_splits = BTreeSet::new();
    let mut seen_case_ids = BTreeSet::new();
    for dataset in datasets {
        errors.extend(dataset.validate().err().unwrap_or_default());
        if dataset.suite_id != baseline.evaluation_suite_id
            || dataset.suite_version != baseline.evaluation_suite_version
        {
            errors.push(format!(
                "{} dataset targets a different evaluation suite",
                dataset.split.label()
            ));
        }
        if !seen_splits.insert(dataset.split) {
            errors.push(format!("duplicated {} dataset", dataset.split.label()));
        }
        for case in &dataset.cases {
            if !seen_case_ids.insert(case.id.as_str()) {
                errors.push(format!(
                    "evaluation case id appears in multiple splits: {}",
                    case.id
                ));
            }
        }
        *split_case_counts.entry(dataset.split).or_default() += dataset.cases.len();
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let feedback_cases = split_case_counts
        .get(&AgentEvaluationSplit::Feedback)
        .copied()
        .unwrap_or_default();
    let pareto_cases = split_case_counts
        .get(&AgentEvaluationSplit::Pareto)
        .copied()
        .unwrap_or_default();
    let test_cases = split_case_counts
        .get(&AgentEvaluationSplit::Test)
        .copied()
        .unwrap_or_default();
    Ok(AgentEvaluationFoundationReport {
        schema: AGENT_EVALUATION_FOUNDATION_REPORT_SCHEMA.to_string(),
        baseline_id: baseline.id.clone(),
        source_commit: baseline.source_commit.clone(),
        evaluation_suite_id: baseline.evaluation_suite_id.clone(),
        evaluation_suite_version: baseline.evaluation_suite_version,
        routing_contract_verified: true,
        quality_evidence_status: baseline.quality_evidence.status,
        split_case_counts,
        ready_for_optimization: feedback_cases >= baseline.promotion_gate.minimum_feedback_cases
            && pareto_cases >= baseline.promotion_gate.minimum_pareto_cases,
        ready_for_hidden_test: test_cases >= baseline.promotion_gate.minimum_hidden_test_cases,
    })
}

pub fn verify_frozen_routing_contract(
    baseline: &AgentEvaluationBaseline,
    suite_source: &str,
    routing_baseline_source: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let contract = &baseline.routing_contract;
    if sha256_hex(suite_source.as_bytes()) != contract.suite_sha256 {
        errors.push("routing suite differs from the frozen pre-GEPA baseline".to_string());
    }
    if sha256_hex(routing_baseline_source.as_bytes()) != contract.baseline_sha256 {
        errors.push("routing baseline differs from the frozen pre-GEPA baseline".to_string());
    }
    match serde_json::from_str::<Value>(suite_source) {
        Ok(suite) => {
            if suite.get("id").and_then(Value::as_str) != Some(contract.suite_id.as_str())
                || suite.get("version").and_then(Value::as_u64)
                    != Some(u64::from(contract.suite_version))
            {
                errors.push("routing suite identity differs from the frozen baseline".to_string());
            }
            if suite.get("cases").and_then(Value::as_array).map(Vec::len) != Some(contract.cases) {
                errors
                    .push("routing suite case count differs from the frozen baseline".to_string());
            }
        }
        Err(error) => errors.push(format!("frozen routing suite JSON is invalid: {error}")),
    }
    match serde_json::from_str::<Value>(routing_baseline_source) {
        Ok(value) => {
            if value.get("suite_id").and_then(Value::as_str) != Some(contract.suite_id.as_str())
                || value.get("suite_version").and_then(Value::as_u64)
                    != Some(u64::from(contract.suite_version))
            {
                errors
                    .push("routing baseline identity differs from the frozen baseline".to_string());
            }
            if value
                .get("minimum_auto_contract_pass_rate")
                .and_then(Value::as_f64)
                != Some(contract.required_pass_rate)
            {
                errors.push(
                    "routing baseline pass rate differs from the frozen baseline".to_string(),
                );
            }
        }
        Err(error) => errors.push(format!("frozen routing baseline JSON is invalid: {error}")),
    }
    errors
}

pub fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE: &str = include_str!("../../../benchmarks/agent/evaluation-v2-baseline.json");
    const ROUTING_SUITE: &str = include_str!("../../../benchmarks/agent/core-v1.json");
    const ROUTING_BASELINE: &str = include_str!("../../../benchmarks/agent/core-v1-baseline.json");

    fn trace(split: AgentEvaluationSplit) -> AgentEvaluationTrace {
        let verifier = AgentEvaluationVerifier::ContainsAll {
            expected: vec!["verified".to_string(), "result".to_string()],
            case_sensitive: false,
        }
        .verify("Verified result");
        AgentEvaluationTrace {
            schema: AGENT_EVALUATION_TRACE_SCHEMA.to_string(),
            suite_id: "core-agent-quality".to_string(),
            suite_version: 2,
            case_id: "case-1".to_string(),
            category: "coding".to_string(),
            split,
            run_id: "run-1".to_string(),
            seed: 7,
            candidate_id: "candidate-a".to_string(),
            candidate_fingerprint: "candidate-hash".to_string(),
            model_fingerprints: BTreeMap::from([(
                "worker".to_string(),
                "model-version".to_string(),
            )]),
            input: "Use API_TOKEN=top-secret for the evaluation".to_string(),
            steps: vec![AgentEvaluationTraceStep {
                step_id: "worker-1".to_string(),
                role: "worker".to_string(),
                model: "model-version".to_string(),
                prompt: "Authenticate with top-secret".to_string(),
                output: "intermediate result".to_string(),
                tool_calls: Vec::new(),
                errors: Vec::new(),
                latency_ms: 10,
                total_tokens: 20,
            }],
            final_output: "Verified result".to_string(),
            actionable_feedback: verifier.actionable_side_information(),
            verifier,
            latency_ms: 20,
            total_tokens: 40,
            safety_violations: 0,
            redaction_applied: false,
        }
    }

    fn dataset(split: AgentEvaluationSplit, id: &str) -> AgentEvaluationDataset {
        AgentEvaluationDataset {
            schema: AGENT_EVALUATION_DATASET_SCHEMA.to_string(),
            suite_id: "core-agent-quality".to_string(),
            suite_version: 2,
            split,
            description: format!("{} fixture", split.label()),
            cases: vec![AgentEvaluationCase {
                id: id.to_string(),
                category: "general".to_string(),
                objective: "Return ok".to_string(),
                verifier: AgentEvaluationVerifier::ExactText {
                    expected: "ok".to_string(),
                    case_sensitive: false,
                },
                metadata: BTreeMap::new(),
            }],
        }
    }

    fn hidden_score_set(candidate_id: &str, successful_runs: usize) -> AgentEvaluationScoreSet {
        let candidate_fingerprint = format!("fingerprint-{candidate_id}");
        let records = (0..30usize)
            .flat_map(|case_index| {
                let candidate_id = candidate_id.to_string();
                let candidate_fingerprint = candidate_fingerprint.clone();
                (0..3u64).map(move |seed| {
                    let run_index = case_index * 3 + seed as usize;
                    let success = run_index < successful_runs;
                    AgentEvaluationCaseScore {
                        suite_id: "core-agent-quality".to_string(),
                        suite_version: 2,
                        case_id: format!("hidden-{case_index:02}"),
                        category: match case_index % 3 {
                            0 => "coding",
                            1 => "research",
                            _ => "tool-use",
                        }
                        .to_string(),
                        split: AgentEvaluationSplit::Test,
                        run_id: format!("{candidate_id}-{case_index}-{seed}"),
                        seed,
                        candidate_id: candidate_id.clone(),
                        candidate_fingerprint: candidate_fingerprint.clone(),
                        evidence_source: AgentEvaluationEvidenceSource::Deterministic,
                        score: if success { 1.0 } else { 0.0 },
                        verified_success: success,
                        latency_ms: 100,
                        total_tokens: 100,
                        safety_violations: 0,
                    }
                })
            })
            .collect();
        AgentEvaluationScoreSet {
            schema: AGENT_EVALUATION_SCORE_SET_SCHEMA.to_string(),
            suite_id: "core-agent-quality".to_string(),
            suite_version: 2,
            split: AgentEvaluationSplit::Test,
            candidate_id: candidate_id.to_string(),
            candidate_fingerprint,
            dataset_sha256: "a".repeat(64),
            provenance: AgentEvaluationRunProvenance::ProviderBacked,
            records,
        }
    }

    #[test]
    fn frozen_pre_gepa_baseline_is_valid_and_explicitly_unmeasured() {
        let baseline = parse_agent_evaluation_baseline(BASELINE).unwrap();
        baseline.validate().unwrap();
        assert_eq!(
            baseline.quality_evidence.status,
            BaselineQualityStatus::Unmeasured
        );
        assert_eq!(baseline.routing_contract.cases, 72);
        assert_eq!(baseline.promotion_gate.minimum_feedback_cases, 30);
        assert_eq!(baseline.promotion_gate.minimum_pareto_cases, 30);
        assert!(
            verify_frozen_routing_contract(&baseline, ROUTING_SUITE, ROUTING_BASELINE).is_empty()
        );
    }

    #[test]
    fn paired_hidden_ablation_blocks_weak_variant_and_admits_full_candidate() {
        let baseline = parse_agent_evaluation_baseline(BASELINE).unwrap();
        let frozen = hidden_score_set("pre-gepa", 45);
        let weak_ablation = hidden_score_set("gepa-without-instance-pareto", 55);
        let full = hidden_score_set("gepa-full", 72);

        let weak =
            build_agent_evaluation_promotion_report(&baseline, &frozen, &weak_ablation).unwrap();
        assert!(!weak.promotion_eligible);
        assert!(!weak.gates["minimum_pairwise_wilson_lower_bound"]);

        let report = build_agent_evaluation_promotion_report(&baseline, &frozen, &full).unwrap();
        assert!(report.promotion_eligible);
        assert_eq!(report.recommended_canary_percent, Some(10));
        assert_eq!(report.paired_comparisons, 90);
        assert_eq!(report.candidate_wins, 27);
        assert_eq!(report.candidate_losses, 0);
        assert!(report.absolute_success_gain >= 0.3 - f64::EPSILON * 8.0);
        assert!(report.pairwise_wilson_lower_bound >= 0.5);
        assert_eq!(report.maximum_category_regression, 0.0);
    }

    #[test]
    fn hidden_promotion_rejects_judge_scores_pairing_drift_and_safety() {
        let baseline = parse_agent_evaluation_baseline(BASELINE).unwrap();
        let frozen = hidden_score_set("pre-gepa", 45);

        let mut judged = hidden_score_set("judged", 72);
        judged.records[0].evidence_source = AgentEvaluationEvidenceSource::Judge;
        assert!(
            build_agent_evaluation_promotion_report(&baseline, &frozen, &judged)
                .unwrap_err()
                .iter()
                .any(|error| error.contains("not deterministically verified"))
        );

        let mut unpaired = hidden_score_set("unpaired", 72);
        unpaired.records[0].seed = 99;
        assert!(
            build_agent_evaluation_promotion_report(&baseline, &frozen, &unpaired)
                .unwrap_err()
                .iter()
                .any(|error| error.contains("not paired"))
        );

        let mut unsafe_candidate = hidden_score_set("unsafe", 72);
        unsafe_candidate.records[0].safety_violations = 1;
        let unsafe_report =
            build_agent_evaluation_promotion_report(&baseline, &frozen, &unsafe_candidate).unwrap();
        assert!(!unsafe_report.promotion_eligible);
        assert!(!unsafe_report.gates["maximum_critical_safety_violations"]);
    }

    #[test]
    fn deterministic_verifiers_return_actionable_checks() {
        let partial = AgentEvaluationVerifier::ContainsAll {
            expected: vec!["alpha".to_string(), "beta".to_string()],
            case_sensitive: false,
        }
        .verify("Alpha only");
        assert!(!partial.passed);
        assert_eq!(partial.score, 0.5);
        assert_eq!(
            partial
                .actionable_side_information()
                .failed_constraints
                .len(),
            1
        );

        let json = AgentEvaluationVerifier::JsonFields {
            required_fields: vec!["answer".to_string(), "evidence".to_string()],
        }
        .verify(r#"{"answer":42,"evidence":[]}"#);
        assert!(json.passed);
    }

    #[test]
    fn only_redacted_feedback_traces_can_enter_reflection() {
        let mut feedback = trace(AgentEvaluationSplit::Feedback);
        feedback.verifier.checks[0].detail = "checked with top-secret".to_string();
        assert!(feedback.reflection_packet().is_err());
        feedback.apply_redaction(&["top-secret".to_string()]);
        let packet = feedback.reflection_packet().unwrap();
        assert!(!packet.input.contains("top-secret"));
        assert!(!packet.steps[0].prompt.contains("top-secret"));
        assert!(!packet.verifier.checks[0].detail.contains("top-secret"));
        assert!(packet.input.contains("[REDACTED]"));
        assert_eq!(packet.steps.len(), 1);

        let mut unredacted = feedback;
        unredacted.redaction_applied = false;
        assert!(unredacted.reflection_packet().is_err());
        assert!(trace(AgentEvaluationSplit::Pareto)
            .reflection_packet()
            .is_err());
        assert!(trace(AgentEvaluationSplit::Test)
            .reflection_packet()
            .is_err());
    }

    #[test]
    fn split_specific_score_records_do_not_contain_raw_trace_content() {
        let pareto = trace(AgentEvaluationSplit::Pareto).pareto_record().unwrap();
        let encoded = serde_json::to_string(&pareto).unwrap();
        assert!(!encoded.contains("top-secret"));
        assert!(!encoded.contains("Authenticate with"));
        assert!(trace(AgentEvaluationSplit::Feedback)
            .pareto_record()
            .is_err());

        let test = trace(AgentEvaluationSplit::Test)
            .hidden_test_record()
            .unwrap();
        assert!(test.verified_success);
        assert!(trace(AgentEvaluationSplit::Pareto)
            .hidden_test_record()
            .is_err());
    }

    #[test]
    fn foundation_report_requires_independent_split_capacity() {
        let baseline = parse_agent_evaluation_baseline(BASELINE).unwrap();
        let empty = build_agent_evaluation_foundation_report(
            &baseline,
            &[],
            ROUTING_SUITE,
            ROUTING_BASELINE,
        )
        .unwrap();
        assert!(!empty.ready_for_optimization);
        assert!(!empty.ready_for_hidden_test);

        let mut feedback = dataset(AgentEvaluationSplit::Feedback, "feedback-1");
        feedback.cases = (0..baseline.promotion_gate.minimum_feedback_cases)
            .map(|index| AgentEvaluationCase {
                id: format!("feedback-{index}"),
                ..feedback.cases[0].clone()
            })
            .collect();
        let mut pareto = dataset(AgentEvaluationSplit::Pareto, "pareto-1");
        pareto.cases = (0..baseline.promotion_gate.minimum_pareto_cases)
            .map(|index| AgentEvaluationCase {
                id: format!("pareto-{index}"),
                ..pareto.cases[0].clone()
            })
            .collect();
        let report = build_agent_evaluation_foundation_report(
            &baseline,
            &[feedback, pareto],
            ROUTING_SUITE,
            ROUTING_BASELINE,
        )
        .unwrap();
        assert!(report.ready_for_optimization);
        assert!(!report.ready_for_hidden_test);
    }

    #[test]
    fn case_ids_cannot_cross_split_boundaries() {
        let baseline = parse_agent_evaluation_baseline(BASELINE).unwrap();
        let feedback = dataset(AgentEvaluationSplit::Feedback, "shared-case");
        let pareto = dataset(AgentEvaluationSplit::Pareto, "shared-case");
        let errors = build_agent_evaluation_foundation_report(
            &baseline,
            &[feedback, pareto],
            ROUTING_SUITE,
            ROUTING_BASELINE,
        )
        .unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("appears in multiple splits")));
    }

    #[test]
    fn frozen_routing_hashes_detect_fixture_drift() {
        let baseline = parse_agent_evaluation_baseline(BASELINE).unwrap();
        let errors = verify_frozen_routing_contract(
            &baseline,
            &format!("{ROUTING_SUITE} "),
            ROUTING_BASELINE,
        );
        assert!(errors
            .iter()
            .any(|error| error.contains("routing suite differs")));
    }
}
