use super::direct_finalizer_campaign::DirectFinalizerCampaignPairEvidence;
use super::direct_finalizer_campaign_contract::{
    DirectFinalizerCampaignCase, DirectFinalizerCampaignSuite, DirectFinalizerCandidateReceipt,
    DirectFinalizerCaseSplit, DirectFinalizerProfileReceipt,
};
use orchestrator::{
    evaluate_prompt_promotion_gate_in_cohort, sha256_hex, AgentEvaluationCheck,
    AgentEvaluationEvidenceSource, AgentEvaluationReflectionPacket, AgentEvaluationVerifierOutcome,
    PromptEvaluationMode, PromptEvaluationProvenance, PromptEvaluationSplit,
    PromptEvolutionObservation, PromptMatchedEvaluationIdentityV1, PromptPromotionGateConfig,
    PromptPromotionGateResult, PROMPT_MATCHED_EVALUATION_SCHEMA_V1,
};
use std::collections::{BTreeMap, BTreeSet};

const PAIRED_EVIDENCE_SCHEMA: &str = "cindx.direct-finalizer-paired-evidence.v1";

pub(super) struct DirectFinalizerEvidenceProjection {
    #[cfg(test)]
    pub(super) observations: Vec<PromptEvolutionObservation>,
    pub(super) gate: PromptPromotionGateResult,
    pub(super) paired_evidence_sha256: String,
}

pub(super) fn reflection_packets(
    suite: &DirectFinalizerCampaignSuite,
    candidate_profile: &DirectFinalizerProfileReceipt,
    pairs: &[DirectFinalizerCampaignPairEvidence],
    producer_model_sha256: &str,
) -> Result<Vec<AgentEvaluationReflectionPacket>, String> {
    if !profile_valid(candidate_profile) || !is_sha256(producer_model_sha256) {
        return Err("Direct-finalizer reflection identity is malformed".to_string());
    }
    let selected = sorted_pairs(suite, pairs, Some(DirectFinalizerCaseSplit::Train))?;
    if selected.len() != 6 {
        return Err(format!(
            "Direct-finalizer reflection requires exactly 6 train pairs, found {}",
            selected.len()
        ));
    }
    selected
        .into_iter()
        .enumerate()
        .map(|(seed, (case, pair))| {
            validate_pair(case, pair)?;
            Ok(candidate_reflection_packet(
                suite,
                candidate_profile,
                case,
                pair,
                producer_model_sha256,
                seed as u64,
            ))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn evaluate_campaign_evidence(
    suite: &DirectFinalizerCampaignSuite,
    parent_profile: &DirectFinalizerProfileReceipt,
    candidate_profile: &DirectFinalizerProfileReceipt,
    dataset_sha256: &str,
    cohort_sha256: &str,
    producer_model: &str,
    reviewer_model: &str,
    pairs: &[DirectFinalizerCampaignPairEvidence],
    gate_config: PromptPromotionGateConfig,
) -> Result<DirectFinalizerEvidenceProjection, String> {
    if !profile_valid(parent_profile)
        || !profile_valid(candidate_profile)
        || parent_profile.profile_id == candidate_profile.profile_id
        || !is_sha256(dataset_sha256)
        || !is_sha256(cohort_sha256)
        || producer_model.trim().is_empty()
        || reviewer_model.trim().is_empty()
        || producer_model.trim() == reviewer_model.trim()
    {
        return Err("Direct-finalizer evidence identity is malformed".to_string());
    }
    let selected = sorted_pairs(suite, pairs, None)?;
    let train = selected
        .iter()
        .filter(|(case, _)| case.split == DirectFinalizerCaseSplit::Train)
        .count();
    if train != 6 || selected.len().saturating_sub(train) != 8 {
        return Err("Direct-finalizer promotion requires 6 train and 8 holdout pairs".into());
    }
    let producer_sha256 = sha256_hex(producer_model.trim().as_bytes());
    let reviewer_sha256 = sha256_hex(reviewer_model.trim().as_bytes());
    let mut observations = Vec::with_capacity(28);
    for (case, pair) in &selected {
        validate_pair(case, pair)?;
        if pair.receipt.reviewer_forward.reviewer_model != reviewer_model
            || pair.receipt.reviewer_reverse.reviewer_model != reviewer_model
        {
            return Err(format!(
                "Direct-finalizer pair {} reviewer identity does not match the frozen cohort",
                case.id
            ));
        }
        observations.extend(project_pair(
            parent_profile,
            candidate_profile,
            dataset_sha256,
            cohort_sha256,
            &producer_sha256,
            &reviewer_sha256,
            case,
            pair,
        ));
    }
    let gate = evaluate_prompt_promotion_gate_in_cohort(
        &observations,
        &candidate_profile.profile_id,
        &parent_profile.profile_id,
        cohort_sha256,
        gate_config,
    );
    let paired_evidence_sha256 = paired_evidence_sha256(cohort_sha256, &selected, &observations)?;
    Ok(DirectFinalizerEvidenceProjection {
        #[cfg(test)]
        observations,
        gate,
        paired_evidence_sha256,
    })
}

fn candidate_reflection_packet(
    suite: &DirectFinalizerCampaignSuite,
    profile: &DirectFinalizerProfileReceipt,
    case: &DirectFinalizerCampaignCase,
    pair: &DirectFinalizerCampaignPairEvidence,
    model_sha256: &str,
    seed: u64,
) -> AgentEvaluationReflectionPacket {
    let receipt = &pair.receipt.candidate;
    let parent_quality = combined_quality(
        pair.receipt.parent.deterministic_score,
        pair.receipt.reviewer_score_parent,
    );
    let candidate_quality = combined_quality(
        receipt.deterministic_score,
        pair.receipt.reviewer_score_candidate,
    );
    let matched_delta = format!(
        "Matched blinded comparison: relative_reward={:.3}; quality={:.3} versus {:.3}; deterministic_contract={} versus {}; safety_violations={} versus {}.",
        pair.receipt.reward_candidate,
        candidate_quality,
        parent_quality,
        receipt.verification.passed,
        pair.receipt.parent.verification.passed,
        pair.receipt.reviewer_safety_violations_candidate,
        pair.receipt.reviewer_safety_violations_parent,
    );
    let mut actionable_feedback = pair.candidate_feedback.clone();
    actionable_feedback.summary = if actionable_feedback.summary.trim().is_empty() {
        matched_delta
    } else {
        format!("{matched_delta}\n{}", actionable_feedback.summary)
    };
    AgentEvaluationReflectionPacket {
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        case_id: case.id.clone(),
        category: case.task_class.clone(),
        run_id: pair.receipt.evaluation_id.clone(),
        seed,
        candidate_id: profile.profile_id.clone(),
        candidate_fingerprint: profile.profile_sha256.clone(),
        model_fingerprints: BTreeMap::from([("finalizer".to_string(), model_sha256.to_string())]),
        input: format!(
            "Objective:\n{}\n\nFrozen evidence:\n{}",
            case.objective, case.evidence_summary
        ),
        steps: Vec::new(),
        final_output: pair.candidate_output.clone(),
        verifier: deterministic_verifier(receipt),
        actionable_feedback,
    }
}

fn deterministic_verifier(
    receipt: &DirectFinalizerCandidateReceipt,
) -> AgentEvaluationVerifierOutcome {
    let passed = receipt.verification.passed;
    AgentEvaluationVerifierOutcome {
        source: AgentEvaluationEvidenceSource::Deterministic,
        passed,
        score: receipt.deterministic_score,
        checks: vec![AgentEvaluationCheck {
            id: "direct_finalizer_contract".to_string(),
            passed,
            detail: format!(
                "Direct-finalizer deterministic contract {}",
                if passed { "passed" } else { "failed" }
            ),
        }],
    }
}

#[allow(clippy::too_many_arguments)]
fn project_pair(
    parent_profile: &DirectFinalizerProfileReceipt,
    candidate_profile: &DirectFinalizerProfileReceipt,
    dataset_sha256: &str,
    cohort_sha256: &str,
    producer_sha256: &str,
    reviewer_sha256: &str,
    case: &DirectFinalizerCampaignCase,
    pair: &DirectFinalizerCampaignPairEvidence,
) -> [PromptEvolutionObservation; 2] {
    let (split, mode) = prompt_split_and_mode(case.split);
    let identity = PromptMatchedEvaluationIdentityV1 {
        schema: PROMPT_MATCHED_EVALUATION_SCHEMA_V1.to_string(),
        evaluation_id: pair.receipt.evaluation_id.clone(),
        cohort_sha256: cohort_sha256.to_string(),
        dataset_sha256: dataset_sha256.to_string(),
        case_id: case.id.clone(),
        objective_sha256: sha256_hex(case.objective.trim().as_bytes()),
        split,
        mode,
    };
    [
        observation(
            candidate_profile,
            parent_profile,
            dataset_sha256,
            producer_sha256,
            reviewer_sha256,
            case,
            pair,
            &pair.receipt.candidate,
            identity.clone(),
            pair.receipt.reviewer_score_candidate,
            pair.receipt.reward_candidate,
            pair.receipt.reviewer_safety_violations_candidate,
        ),
        observation(
            parent_profile,
            candidate_profile,
            dataset_sha256,
            producer_sha256,
            reviewer_sha256,
            case,
            pair,
            &pair.receipt.parent,
            identity,
            pair.receipt.reviewer_score_parent,
            pair.receipt.reward_parent,
            pair.receipt.reviewer_safety_violations_parent,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn observation(
    profile: &DirectFinalizerProfileReceipt,
    opponent: &DirectFinalizerProfileReceipt,
    dataset_sha256: &str,
    producer_sha256: &str,
    reviewer_sha256: &str,
    case: &DirectFinalizerCampaignCase,
    pair: &DirectFinalizerCampaignPairEvidence,
    receipt: &DirectFinalizerCandidateReceipt,
    identity: PromptMatchedEvaluationIdentityV1,
    reviewer_score: f64,
    relative_reward: f64,
    safety_violations: u64,
) -> PromptEvolutionObservation {
    let quality_score = combined_quality(receipt.deterministic_score, reviewer_score);
    let succeeded = receipt.verification.passed && safety_violations == 0;
    let provenance = PromptEvaluationProvenance::blind_pairwise_swap(
        vec![reviewer_sha256.to_string()],
        vec![producer_sha256.to_string()],
        dataset_sha256,
        &profile.profile_sha256,
        &opponent.profile_sha256,
    )
    .with_matched_evaluation(identity.clone());
    PromptEvolutionObservation {
        profile_id: profile.profile_id.clone(),
        evaluation_id: pair.receipt.evaluation_id.clone(),
        case_id: case.id.clone(),
        opponent_profile_id: Some(opponent.profile_id.clone()),
        task_class: case.task_class.clone(),
        split: identity.split,
        mode: identity.mode,
        format_valid: true,
        succeeded,
        quality_score,
        latency_ms: receipt.metrics.latency_ms,
        total_tokens: receipt.metrics.total_tokens,
        estimated_cost_microusd: 0,
        safety_violations,
        relative_reward: Some(relative_reward),
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance,
    }
}

fn validate_pair(
    case: &DirectFinalizerCampaignCase,
    pair: &DirectFinalizerCampaignPairEvidence,
) -> Result<(), String> {
    let receipt = &pair.receipt;
    let valid_metric = |value: f64, range: std::ops::RangeInclusive<f64>| {
        value.is_finite() && range.contains(&value)
    };
    if receipt.case_id != case.id
        || receipt.split != case.split
        || receipt.task_class != case.task_class
        || receipt.evaluation_id.trim().is_empty()
        || pair.parent_output.trim().is_empty()
        || pair.candidate_output.trim().is_empty()
        || receipt.parent.output_sha256 != sha256_hex(pair.parent_output.as_bytes())
        || receipt.candidate.output_sha256 != sha256_hex(pair.candidate_output.as_bytes())
        || !valid_metric(receipt.reviewer_score_parent, 0.0..=1.0)
        || !valid_metric(receipt.reviewer_score_candidate, 0.0..=1.0)
        || !valid_metric(receipt.reward_parent, -1.0..=1.0)
        || !valid_metric(receipt.reward_candidate, -1.0..=1.0)
        || (receipt.reward_parent + receipt.reward_candidate).abs() > 1e-9
        || !is_sha256(&receipt.pre_treatment_state_sha256)
        || !is_sha256(&receipt.task_contract_sha256)
        || receipt.parent.canonical_request_sha256 == receipt.candidate.canonical_request_sha256
        || receipt.parent.directive_sha256.is_some()
        || receipt
            .candidate
            .directive_sha256
            .as_deref()
            .is_none_or(|value| !is_sha256(value))
        || receipt.reviewer_forward.review_id != format!("{}-forward", receipt.evaluation_id)
        || receipt.reviewer_reverse.review_id != format!("{}-reverse", receipt.evaluation_id)
        || receipt.reviewer_forward.reviewer_model != receipt.reviewer_reverse.reviewer_model
        || !reviewer_receipt_valid(&receipt.reviewer_forward)
        || !reviewer_receipt_valid(&receipt.reviewer_reverse)
        || !candidate_receipt_valid(&receipt.parent)
        || !candidate_receipt_valid(&receipt.candidate)
    {
        return Err(format!(
            "Direct-finalizer pair {} is not projection-safe",
            case.id
        ));
    }
    let parent_quality = combined_quality(
        receipt.parent.deterministic_score,
        receipt.reviewer_score_parent,
    );
    let candidate_quality = combined_quality(
        receipt.candidate.deterministic_score,
        receipt.reviewer_score_candidate,
    );
    if (receipt.reward_parent - (parent_quality - candidate_quality).clamp(-1.0, 1.0)).abs() > 1e-9
        || (receipt.reward_candidate - (candidate_quality - parent_quality).clamp(-1.0, 1.0)).abs()
            > 1e-9
    {
        return Err(format!(
            "Direct-finalizer pair {} reward does not match its blinded evidence",
            case.id
        ));
    }
    Ok(())
}

fn candidate_receipt_valid(receipt: &DirectFinalizerCandidateReceipt) -> bool {
    receipt.deterministic_score.is_finite()
        && (0.0..=1.0).contains(&receipt.deterministic_score)
        && is_sha256(&receipt.canonical_request_sha256)
        && is_sha256(&receipt.output_sha256)
        && receipt.metrics.total_tokens
            == receipt
                .metrics
                .prompt_tokens
                .saturating_add(receipt.metrics.completion_tokens)
        && receipt.verification.passed == receipt.verification.failures.is_empty()
        && (receipt.deterministic_score - deterministic_score(&receipt.verification)).abs() <= 1e-9
        && !receipt.provider_receipts.is_empty()
        && receipt.provider_receipts.iter().all(provider_receipt_valid)
}

fn deterministic_score(
    verification: &super::direct_finalizer_campaign_contract::DirectFinalizerDeterministicVerification,
) -> f64 {
    let exact_total = usize::from(verification.exact_json_passed.is_some());
    let exact_passed = usize::from(verification.exact_json_passed == Some(true));
    let passed = verification
        .required_groups_passed
        .saturating_add(verification.forbidden_terms_absent)
        .saturating_add(exact_passed);
    let total = verification
        .required_groups_total
        .saturating_add(verification.forbidden_terms_total)
        .saturating_add(exact_total)
        .max(1);
    passed as f64 / total as f64
}

fn reviewer_receipt_valid(
    receipt: &super::direct_finalizer_campaign_contract::DirectFinalizerReviewerReceipt,
) -> bool {
    !receipt.review_id.trim().is_empty()
        && !receipt.reviewer_model.trim().is_empty()
        && receipt.total_tokens
            == receipt
                .prompt_tokens
                .saturating_add(receipt.completion_tokens)
        && !receipt.provider_receipts.is_empty()
        && receipt.provider_receipts.iter().all(|provider| {
            provider.configured_model == receipt.reviewer_model && provider_receipt_valid(provider)
        })
}

fn provider_receipt_valid(
    receipt: &super::direct_finalizer_campaign_contract::DirectFinalizerProviderReceipt,
) -> bool {
    !receipt.configured_model.trim().is_empty()
        && is_sha256(&receipt.request_payload_sha256)
        && is_sha256(&receipt.response_semantic_sha256)
        && receipt
            .provider_response_id_sha256
            .as_deref()
            .is_none_or(is_sha256)
        && receipt
            .provider_system_fingerprint_sha256
            .as_deref()
            .is_none_or(is_sha256)
        && match receipt.receipt_status.as_str() {
            "observed" => receipt.provider_response_id_sha256.is_some(),
            "provider_id_missing" => receipt.provider_response_id_sha256.is_none(),
            _ => false,
        }
}

fn sorted_pairs<'a>(
    suite: &'a DirectFinalizerCampaignSuite,
    pairs: &'a [DirectFinalizerCampaignPairEvidence],
    required_split: Option<DirectFinalizerCaseSplit>,
) -> Result<
    Vec<(
        &'a DirectFinalizerCampaignCase,
        &'a DirectFinalizerCampaignPairEvidence,
    )>,
    String,
> {
    let by_case = suite
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let mut selected = pairs
        .iter()
        .filter(|pair| required_split.is_none_or(|split| pair.receipt.split == split))
        .map(|pair| {
            by_case
                .get(pair.receipt.case_id.as_str())
                .copied()
                .map(|case| (case, pair))
                .ok_or_else(|| "Direct-finalizer pair case is absent from suite".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    selected.sort_by(|left, right| left.0.id.cmp(&right.0.id));
    let mut case_ids = BTreeSet::new();
    let mut evaluation_ids = BTreeSet::new();
    if selected.iter().any(|(case, pair)| {
        !case_ids.insert(case.id.as_str())
            || !evaluation_ids.insert(pair.receipt.evaluation_id.as_str())
    }) {
        return Err("Direct-finalizer evidence contains duplicate pair identity".to_string());
    }
    Ok(selected)
}

fn combined_quality(deterministic_score: f64, reviewer_score: f64) -> f64 {
    0.6 * deterministic_score + 0.4 * reviewer_score
}

fn prompt_split_and_mode(
    split: DirectFinalizerCaseSplit,
) -> (PromptEvaluationSplit, PromptEvaluationMode) {
    match split {
        DirectFinalizerCaseSplit::Train => (
            PromptEvaluationSplit::Train,
            PromptEvaluationMode::PairedExecution,
        ),
        DirectFinalizerCaseSplit::Holdout => (
            PromptEvaluationSplit::Holdout,
            PromptEvaluationMode::ReplayExecution,
        ),
    }
}

fn profile_valid(profile: &DirectFinalizerProfileReceipt) -> bool {
    !profile.profile_id.trim().is_empty() && is_sha256(&profile.profile_sha256)
}

fn paired_evidence_sha256(
    cohort_sha256: &str,
    pairs: &[(
        &DirectFinalizerCampaignCase,
        &DirectFinalizerCampaignPairEvidence,
    )],
    observations: &[PromptEvolutionObservation],
) -> Result<String, String> {
    serde_json::to_vec(&(PAIRED_EVIDENCE_SCHEMA, cohort_sha256, pairs, observations))
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("Direct-finalizer evidence serialization failed: {error}"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
#[path = "direct_finalizer_campaign_evidence_tests.rs"]
mod tests;
