#![allow(dead_code)]

use agent_runtime::{
    DeliveryVerificationEvidence, DeliveryVerificationObligation,
    MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES,
};
use orchestrator::sha256_hex;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;

pub(super) const DELIVERY_VERIFICATION_PROTOCOL_SCHEMA: &str =
    "cindx.agent-eval.delivery-verification-protocol.v2";
pub(super) const DELIVERY_VERIFICATION_SUITE_SCHEMA: &str =
    "cindx.agent-eval.delivery-verification-suite.v1";
pub(super) const DELIVERY_VERIFICATION_PROTOCOL_ID: &str =
    "cindx-delivery-verification-protocol-v2";
pub(super) const CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID: &str =
    "cindx-delivery-verification-protocol-v1";
pub(super) const DELIVERY_VERIFICATION_SUITE_ID: &str = "cindx-delivery-verification-v1";
pub(super) const DELIVERY_VERIFICATION_PROTOCOL_RELATIVE_PATH: &str =
    "benchmarks/agent/delivery-verification-protocol-v2.json";
pub(super) const DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH: &str =
    "benchmarks/agent/delivery-verification-v1.json";
pub(super) const DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME: &str =
    "cindx-delivery-verification-v2-execute";
pub(super) const DELIVERY_VERIFICATION_EXECUTION_JOURNAL_SCHEMA: &str =
    "cindx.agent-eval.delivery-verification-execution-journal.v2";

const CASE_COUNT: usize = 32;
const CALIBRATION_CASE_COUNT: usize = 8;
const HOLDOUT_CASE_COUNT: usize = 24;
const CASE_DIGEST_DOMAIN: &[u8] = b"cindx.agent-eval.delivery-verification-case.v1\0";
const CASE_ORDER_DIGEST_DOMAIN: &[u8] = b"cindx.agent-eval.delivery-verification-case-order.v1\0";
const HIDDEN_ORACLE_DIGEST_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-model-hidden-oracle.v1\0";
const BUDGET_DIGEST_DOMAIN: &[u8] = b"cindx.agent-eval.delivery-verification-budget.v1\0";
const MAX_PROTOCOL_BYTES: usize = 256 * 1024;
const MAX_SUITE_BYTES: usize = 1024 * 1024;
const MAX_OBJECTIVE_BYTES: usize = 4 * 1024;
const MAX_ORACLE_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolManifest {
    schema: String,
    id: String,
    version: u32,
    suite: FrozenSuite,
    design: FrozenDesign,
    budget: ProtocolBudget,
    instrumentation: FrozenInstrumentation,
    calibration_gate: CalibrationGate,
    holdout_decision: HoldoutDecision,
    stop_contract: StopContract,
    execution: ExecutionContract,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenSuite {
    path: String,
    sha256: String,
    schema: String,
    id: String,
    case_count: usize,
    case_order_sha256: String,
    hidden_oracle_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenDesign {
    calibration_cases: usize,
    holdout_cases: usize,
    total_cases: usize,
    strata: Vec<DeliveryVerificationStratum>,
    calibration_cases_per_stratum: usize,
    holdout_cases_per_stratum: usize,
    minimum_evidence_items_per_case: usize,
    case_order: String,
    matched_arms: Vec<String>,
    shared_owner_draft: bool,
    control_output: String,
    treatment_workflow: Vec<String>,
    max_owner_repairs_per_case: usize,
    max_verifier_rechecks_per_case: usize,
    owner_and_verifier_models_must_differ: bool,
    oracle_predicate: String,
    oracle_visibility: String,
    tool_calls_allowed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenInstrumentation {
    request_mode: String,
    prepared_wire_payload: String,
    reservation_digest_authorities: Vec<String>,
    terminal_request_payload_binding: String,
    execution_journal_schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProtocolBudget {
    pub(super) max_logical_model_calls_per_case: usize,
    pub(super) max_logical_model_calls_total: usize,
    pub(super) max_physical_model_attempts_total: usize,
    pub(super) transport_retries: usize,
    pub(super) max_bound_context_bytes: usize,
    pub(super) max_owner_output_tokens: u64,
    pub(super) max_verifier_output_tokens: u64,
    pub(super) max_total_tokens_per_case: u64,
    pub(super) max_total_tokens_campaign: u64,
    pub(super) model_call_timeout_ms: u64,
    pub(super) case_timeout_ms: u64,
    pub(super) campaign_timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CalibrationGate {
    complete_cases: usize,
    minimum_control_failures: usize,
    minimum_treatment_only_wins: usize,
    maximum_control_only_losses: usize,
    maximum_structural_failures: usize,
    maximum_treatment_execution_failures: usize,
    calibration_results_excluded_from_holdout: bool,
    prompt_model_threshold_changes_after_calibration: String,
    action_on_failure: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HoldoutDecision {
    complete_cases: usize,
    minimum_treatment_only_wins: usize,
    maximum_control_only_losses: usize,
    maximum_structural_failures: usize,
    maximum_treatment_execution_failures: usize,
    primary_test: String,
    alpha_millionths: u64,
    five_zero_p_millionths: u64,
    calibration_results_excluded: bool,
    incomplete_pairs: String,
    no_regression_required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopContract {
    calibration_gate_failure: String,
    any_structural_failure: String,
    any_treatment_execution_failure: String,
    any_holdout_control_only_loss: String,
    insufficient_holdout_wins: String,
    budget_exhaustion: String,
    case_replacement: String,
    frozen_attempt_rerun: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionContract {
    runner_binary: String,
    required_feature: String,
    execution_authorized: bool,
    online_execution_requires_explicit_authorization: bool,
    provider_transport_retries: usize,
    failures_retained: bool,
    case_replacement_allowed: bool,
    gepa_changes_authorized: bool,
    learning_changes_authorized: bool,
    production_changes_authorized: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryVerificationSuite {
    schema: String,
    id: String,
    version: u32,
    description: String,
    cases: Vec<DeliveryVerificationCase>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryVerificationCase {
    ordinal: usize,
    id: String,
    split: DeliveryVerificationSplit,
    stratum: DeliveryVerificationStratum,
    objective: String,
    obligations: Vec<DeliveryVerificationObligation>,
    evidence: Vec<DeliveryVerificationEvidence>,
    oracle: ModelHiddenOracle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryVerificationSplit {
    Calibration,
    Holdout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryVerificationStratum {
    UnsupportedClaim,
    OmittedObligation,
    Contradiction,
    Preservation,
}

const STRATA: [DeliveryVerificationStratum; 4] = [
    DeliveryVerificationStratum::UnsupportedClaim,
    DeliveryVerificationStratum::OmittedObligation,
    DeliveryVerificationStratum::Contradiction,
    DeliveryVerificationStratum::Preservation,
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelHiddenOracle {
    hidden_from_model: bool,
    required_groups: Vec<Vec<String>>,
    forbidden: Vec<String>,
    exact_json: Value,
}

pub(super) struct ValidatedProtocol<'a> {
    manifest: ProtocolManifest,
    suite: DeliveryVerificationSuite,
    manifest_bytes: &'a [u8],
    manifest_sha256: String,
    suite_sha256: String,
    case_sha256s: Vec<String>,
    budget_sha256: String,
    hidden_oracle_sha256: String,
}

#[derive(Clone, Copy)]
pub(super) struct ValidatedCase<'a> {
    case: &'a DeliveryVerificationCase,
    case_sha256: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelInput<'a> {
    objective: &'a str,
    obligations: &'a [DeliveryVerificationObligation],
    evidence: &'a [DeliveryVerificationEvidence],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OracleEvaluation {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MatchedPairCounts {
    pub(super) complete_cases: usize,
    pub(super) control_failures: usize,
    pub(super) treatment_only_wins: usize,
    pub(super) control_only_losses: usize,
    pub(super) structural_failures: usize,
    pub(super) treatment_execution_failures: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CalibrationDecision {
    OpenHoldout,
    TerminalFutility,
    Inconclusive,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HoldoutDecisionResult {
    EvidenceOfUplift,
    NoEvidence,
    Regression,
    Inconclusive,
    Invalid,
}

pub(super) fn parse_and_validate_protocol<'a>(
    manifest_bytes: &'a [u8],
    suite_bytes: &'a [u8],
) -> Result<ValidatedProtocol<'a>, String> {
    validate_canonical_document(manifest_bytes, MAX_PROTOCOL_BYTES, "protocol manifest")?;
    validate_canonical_document(suite_bytes, MAX_SUITE_BYTES, "delivery verification suite")?;
    reject_raw_credentials(manifest_bytes, "protocol manifest")?;
    reject_raw_credentials(suite_bytes, "delivery verification suite")?;

    let manifest: ProtocolManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("invalid delivery verification protocol JSON: {error}"))?;
    let suite: DeliveryVerificationSuite = serde_json::from_slice(suite_bytes)
        .map_err(|error| format!("invalid delivery verification suite JSON: {error}"))?;
    let suite_sha256 = sha256_hex(suite_bytes);
    let case_sha256s = validate_suite(&suite)?;
    let case_order_sha256 = case_order_digest(&suite, &case_sha256s)?;
    let hidden_oracle_sha256 = hidden_oracle_digest(&suite)?;
    let budget_sha256 = domain_hash_json(
        BUDGET_DIGEST_DOMAIN,
        &manifest.budget,
        "delivery verification budget",
    )?;
    validate_manifest(
        &manifest,
        &suite,
        &suite_sha256,
        &case_order_sha256,
        &hidden_oracle_sha256,
    )?;

    Ok(ValidatedProtocol {
        manifest,
        suite,
        manifest_bytes,
        manifest_sha256: sha256_hex(manifest_bytes),
        suite_sha256,
        case_sha256s,
        budget_sha256,
        hidden_oracle_sha256,
    })
}

impl ValidatedProtocol<'_> {
    pub(super) fn protocol_id(&self) -> &str {
        &self.manifest.id
    }

    pub(super) fn suite_id(&self) -> &str {
        &self.suite.id
    }

    pub(super) fn manifest_bytes(&self) -> &[u8] {
        self.manifest_bytes
    }

    pub(super) fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub(super) fn suite_sha256(&self) -> &str {
        &self.suite_sha256
    }

    pub(super) fn case_sha256s(&self) -> &[String] {
        &self.case_sha256s
    }

    pub(super) fn budget(&self) -> &ProtocolBudget {
        &self.manifest.budget
    }

    pub(super) fn budget_sha256(&self) -> &str {
        &self.budget_sha256
    }

    pub(super) fn hidden_oracle_sha256(&self) -> &str {
        &self.hidden_oracle_sha256
    }

    pub(super) fn execution_authorized(&self) -> bool {
        self.manifest.execution.execution_authorized
    }

    pub(super) fn execute_binary_name(&self) -> &str {
        &self.manifest.execution.runner_binary
    }

    pub(super) fn cases(&self) -> impl ExactSizeIterator<Item = ValidatedCase<'_>> {
        self.suite
            .cases
            .iter()
            .zip(self.case_sha256s.iter())
            .map(|(case, case_sha256)| ValidatedCase { case, case_sha256 })
    }

    pub(super) fn calibration_cases(&self) -> impl Iterator<Item = ValidatedCase<'_>> {
        self.cases()
            .filter(|case| case.split() == DeliveryVerificationSplit::Calibration)
    }

    pub(super) fn holdout_cases(&self) -> impl Iterator<Item = ValidatedCase<'_>> {
        self.cases()
            .filter(|case| case.split() == DeliveryVerificationSplit::Holdout)
    }
}

impl ValidatedCase<'_> {
    pub(super) fn ordinal(&self) -> usize {
        self.case.ordinal
    }

    pub(super) fn id(&self) -> &str {
        &self.case.id
    }

    pub(super) fn split(&self) -> DeliveryVerificationSplit {
        self.case.split
    }

    pub(super) fn stratum(&self) -> DeliveryVerificationStratum {
        self.case.stratum
    }

    pub(super) fn objective(&self) -> &str {
        &self.case.objective
    }

    pub(super) fn obligations(&self) -> &[DeliveryVerificationObligation] {
        &self.case.obligations
    }

    pub(super) fn evidence(&self) -> &[DeliveryVerificationEvidence] {
        &self.case.evidence
    }

    pub(super) fn case_sha256(&self) -> &str {
        self.case_sha256
    }

    pub(super) fn model_input_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&ModelInput {
            objective: &self.case.objective,
            obligations: &self.case.obligations,
            evidence: &self.case.evidence,
        })
        .map_err(|error| format!("could not encode model-facing case input: {error}"))
    }

    pub(super) fn evaluate_output(&self, output: &str) -> OracleEvaluation {
        if output.is_empty() || output.len() > MAX_ORACLE_OUTPUT_BYTES {
            return OracleEvaluation::Failed;
        }
        let Ok(parsed) = parse_unique_json(output) else {
            return OracleEvaluation::Failed;
        };
        if parsed != self.case.oracle.exact_json {
            return OracleEvaluation::Failed;
        }
        if self
            .case
            .oracle
            .required_groups
            .iter()
            .any(|group| !group.iter().any(|needle| output.contains(needle)))
            || self
                .case
                .oracle
                .forbidden
                .iter()
                .any(|needle| output.contains(needle))
        {
            return OracleEvaluation::Failed;
        }
        OracleEvaluation::Passed
    }
}

pub(super) fn calibration_decision(counts: MatchedPairCounts) -> CalibrationDecision {
    if !counts_are_possible(counts, CALIBRATION_CASE_COUNT) {
        return CalibrationDecision::Invalid;
    }
    if counts.complete_cases != CALIBRATION_CASE_COUNT
        || counts.structural_failures > 0
        || counts.treatment_execution_failures > 0
    {
        return CalibrationDecision::Inconclusive;
    }
    if counts.control_only_losses > 0
        || counts.control_failures < 2
        || counts.treatment_only_wins < 1
    {
        return CalibrationDecision::TerminalFutility;
    }
    CalibrationDecision::OpenHoldout
}

pub(super) fn holdout_decision(counts: MatchedPairCounts) -> HoldoutDecisionResult {
    if !counts_are_possible(counts, HOLDOUT_CASE_COUNT) {
        return HoldoutDecisionResult::Invalid;
    }
    if counts.complete_cases != HOLDOUT_CASE_COUNT
        || counts.structural_failures > 0
        || counts.treatment_execution_failures > 0
    {
        return HoldoutDecisionResult::Inconclusive;
    }
    if counts.control_only_losses > 0 {
        return HoldoutDecisionResult::Regression;
    }
    if counts.treatment_only_wins >= 5 {
        HoldoutDecisionResult::EvidenceOfUplift
    } else {
        HoldoutDecisionResult::NoEvidence
    }
}

struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

struct UniqueJsonVisitor;

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueJson)
            .ok_or_else(|| E::custom("JSON number is not finite"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        UniqueJson::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueJson(value)) = sequence.next_element::<UniqueJson>()? {
            values.push(value);
        }
        Ok(UniqueJson(Value::Array(values)))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(<A::Error as de::Error>::custom(format!(
                    "duplicate JSON object key {key:?}"
                )));
            }
            let UniqueJson(value) = map.next_value::<UniqueJson>()?;
            values.insert(key, value);
        }
        Ok(UniqueJson(Value::Object(values)))
    }
}

fn parse_unique_json(input: &str) -> Result<Value, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value = UniqueJson::deserialize(&mut deserializer)?.0;
    deserializer.end()?;
    Ok(value)
}

fn counts_are_possible(counts: MatchedPairCounts, maximum_cases: usize) -> bool {
    counts.complete_cases <= maximum_cases
        && counts.control_failures <= counts.complete_cases
        && counts.treatment_only_wins <= counts.control_failures
        && counts.treatment_only_wins + counts.control_only_losses <= counts.complete_cases
        && counts.structural_failures <= maximum_cases - counts.complete_cases
        && counts.treatment_execution_failures <= maximum_cases
}

fn validate_suite(suite: &DeliveryVerificationSuite) -> Result<Vec<String>, String> {
    if suite.schema != DELIVERY_VERIFICATION_SUITE_SCHEMA
        || suite.id != DELIVERY_VERIFICATION_SUITE_ID
        || suite.version != 1
        || suite.description.trim().is_empty()
        || suite.cases.len() != CASE_COUNT
    {
        return Err("delivery verification suite identity or case count is invalid".into());
    }

    let mut ids = BTreeSet::new();
    let mut split_strata = [[0usize; 4]; 2];
    let mut case_sha256s = Vec::with_capacity(CASE_COUNT);
    for (index, case) in suite.cases.iter().enumerate() {
        let expected_split = if index < CALIBRATION_CASE_COUNT {
            DeliveryVerificationSplit::Calibration
        } else {
            DeliveryVerificationSplit::Holdout
        };
        let expected_stratum = STRATA[index % STRATA.len()];
        if case.ordinal != index + 1
            || case.split != expected_split
            || case.stratum != expected_stratum
            || !is_lower_kebab(&case.id)
            || !ids.insert(case.id.as_str())
        {
            return Err(format!(
                "delivery verification case {} has invalid identity, split, order, or stratum",
                index + 1
            ));
        }
        validate_case(case)?;
        let split_index = usize::from(case.split == DeliveryVerificationSplit::Holdout);
        let stratum_index = STRATA
            .iter()
            .position(|candidate| *candidate == case.stratum)
            .expect("validated stratum is present");
        split_strata[split_index][stratum_index] += 1;
        case_sha256s.push(domain_hash_json(
            CASE_DIGEST_DOMAIN,
            case,
            "delivery verification case",
        )?);
    }
    if split_strata[0] != [2; 4] || split_strata[1] != [6; 4] {
        return Err("delivery verification strata are not balanced 2/6 by split".into());
    }
    Ok(case_sha256s)
}

fn validate_case(case: &DeliveryVerificationCase) -> Result<(), String> {
    validate_text(&case.objective, MAX_OBJECTIVE_BYTES, "case objective")?;
    if case.obligations.is_empty() || case.evidence.len() < 2 {
        return Err(format!(
            "delivery verification case {} lacks obligations or multi-item evidence",
            case.id
        ));
    }

    let mut previous_obligation_ref: Option<&str> = None;
    let mut context_bytes = 0usize;
    for obligation in &case.obligations {
        if !is_sha256(&obligation.obligation_ref)
            || previous_obligation_ref
                .is_some_and(|previous| previous >= obligation.obligation_ref.as_str())
        {
            return Err(format!(
                "delivery verification case {} has unordered obligation references",
                case.id
            ));
        }
        validate_text(
            &obligation.content,
            MAX_OBJECTIVE_BYTES,
            "obligation content",
        )?;
        context_bytes = context_bytes.saturating_add(obligation.content.len());
        previous_obligation_ref = Some(&obligation.obligation_ref);
    }

    let mut previous_evidence_ref = 0u64;
    for evidence in &case.evidence {
        if evidence.evidence_ref == 0 || evidence.evidence_ref <= previous_evidence_ref {
            return Err(format!(
                "delivery verification case {} has unordered evidence references",
                case.id
            ));
        }
        validate_text(&evidence.content, MAX_OBJECTIVE_BYTES, "evidence content")?;
        context_bytes = context_bytes.saturating_add(evidence.content.len());
        previous_evidence_ref = evidence.evidence_ref;
    }
    if context_bytes > MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES {
        return Err(format!(
            "delivery verification case {} exceeds the bound context limit",
            case.id
        ));
    }
    validate_oracle(&case.id, &case.oracle)
}

fn validate_oracle(case_id: &str, oracle: &ModelHiddenOracle) -> Result<(), String> {
    let Some(exact_object) = oracle.exact_json.as_object() else {
        return Err(format!(
            "delivery verification case {case_id} exact oracle is not an object"
        ));
    };
    if !oracle.hidden_from_model
        || exact_object.is_empty()
        || oracle.required_groups.is_empty()
        || oracle.forbidden.is_empty()
    {
        return Err(format!(
            "delivery verification case {case_id} has an incomplete model-hidden oracle"
        ));
    }
    let exact_json = serde_json::to_string(&oracle.exact_json)
        .map_err(|error| format!("could not encode case {case_id} exact oracle: {error}"))?;
    let mut required = BTreeSet::new();
    for group in &oracle.required_groups {
        if group.is_empty() {
            return Err(format!(
                "delivery verification case {case_id} has an empty required group"
            ));
        }
        let mut group_values = BTreeSet::new();
        for value in group {
            validate_text(value, 256, "oracle required value")?;
            if !group_values.insert(value.as_str()) || !required.insert(value.as_str()) {
                return Err(format!(
                    "delivery verification case {case_id} repeats an oracle predicate"
                ));
            }
        }
        if !group.iter().any(|value| exact_json.contains(value)) {
            return Err(format!(
                "delivery verification case {case_id} required oracle group misses exact JSON"
            ));
        }
    }
    let mut forbidden = BTreeSet::new();
    for value in &oracle.forbidden {
        validate_text(value, 256, "oracle forbidden value")?;
        if !forbidden.insert(value.as_str()) || exact_json.contains(value) {
            return Err(format!(
                "delivery verification case {case_id} has a conflicting forbidden predicate"
            ));
        }
    }
    Ok(())
}

fn validate_manifest(
    manifest: &ProtocolManifest,
    suite: &DeliveryVerificationSuite,
    suite_sha256: &str,
    case_order_sha256: &str,
    hidden_oracle_sha256: &str,
) -> Result<(), String> {
    if manifest.schema != DELIVERY_VERIFICATION_PROTOCOL_SCHEMA
        || manifest.id != DELIVERY_VERIFICATION_PROTOCOL_ID
        || manifest.version != 2
        || manifest.suite.path != DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH
        || manifest.suite.sha256 != suite_sha256
        || manifest.suite.schema != suite.schema
        || manifest.suite.id != suite.id
        || manifest.suite.case_count != CASE_COUNT
        || manifest.suite.case_order_sha256 != case_order_sha256
        || manifest.suite.hidden_oracle_sha256 != hidden_oracle_sha256
    {
        return Err("delivery verification protocol suite binding is invalid".into());
    }
    validate_design(&manifest.design)?;
    validate_budget(&manifest.budget)?;
    validate_instrumentation(&manifest.instrumentation)?;
    validate_calibration_gate(&manifest.calibration_gate)?;
    validate_holdout_decision(&manifest.holdout_decision)?;
    validate_stop_contract(&manifest.stop_contract)?;
    validate_execution(&manifest.execution)
}

fn validate_instrumentation(instrumentation: &FrozenInstrumentation) -> Result<(), String> {
    if instrumentation.request_mode != "non_streaming"
        || instrumentation.prepared_wire_payload != "immutable_prepared_bytes_dispatched_exactly"
        || instrumentation.reservation_digest_authorities
            != ["semantic_request_sha256", "wire_payload_sha256"]
        || instrumentation.terminal_request_payload_binding
            != "request_payload_sha256_equals_reserved_wire_payload_sha256"
        || instrumentation.execution_journal_schema
            != DELIVERY_VERIFICATION_EXECUTION_JOURNAL_SCHEMA
    {
        return Err("delivery verification instrumentation authority is invalid".into());
    }
    Ok(())
}

fn validate_design(design: &FrozenDesign) -> Result<(), String> {
    if design.calibration_cases != CALIBRATION_CASE_COUNT
        || design.holdout_cases != HOLDOUT_CASE_COUNT
        || design.total_cases != CASE_COUNT
        || design.strata != STRATA
        || design.calibration_cases_per_stratum != 2
        || design.holdout_cases_per_stratum != 6
        || design.minimum_evidence_items_per_case != 2
        || design.case_order != "manifest_order"
        || design.matched_arms != ["control", "treatment"]
        || !design.shared_owner_draft
        || design.control_output != "exact_shared_owner_draft"
        || design.treatment_workflow
            != [
                "shared_owner_draft",
                "initial_verifier",
                "optional_single_owner_repair",
                "verifier_recheck_after_repair",
            ]
        || design.max_owner_repairs_per_case != 1
        || design.max_verifier_rechecks_per_case != 1
        || !design.owner_and_verifier_models_must_differ
        || design.oracle_predicate != "required_groups_and_forbidden_and_exact_json"
        || design.oracle_visibility != "evaluator_only_model_hidden"
        || design.tool_calls_allowed
    {
        return Err("delivery verification matched design is invalid".into());
    }
    Ok(())
}

fn validate_budget(budget: &ProtocolBudget) -> Result<(), String> {
    if budget.max_logical_model_calls_per_case != 4
        || budget.max_logical_model_calls_total != 128
        || budget.max_physical_model_attempts_total != 128
        || budget.transport_retries != 0
        || budget.max_bound_context_bytes != MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES
        || budget.max_owner_output_tokens != 4096
        || budget.max_verifier_output_tokens != 2048
        || budget.max_total_tokens_per_case != 64_000
        || budget.max_total_tokens_campaign != 2_048_000
        || budget.model_call_timeout_ms != 150_000
        || budget.case_timeout_ms != 600_000
        || budget.campaign_timeout_ms != 21_600_000
    {
        return Err("delivery verification budget is not the frozen 32-case budget".into());
    }
    Ok(())
}

fn validate_calibration_gate(gate: &CalibrationGate) -> Result<(), String> {
    if gate.complete_cases != CALIBRATION_CASE_COUNT
        || gate.minimum_control_failures != 2
        || gate.minimum_treatment_only_wins != 1
        || gate.maximum_control_only_losses != 0
        || gate.maximum_structural_failures != 0
        || gate.maximum_treatment_execution_failures != 0
        || !gate.calibration_results_excluded_from_holdout
        || gate.prompt_model_threshold_changes_after_calibration != "invalidate_protocol"
        || gate.action_on_failure != "terminal_futility"
    {
        return Err("delivery verification calibration gate is invalid".into());
    }
    Ok(())
}

fn validate_holdout_decision(decision: &HoldoutDecision) -> Result<(), String> {
    if decision.complete_cases != HOLDOUT_CASE_COUNT
        || decision.minimum_treatment_only_wins != 5
        || decision.maximum_control_only_losses != 0
        || decision.maximum_structural_failures != 0
        || decision.maximum_treatment_execution_failures != 0
        || decision.primary_test != "one_sided_exact_mcnemar"
        || decision.alpha_millionths != 50_000
        || decision.five_zero_p_millionths != 31_250
        || !decision.calibration_results_excluded
        || decision.incomplete_pairs != "inconclusive"
        || !decision.no_regression_required
    {
        return Err("delivery verification holdout decision rule is invalid".into());
    }
    Ok(())
}

fn validate_stop_contract(stop: &StopContract) -> Result<(), String> {
    if stop.calibration_gate_failure != "terminal_futility"
        || stop.any_structural_failure != "inconclusive"
        || stop.any_treatment_execution_failure != "inconclusive"
        || stop.any_holdout_control_only_loss != "regression"
        || stop.insufficient_holdout_wins != "no_evidence"
        || stop.budget_exhaustion != "inconclusive"
        || stop.case_replacement != "invalidate_protocol"
        || stop.frozen_attempt_rerun != "forbidden"
    {
        return Err("delivery verification stop contract is invalid".into());
    }
    Ok(())
}

fn validate_execution(execution: &ExecutionContract) -> Result<(), String> {
    if execution.runner_binary != DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME
        || execution.required_feature != "realworld-eval"
        || execution.execution_authorized
        || !execution.online_execution_requires_explicit_authorization
        || execution.provider_transport_retries != 0
        || !execution.failures_retained
        || execution.case_replacement_allowed
        || execution.gepa_changes_authorized
        || execution.learning_changes_authorized
        || execution.production_changes_authorized
    {
        return Err("delivery verification execution authority is invalid or over-broad".into());
    }
    Ok(())
}

#[derive(Serialize)]
struct CaseOrderDigestEntry<'a> {
    ordinal: usize,
    id: &'a str,
    case_sha256: &'a str,
}

fn case_order_digest(
    suite: &DeliveryVerificationSuite,
    case_sha256s: &[String],
) -> Result<String, String> {
    let entries = suite
        .cases
        .iter()
        .zip(case_sha256s)
        .map(|(case, case_sha256)| CaseOrderDigestEntry {
            ordinal: case.ordinal,
            id: &case.id,
            case_sha256,
        })
        .collect::<Vec<_>>();
    domain_hash_json(
        CASE_ORDER_DIGEST_DOMAIN,
        &entries,
        "delivery verification case order",
    )
}

#[derive(Serialize)]
struct HiddenOracleDigestEntry<'a> {
    case_id: &'a str,
    oracle: &'a ModelHiddenOracle,
}

fn hidden_oracle_digest(suite: &DeliveryVerificationSuite) -> Result<String, String> {
    let entries = suite
        .cases
        .iter()
        .map(|case| HiddenOracleDigestEntry {
            case_id: &case.id,
            oracle: &case.oracle,
        })
        .collect::<Vec<_>>();
    domain_hash_json(
        HIDDEN_ORACLE_DIGEST_DOMAIN,
        &entries,
        "delivery verification model-hidden oracle",
    )
}

fn domain_hash_json<T: Serialize>(domain: &[u8], value: &T, label: &str) -> Result<String, String> {
    let mut canonical = serde_json::to_value(value)
        .map_err(|error| format!("could not encode {label} for hashing: {error}"))?;
    canonicalize_json(&mut canonical);
    let encoded = serde_json::to_vec(&canonical)
        .map_err(|error| format!("could not encode canonical {label}: {error}"))?;
    let mut payload = Vec::with_capacity(domain.len() + encoded.len());
    payload.extend_from_slice(domain);
    payload.extend_from_slice(&encoded);
    Ok(sha256_hex(&payload))
}

fn canonicalize_json(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                canonicalize_json(value);
            }
        }
        Value::Object(values) => {
            let mut ordered = std::mem::take(values)
                .into_iter()
                .map(|(key, mut value)| {
                    canonicalize_json(&mut value);
                    (key, value)
                })
                .collect::<Vec<_>>();
            ordered.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            values.extend(ordered);
        }
        _ => {}
    }
}

fn validate_canonical_document(bytes: &[u8], maximum: usize, label: &str) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(format!("{label} is empty or exceeds its byte limit"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{label} must be UTF-8"))?;
    if !text.starts_with("{\n")
        || !text.ends_with("\n}\n")
        || text.ends_with("\n\n")
        || text.contains('\r')
        || text.contains('\t')
        || text.lines().any(|line| line.ends_with(' '))
    {
        return Err(format!("{label} is not canonical LF JSON"));
    }
    Ok(())
}

fn reject_raw_credentials(bytes: &[u8], label: &str) -> Result<(), String> {
    let lowered = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    for marker in [
        "ghp_",
        "github_pat_",
        "bearer ",
        "\"api_key\"",
        "\"apikey\"",
        "\"password\"",
        "\"authorization\"",
        "sk-proj-",
    ] {
        if lowered.contains(marker) {
            return Err(format!("{label} contains a raw credential marker"));
        }
    }
    Ok(())
}

fn validate_text(value: &str, maximum: usize, label: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} is empty, untrimmed, invalid, or too large"
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_lower_kebab(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.contains("--")
}

#[cfg(test)]
#[path = "delivery_verification_protocol_tests.rs"]
mod tests;
