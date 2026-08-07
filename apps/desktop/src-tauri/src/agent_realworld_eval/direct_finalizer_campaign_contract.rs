use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) const DIRECT_FINALIZER_GEPA_SUITE_SCHEMA: &str = "cindx.direct-finalizer-gepa-suite.v1";
pub(super) const DIRECT_FINALIZER_GEPA_SUITE_ID: &str = "cindx-direct-finalizer-gepa-v1";
pub(super) const DIRECT_FINALIZER_GEPA_SUITE_VERSION: u32 = 1;
pub(super) const DIRECT_FINALIZER_CAMPAIGN_RECEIPT_SCHEMA: &str =
    "cindx.direct-finalizer-gepa-campaign-receipt.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DirectFinalizerCaseSplit {
    Train,
    Holdout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerCampaignSuite {
    pub(super) schema: String,
    pub(super) id: String,
    pub(super) version: u32,
    pub(super) description: String,
    pub(super) gate_a_case_ids: Vec<String>,
    pub(super) cases: Vec<DirectFinalizerCampaignCase>,
}

impl DirectFinalizerCampaignSuite {
    pub(super) fn train_cases(&self) -> impl Iterator<Item = &DirectFinalizerCampaignCase> {
        self.cases
            .iter()
            .filter(|case| case.split == DirectFinalizerCaseSplit::Train)
    }

    pub(super) fn holdout_cases(&self) -> impl Iterator<Item = &DirectFinalizerCampaignCase> {
        self.cases
            .iter()
            .filter(|case| case.split == DirectFinalizerCaseSplit::Holdout)
    }

    pub(super) fn gate_a_cases(&self) -> Vec<&DirectFinalizerCampaignCase> {
        let by_id = self
            .cases
            .iter()
            .map(|case| (case.id.as_str(), case))
            .collect::<BTreeMap<_, _>>();
        self.gate_a_case_ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerCampaignCase {
    pub(super) id: String,
    pub(super) split: DirectFinalizerCaseSplit,
    pub(super) task_class: String,
    pub(super) objective: String,
    pub(super) evidence_summary: String,
    pub(super) actor_draft: String,
    #[serde(default)]
    pub(super) required_any_groups: Vec<Vec<String>>,
    #[serde(default)]
    pub(super) forbidden_terms: Vec<String>,
    #[serde(default)]
    pub(super) exact_json: Option<Value>,
}

pub(super) fn parse_and_validate_direct_finalizer_suite(
    encoded: &[u8],
) -> Result<DirectFinalizerCampaignSuite, String> {
    let suite = serde_json::from_slice::<DirectFinalizerCampaignSuite>(encoded)
        .map_err(|error| format!("invalid Direct-finalizer GEPA suite JSON: {error}"))?;
    validate_direct_finalizer_suite(&suite)?;
    Ok(suite)
}

fn validate_direct_finalizer_suite(suite: &DirectFinalizerCampaignSuite) -> Result<(), String> {
    if suite.schema != DIRECT_FINALIZER_GEPA_SUITE_SCHEMA {
        return Err(format!(
            "unsupported Direct-finalizer GEPA suite schema {}",
            suite.schema
        ));
    }
    if suite.id != DIRECT_FINALIZER_GEPA_SUITE_ID {
        return Err(format!(
            "unsupported Direct-finalizer GEPA suite id {}",
            suite.id
        ));
    }
    if suite.version != DIRECT_FINALIZER_GEPA_SUITE_VERSION {
        return Err(format!(
            "unsupported Direct-finalizer GEPA suite version {}",
            suite.version
        ));
    }
    if suite.description.trim().is_empty() {
        return Err("Direct-finalizer GEPA suite description is empty".to_string());
    }

    let mut case_ids = BTreeSet::new();
    let mut train_ids = BTreeSet::new();
    let mut holdout_ids = BTreeSet::new();
    let mut train_task_classes = BTreeSet::new();
    let mut holdout_task_classes = BTreeSet::new();
    let mut train_preservation_controls = 0usize;
    let mut holdout_preservation_controls = 0usize;
    for case in &suite.cases {
        validate_direct_finalizer_case(case)?;
        if !case_ids.insert(case.id.as_str()) {
            return Err(format!(
                "duplicate Direct-finalizer GEPA case id {}",
                case.id
            ));
        }
        match case.split {
            DirectFinalizerCaseSplit::Train => {
                train_ids.insert(case.id.as_str());
                train_task_classes.insert(case.task_class.as_str());
                train_preservation_controls +=
                    usize::from(verify_direct_finalizer_output(case, &case.actor_draft).passed);
            }
            DirectFinalizerCaseSplit::Holdout => {
                holdout_ids.insert(case.id.as_str());
                holdout_task_classes.insert(case.task_class.as_str());
                holdout_preservation_controls +=
                    usize::from(verify_direct_finalizer_output(case, &case.actor_draft).passed);
            }
        }
    }
    if train_ids.len() != 6 {
        return Err(format!(
            "Direct-finalizer GEPA suite requires exactly 6 train cases, found {}",
            train_ids.len()
        ));
    }
    if holdout_ids.len() != 8 {
        return Err(format!(
            "Direct-finalizer GEPA suite requires exactly 8 holdout cases, found {}",
            holdout_ids.len()
        ));
    }
    if !train_ids.is_disjoint(&holdout_ids) {
        return Err("Direct-finalizer train and holdout case ids overlap".to_string());
    }
    if train_task_classes.len() < 2 || holdout_task_classes.len() < 2 {
        return Err(
            "Direct-finalizer GEPA suite requires at least 2 task classes in each split"
                .to_string(),
        );
    }
    if !(2..=4).contains(&train_preservation_controls)
        || !(2..=6).contains(&holdout_preservation_controls)
    {
        return Err(
            "Direct-finalizer GEPA suite requires both preservation and correction cases in each split"
                .to_string(),
        );
    }

    let gate_a_ids = suite
        .gate_a_case_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if suite.gate_a_case_ids.len() != 4 || gate_a_ids.len() != 4 {
        return Err("Direct-finalizer GEPA suite requires 4 unique Gate A cases".to_string());
    }
    if let Some(case_id) = gate_a_ids
        .iter()
        .find(|case_id| !train_ids.contains(**case_id))
    {
        return Err(format!(
            "Direct-finalizer Gate A case {case_id} is not in the train split"
        ));
    }
    let gate_a_preservation_controls = suite
        .cases
        .iter()
        .filter(|case| gate_a_ids.contains(case.id.as_str()))
        .filter(|case| verify_direct_finalizer_output(case, &case.actor_draft).passed)
        .count();
    if !(1..=2).contains(&gate_a_preservation_controls) {
        return Err(
            "Direct-finalizer Gate A requires both preservation and correction cases".to_string(),
        );
    }
    Ok(())
}

fn validate_direct_finalizer_case(case: &DirectFinalizerCampaignCase) -> Result<(), String> {
    for (field, value) in [
        ("id", case.id.as_str()),
        ("task_class", case.task_class.as_str()),
        ("objective", case.objective.as_str()),
        ("evidence_summary", case.evidence_summary.as_str()),
        ("actor_draft", case.actor_draft.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(format!(
                "Direct-finalizer case {} has an empty {field}",
                display_case_id(case)
            ));
        }
    }
    for (group_index, group) in case.required_any_groups.iter().enumerate() {
        if group.is_empty() {
            return Err(format!(
                "Direct-finalizer case {} required group {group_index} is empty",
                case.id
            ));
        }
        if group.iter().any(|term| term.trim().is_empty()) {
            return Err(format!(
                "Direct-finalizer case {} required group {group_index} contains an empty term",
                case.id
            ));
        }
    }
    if case
        .forbidden_terms
        .iter()
        .any(|term| term.trim().is_empty())
    {
        return Err(format!(
            "Direct-finalizer case {} contains an empty forbidden term",
            case.id
        ));
    }
    if case.required_any_groups.is_empty()
        && case.forbidden_terms.is_empty()
        && case.exact_json.is_none()
    {
        return Err(format!(
            "Direct-finalizer case {} has no deterministic verification contract",
            case.id
        ));
    }
    Ok(())
}

fn display_case_id(case: &DirectFinalizerCampaignCase) -> &str {
    if case.id.trim().is_empty() {
        "<missing>"
    } else {
        case.id.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum DirectFinalizerVerificationFailure {
    RequiredGroupMissing { group_index: usize },
    ForbiddenTermPresent { term_index: usize },
    ExactJsonInvalid,
    ExactJsonMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerDeterministicVerification {
    pub(super) passed: bool,
    pub(super) required_groups_passed: usize,
    pub(super) required_groups_total: usize,
    pub(super) forbidden_terms_absent: usize,
    pub(super) forbidden_terms_total: usize,
    pub(super) exact_json_passed: Option<bool>,
    pub(super) failures: Vec<DirectFinalizerVerificationFailure>,
}

pub(super) fn verify_direct_finalizer_output(
    case: &DirectFinalizerCampaignCase,
    output: &str,
) -> DirectFinalizerDeterministicVerification {
    let normalized_output = output.to_lowercase();
    let mut failures = Vec::new();
    let mut required_groups_passed = 0;
    for (group_index, group) in case.required_any_groups.iter().enumerate() {
        if group
            .iter()
            .any(|term| normalized_output.contains(&term.to_lowercase()))
        {
            required_groups_passed += 1;
        } else {
            failures.push(DirectFinalizerVerificationFailure::RequiredGroupMissing { group_index });
        }
    }
    let mut forbidden_terms_absent = 0;
    for (term_index, term) in case.forbidden_terms.iter().enumerate() {
        if normalized_output.contains(&term.to_lowercase()) {
            failures.push(DirectFinalizerVerificationFailure::ForbiddenTermPresent { term_index });
        } else {
            forbidden_terms_absent += 1;
        }
    }
    let exact_json_passed = case.exact_json.as_ref().map(|expected| {
        match serde_json::from_str::<Value>(output.trim()) {
            Ok(actual) if actual == *expected => true,
            Ok(_) => {
                failures.push(DirectFinalizerVerificationFailure::ExactJsonMismatch);
                false
            }
            Err(_) => {
                failures.push(DirectFinalizerVerificationFailure::ExactJsonInvalid);
                false
            }
        }
    });
    DirectFinalizerDeterministicVerification {
        passed: failures.is_empty(),
        required_groups_passed,
        required_groups_total: case.required_any_groups.len(),
        forbidden_terms_absent,
        forbidden_terms_total: case.forbidden_terms.len(),
        exact_json_passed,
        failures,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerProviderReceipt {
    pub(super) configured_model: String,
    pub(super) request_payload_sha256: String,
    pub(super) response_semantic_sha256: String,
    pub(super) provider_response_model: Option<String>,
    pub(super) provider_response_id_sha256: Option<String>,
    pub(super) provider_system_fingerprint_sha256: Option<String>,
    pub(super) receipt_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerReviewerReceipt {
    pub(super) review_id: String,
    pub(super) reviewer_model: String,
    pub(super) latency_ms: u64,
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) provider_receipts: Vec<DirectFinalizerProviderReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerProfileReceipt {
    pub(super) profile_id: String,
    pub(super) profile_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerCallMetrics {
    pub(super) latency_ms: u64,
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerCandidateReceipt {
    pub(super) canonical_request_sha256: String,
    pub(super) directive_sha256: Option<String>,
    pub(super) output_sha256: String,
    pub(super) deterministic_score: f64,
    pub(super) provider_receipts: Vec<DirectFinalizerProviderReceipt>,
    pub(super) metrics: DirectFinalizerCallMetrics,
    pub(super) verification: DirectFinalizerDeterministicVerification,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerPairReceipt {
    pub(super) evaluation_id: String,
    pub(super) case_id: String,
    pub(super) split: DirectFinalizerCaseSplit,
    pub(super) task_class: String,
    pub(super) pre_treatment_state_sha256: String,
    pub(super) task_contract_sha256: String,
    pub(super) parent: DirectFinalizerCandidateReceipt,
    pub(super) candidate: DirectFinalizerCandidateReceipt,
    pub(super) reviewer_forward: DirectFinalizerReviewerReceipt,
    pub(super) reviewer_reverse: DirectFinalizerReviewerReceipt,
    pub(super) reviewer_score_parent: f64,
    pub(super) reviewer_score_candidate: f64,
    pub(super) reviewer_safety_violations_parent: u64,
    pub(super) reviewer_safety_violations_candidate: u64,
    pub(super) reward_parent: f64,
    pub(super) reward_candidate: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerGateReceipt {
    pub(super) protocol: String,
    pub(super) status: String,
    pub(super) passed: bool,
    pub(super) evaluated_case_ids: Vec<String>,
    pub(super) blocker_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerCampaignModels {
    pub(super) producer_model: String,
    pub(super) reviewer_model: String,
    pub(super) gepa_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerGepaReceipt {
    pub(super) attempts: u32,
    pub(super) response_sha256: Option<String>,
    pub(super) decision: Option<orchestrator::DirectFinalizerGepaDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerPromotionReceipt {
    pub(super) protocol: String,
    pub(super) eligible: bool,
    pub(super) blocker_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct DirectFinalizerCampaignReceipt {
    pub(super) schema: String,
    pub(super) suite_id: String,
    pub(super) suite_version: u32,
    pub(super) suite_sha256: String,
    pub(super) dataset_sha256: String,
    pub(super) cohort_sha256: String,
    pub(super) source_commit: String,
    pub(super) provider_id: String,
    pub(super) models: DirectFinalizerCampaignModels,
    pub(super) parent_profile: DirectFinalizerProfileReceipt,
    pub(super) candidate_profile: Option<DirectFinalizerProfileReceipt>,
    pub(super) call_count: u64,
    pub(super) gate_a: DirectFinalizerGateReceipt,
    pub(super) gepa: DirectFinalizerGepaReceipt,
    pub(super) promotion: Option<DirectFinalizerPromotionReceipt>,
    pub(super) paired_evidence_sha256: Option<String>,
    pub(super) snapshot_artifact_sha256: Option<String>,
    pub(super) pairs: Vec<DirectFinalizerPairReceipt>,
}

pub(super) fn write_sanitized_direct_finalizer_campaign(
    output_path: &Path,
    receipt: &DirectFinalizerCampaignReceipt,
) -> Result<(), String> {
    if !output_path.is_absolute() {
        return Err("Direct-finalizer campaign receipt path must be absolute".to_string());
    }
    if receipt.schema != DIRECT_FINALIZER_CAMPAIGN_RECEIPT_SCHEMA {
        return Err(format!(
            "unsupported Direct-finalizer campaign receipt schema {}",
            receipt.schema
        ));
    }
    if receipt.gate_a.status != "not_run" && receipt.candidate_profile.is_none() {
        return Err(
            "Direct-finalizer campaign receipt with Gate A evidence requires a candidate profile"
                .to_string(),
        );
    }
    let encoded = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("failed to encode Direct-finalizer campaign receipt: {error}"))?;
    tools::write_private_file_atomically(output_path, &encoded).map_err(|error| {
        format!(
            "failed to write Direct-finalizer campaign receipt {}: {error}",
            output_path.display()
        )
    })
}

#[cfg(test)]
#[path = "direct_finalizer_campaign_contract_tests.rs"]
mod tests;
