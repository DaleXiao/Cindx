use crate::{
    prompt_promotion_confidence_from_relative_rewards, PromptEvaluationMode, PromptEvaluationSplit,
    PromptEvolutionObservation, PromptPromotionConfidence,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PromptPromotionGateConfig {
    pub minimum_train_runs: usize,
    pub minimum_holdout_runs: usize,
    pub minimum_unique_train_cases: usize,
    pub minimum_unique_holdout_cases: usize,
    pub minimum_train_task_classes: usize,
    pub minimum_holdout_task_classes: usize,
    pub minimum_wilson_lower_bound: f64,
    pub maximum_generalization_gap: f64,
    pub maximum_holdout_task_class_regression: f64,
    pub maximum_holdout_quality_regression: f64,
    pub maximum_holdout_latency_regression_bps: u64,
    pub maximum_holdout_token_regression_bps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromptPromotionBlocker {
    IncompletePairedEvidence,
    InvalidEvidenceShape,
    InvalidFormat,
    SafetyViolation,
    InsufficientTrainRuns,
    InsufficientHoldoutRuns,
    InsufficientTrainCaseDiversity,
    InsufficientHoldoutCaseDiversity,
    InsufficientTrainTaskClassDiversity,
    InsufficientHoldoutTaskClassDiversity,
    WeakConfidence,
    GeneralizationGap,
    HoldoutTaskClassRegression,
    HoldoutFailureRegression,
    HoldoutQualityRegression,
    HoldoutLatencyRegression,
    HoldoutTokenRegression,
}

impl PromptPromotionBlocker {
    pub fn label(self) -> &'static str {
        match self {
            Self::IncompletePairedEvidence => "incomplete_paired_evidence",
            Self::InvalidEvidenceShape => "invalid_evidence_shape",
            Self::InvalidFormat => "invalid_format",
            Self::SafetyViolation => "safety_violation",
            Self::InsufficientTrainRuns => "insufficient_train_runs",
            Self::InsufficientHoldoutRuns => "insufficient_holdout_runs",
            Self::InsufficientTrainCaseDiversity => "insufficient_train_case_diversity",
            Self::InsufficientHoldoutCaseDiversity => "insufficient_holdout_case_diversity",
            Self::InsufficientTrainTaskClassDiversity => "insufficient_train_task_class_diversity",
            Self::InsufficientHoldoutTaskClassDiversity => {
                "insufficient_holdout_task_class_diversity"
            }
            Self::WeakConfidence => "weak_confidence",
            Self::GeneralizationGap => "generalization_gap",
            Self::HoldoutTaskClassRegression => "holdout_task_class_regression",
            Self::HoldoutFailureRegression => "holdout_failure_regression",
            Self::HoldoutQualityRegression => "holdout_quality_regression",
            Self::HoldoutLatencyRegression => "holdout_latency_regression",
            Self::HoldoutTokenRegression => "holdout_token_regression",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPromotionFailurePenalty {
    pub evaluation_id: String,
    pub cohort_sha256: String,
    pub case_id: String,
    pub task_class: String,
    pub split: PromptEvaluationSplit,
    pub candidate_profile_id: String,
    pub stable_profile_id: String,
    pub candidate_failed: bool,
    pub stable_failed: bool,
}

impl PromptPromotionFailurePenalty {
    pub fn validate(&self) -> Result<(), String> {
        let valid_sha256 = self.cohort_sha256.len() == 64
            && self
                .cohort_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if self.evaluation_id.trim().is_empty()
            || self.evaluation_id.len() > 512
            || self.case_id.trim().is_empty()
            || self.case_id.len() > 256
            || self.task_class.trim().is_empty()
            || self.task_class.len() > 256
            || self.candidate_profile_id.trim().is_empty()
            || self.stable_profile_id.trim().is_empty()
            || self.candidate_profile_id == self.stable_profile_id
            || (!self.candidate_failed && !self.stable_failed)
            || !valid_sha256
        {
            return Err("prompt promotion failure penalty is malformed".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptPromotionGateResult {
    pub eligible: bool,
    pub train_runs: usize,
    pub holdout_runs: usize,
    pub unique_train_cases: usize,
    pub unique_holdout_cases: usize,
    pub train_task_classes: usize,
    pub holdout_task_classes: usize,
    pub train_average_reward: f64,
    pub holdout_average_reward: f64,
    pub confidence: PromptPromotionConfidence,
    pub blockers: Vec<PromptPromotionBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PairKey {
    evaluation_id: String,
    case_id: String,
    split: PromptEvaluationSplit,
    mode: PromptEvaluationMode,
    cohort_sha256: String,
}

impl PairKey {
    pub(crate) fn from_observation(observation: &PromptEvolutionObservation) -> Option<Self> {
        let evaluation_id = observation.evaluation_id.trim();
        let case_id = observation.case_id.trim();
        if evaluation_id.is_empty() || case_id.is_empty() {
            return None;
        }
        Some(Self {
            evaluation_id: evaluation_id.to_string(),
            case_id: case_id.to_string(),
            split: observation.split,
            mode: observation.mode,
            cohort_sha256: observation.scientific_cohort_sha256()?.to_string(),
        })
    }

    fn from_failure(penalty: &PromptPromotionFailurePenalty) -> Self {
        Self {
            evaluation_id: penalty.evaluation_id.clone(),
            case_id: penalty.case_id.clone(),
            split: penalty.split,
            mode: match penalty.split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
            },
            cohort_sha256: penalty.cohort_sha256.clone(),
        }
    }
}

pub fn evaluate_prompt_promotion_gate(
    observations: &[PromptEvolutionObservation],
    candidate_id: &str,
    stable_id: &str,
    config: PromptPromotionGateConfig,
) -> PromptPromotionGateResult {
    evaluate_prompt_pair_gate(
        observations,
        candidate_id,
        stable_id,
        config,
        None,
        &[],
        PromptEvolutionObservation::is_scientific_evidence,
        |_, _| true,
    )
}

pub fn evaluate_prompt_promotion_gate_in_cohort(
    observations: &[PromptEvolutionObservation],
    candidate_id: &str,
    stable_id: &str,
    cohort_sha256: &str,
    config: PromptPromotionGateConfig,
) -> PromptPromotionGateResult {
    evaluate_prompt_pair_gate(
        observations,
        candidate_id,
        stable_id,
        config,
        Some(cohort_sha256),
        &[],
        PromptEvolutionObservation::is_scientific_evidence,
        |_, _| true,
    )
}

pub fn evaluate_prompt_promotion_gate_with_failures_in_cohort(
    observations: &[PromptEvolutionObservation],
    failures: &[PromptPromotionFailurePenalty],
    candidate_id: &str,
    stable_id: &str,
    cohort_sha256: &str,
    config: PromptPromotionGateConfig,
) -> PromptPromotionGateResult {
    evaluate_prompt_pair_gate(
        observations,
        candidate_id,
        stable_id,
        config,
        Some(cohort_sha256),
        failures,
        PromptEvolutionObservation::is_scientific_evidence,
        |_, _| true,
    )
}

pub fn evaluate_prompt_auto_transfer_gate(
    observations: &[PromptEvolutionObservation],
    candidate_id: &str,
    auto_profile_id: &str,
    auto_profile_sha256: &str,
    config: PromptPromotionGateConfig,
) -> PromptPromotionGateResult {
    evaluate_prompt_pair_gate(
        observations,
        candidate_id,
        auto_profile_id,
        config,
        None,
        &[],
        |observation| {
            observation.is_source_attested_transfer_evidence()
                && observation
                    .provenance
                    .transfer
                    .as_ref()
                    .is_some_and(|transfer| {
                        transfer.source_profile_id == auto_profile_id
                            && transfer.source_profile_sha256 == auto_profile_sha256
                    })
        },
        |candidate, auto| candidate.provenance.transfer == auto.provenance.transfer,
    )
}

pub fn evaluate_prompt_auto_transfer_gate_in_cohort_with_failures(
    observations: &[PromptEvolutionObservation],
    failures: &[PromptPromotionFailurePenalty],
    candidate_id: &str,
    auto_profile_id: &str,
    auto_profile_sha256: &str,
    cohort_sha256: &str,
    config: PromptPromotionGateConfig,
) -> PromptPromotionGateResult {
    evaluate_prompt_pair_gate(
        observations,
        candidate_id,
        auto_profile_id,
        config,
        Some(cohort_sha256),
        failures,
        |observation| {
            observation.is_strict_source_attested_transfer_evidence()
                && observation
                    .provenance
                    .transfer
                    .as_ref()
                    .is_some_and(|transfer| {
                        transfer.source_profile_id == auto_profile_id
                            && transfer.source_profile_sha256 == auto_profile_sha256
                    })
        },
        |candidate, auto| candidate.provenance.transfer == auto.provenance.transfer,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_prompt_pair_gate<Accept, ValidatePair>(
    observations: &[PromptEvolutionObservation],
    candidate_id: &str,
    stable_id: &str,
    config: PromptPromotionGateConfig,
    required_cohort_sha256: Option<&str>,
    failure_penalties: &[PromptPromotionFailurePenalty],
    accept: Accept,
    validate_pair: ValidatePair,
) -> PromptPromotionGateResult
where
    Accept: Fn(&PromptEvolutionObservation) -> bool,
    ValidatePair: Fn(&PromptEvolutionObservation, &PromptEvolutionObservation) -> bool,
{
    let mut blockers = BTreeSet::new();
    let mut replay_payloads = BTreeMap::<(String, String), &PromptEvolutionObservation>::new();
    let mut conflicting_replays = BTreeSet::<(String, String)>::new();
    for observation in observations.iter().filter(|observation| {
        accept(observation)
            && matches!(
                observation.profile_id.as_str(),
                profile if profile == candidate_id || profile == stable_id
            )
    }) {
        let key = (
            observation.profile_id.clone(),
            observation.evaluation_id.clone(),
        );
        if replay_payloads
            .get(&key)
            .is_some_and(|existing| **existing != *observation)
        {
            conflicting_replays.insert(key);
        } else {
            replay_payloads.entry(key).or_insert(observation);
        }
    }
    if !conflicting_replays.is_empty() {
        blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
    }
    let mut candidate_by_pair = BTreeMap::new();
    let mut stable_by_pair = BTreeMap::new();
    let mut candidate_seen = BTreeSet::new();
    let mut stable_seen = BTreeSet::new();
    let active_cohort_sha256 = required_cohort_sha256.or_else(|| {
        observations
            .iter()
            .rev()
            .filter(|observation| accept(observation))
            .filter(|observation| {
                !conflicting_replays.contains(&(
                    observation.profile_id.clone(),
                    observation.evaluation_id.clone(),
                ))
            })
            .find_map(PromptEvolutionObservation::scientific_cohort_sha256)
    });

    for observation in observations
        .iter()
        .filter(|observation| accept(observation))
        .filter(|observation| {
            !conflicting_replays.contains(&(
                observation.profile_id.clone(),
                observation.evaluation_id.clone(),
            ))
        })
        .filter(|observation| {
            active_cohort_sha256
                .is_some_and(|digest| observation.scientific_cohort_sha256() == Some(digest))
        })
    {
        let is_candidate = observation.profile_id == candidate_id
            && observation.opponent_profile_id.as_deref() == Some(stable_id);
        let is_stable = observation.profile_id == stable_id
            && observation.opponent_profile_id.as_deref() == Some(candidate_id);
        if !is_candidate && !is_stable {
            continue;
        }
        let Some(pair_key) = PairKey::from_observation(observation) else {
            blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
            continue;
        };
        let valid_shape = matches!(
            (observation.split, observation.mode),
            (
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution
            ) | (
                PromptEvaluationSplit::Holdout,
                PromptEvaluationMode::ReplayExecution
            )
        );
        if !valid_shape {
            blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
            continue;
        }
        if is_candidate && candidate_seen.insert(observation.evidence_identity()) {
            candidate_by_pair.insert(pair_key, observation);
        } else if is_stable && stable_seen.insert(observation.evidence_identity()) {
            stable_by_pair.insert(pair_key, observation);
        }
    }

    let candidate_keys = candidate_by_pair.keys().cloned().collect::<BTreeSet<_>>();
    let stable_keys = stable_by_pair.keys().cloned().collect::<BTreeSet<_>>();
    if candidate_keys != stable_keys {
        blockers.insert(PromptPromotionBlocker::IncompletePairedEvidence);
    }

    let complete_pairs = candidate_keys
        .intersection(&stable_keys)
        .filter_map(|key| {
            let candidate = candidate_by_pair.get(key).copied()?;
            let stable = stable_by_pair.get(key).copied()?;
            let mirrored_prompt_lineage = candidate.provenance.candidate_prompt_sha256
                == stable.provenance.opponent_prompt_sha256
                && candidate.provenance.opponent_prompt_sha256
                    == stable.provenance.candidate_prompt_sha256;
            let same_evaluator_protocol = candidate.provenance.protocol
                == stable.provenance.protocol
                && candidate.provenance.evaluator_models == stable.provenance.evaluator_models
                && candidate.provenance.participant_models == stable.provenance.participant_models;
            if !mirrored_prompt_lineage
                || !same_evaluator_protocol
                || !validate_pair(candidate, stable)
            {
                blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
                return None;
            }
            if !candidate.format_valid || !stable.format_valid {
                blockers.insert(PromptPromotionBlocker::InvalidFormat);
            }
            if candidate.safety_violations > 0 || stable.safety_violations > 0 {
                blockers.insert(PromptPromotionBlocker::SafetyViolation);
            }
            Some((key.clone(), candidate, stable))
        })
        .collect::<Vec<_>>();

    let mut failure_by_pair = BTreeMap::<PairKey, &PromptPromotionFailurePenalty>::new();
    for failure in failure_penalties.iter().filter(|failure| {
        active_cohort_sha256 == Some(failure.cohort_sha256.as_str())
            && failure.candidate_profile_id == candidate_id
            && failure.stable_profile_id == stable_id
    }) {
        if failure.validate().is_err() {
            blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
            continue;
        }
        let key = PairKey::from_failure(failure);
        if failure_by_pair
            .insert(key, failure)
            .is_some_and(|existing| existing != failure)
        {
            blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
        }
    }

    let complete_pairs = complete_pairs
        .into_iter()
        .filter(|(key, _, _)| !failure_by_pair.contains_key(key))
        .collect::<Vec<_>>();
    let complete_candidate = complete_pairs
        .iter()
        .map(|(_, candidate, _)| *candidate)
        .collect::<Vec<_>>();

    let train = complete_candidate
        .iter()
        .copied()
        .filter(|observation| observation.mode == PromptEvaluationMode::PairedExecution)
        .collect::<Vec<_>>();
    let holdout = complete_candidate
        .iter()
        .copied()
        .filter(|observation| observation.mode == PromptEvaluationMode::ReplayExecution)
        .collect::<Vec<_>>();
    let train_failures = failure_by_pair
        .values()
        .copied()
        .filter(|failure| failure.split == PromptEvaluationSplit::Train)
        .collect::<Vec<_>>();
    let holdout_failures = failure_by_pair
        .values()
        .copied()
        .filter(|failure| failure.split == PromptEvaluationSplit::Holdout)
        .collect::<Vec<_>>();
    let train_case_ids = train
        .iter()
        .map(|observation| observation.case_id.as_str())
        .collect::<BTreeSet<_>>();
    let holdout_case_ids = holdout
        .iter()
        .map(|observation| observation.case_id.as_str())
        .collect::<BTreeSet<_>>();
    if !train_case_ids.is_disjoint(&holdout_case_ids) {
        blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
    }
    let unique_train_cases = train
        .iter()
        .map(|observation| observation.case_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let unique_holdout_cases = holdout
        .iter()
        .map(|observation| observation.case_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let train_task_classes = train
        .iter()
        .map(|observation| observation.task_class.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let holdout_task_classes = holdout
        .iter()
        .map(|observation| observation.task_class.as_str())
        .collect::<BTreeSet<_>>()
        .len();

    let train_runs = train.len() + train_failures.len();
    let holdout_runs = holdout.len() + holdout_failures.len();
    if train_runs < config.minimum_train_runs {
        blockers.insert(PromptPromotionBlocker::InsufficientTrainRuns);
    }
    if holdout_runs < config.minimum_holdout_runs {
        blockers.insert(PromptPromotionBlocker::InsufficientHoldoutRuns);
    }
    if unique_train_cases < config.minimum_unique_train_cases {
        blockers.insert(PromptPromotionBlocker::InsufficientTrainCaseDiversity);
    }
    if unique_holdout_cases < config.minimum_unique_holdout_cases {
        blockers.insert(PromptPromotionBlocker::InsufficientHoldoutCaseDiversity);
    }
    if train_task_classes < config.minimum_train_task_classes {
        blockers.insert(PromptPromotionBlocker::InsufficientTrainTaskClassDiversity);
    }
    if holdout_task_classes < config.minimum_holdout_task_classes {
        blockers.insert(PromptPromotionBlocker::InsufficientHoldoutTaskClassDiversity);
    }

    let average_reward = |entries: &[&PromptEvolutionObservation], failures: usize| {
        entries.iter().map(|entry| entry.reward()).sum::<f64>()
            / (entries.len() + failures).max(1) as f64
    };
    let train_average_reward = average_reward(&train, train_failures.len());
    let holdout_average_reward = average_reward(&holdout, holdout_failures.len());
    if train_runs > 0
        && holdout_runs > 0
        && train_average_reward - holdout_average_reward > config.maximum_generalization_gap
    {
        blockers.insert(PromptPromotionBlocker::GeneralizationGap);
    }

    let mut holdout_classes = BTreeMap::<&str, Vec<f64>>::new();
    for observation in &holdout {
        holdout_classes
            .entry(observation.task_class.as_str())
            .or_default()
            .push(observation.relative_reward.unwrap_or_default());
    }
    for failure in &holdout_failures {
        holdout_classes
            .entry(failure.task_class.as_str())
            .or_default()
            .push(-1.0);
    }
    if holdout_classes.values().any(|rewards| {
        rewards.iter().sum::<f64>() / (rewards.len().max(1) as f64)
            < -config.maximum_holdout_task_class_regression
    }) {
        blockers.insert(PromptPromotionBlocker::HoldoutTaskClassRegression);
    }

    let holdout_pairs = complete_pairs
        .iter()
        .filter(|(_, candidate, _)| candidate.split == PromptEvaluationSplit::Holdout)
        .map(|(_, candidate, stable)| (*candidate, *stable))
        .collect::<Vec<_>>();
    if holdout_pairs
        .iter()
        .any(|(candidate, stable)| !candidate.succeeded && stable.succeeded)
        || holdout_failures
            .iter()
            .any(|failure| failure.candidate_failed && !failure.stable_failed)
    {
        blockers.insert(PromptPromotionBlocker::HoldoutFailureRegression);
    }
    if !holdout_pairs.is_empty() {
        let pair_count = holdout_pairs.len() as f64;
        let candidate_quality = holdout_pairs
            .iter()
            .map(|(candidate, _)| candidate.quality_score)
            .sum::<f64>()
            / pair_count;
        let stable_quality = holdout_pairs
            .iter()
            .map(|(_, stable)| stable.quality_score)
            .sum::<f64>()
            / pair_count;
        if candidate_quality + config.maximum_holdout_quality_regression < stable_quality {
            blockers.insert(PromptPromotionBlocker::HoldoutQualityRegression);
        }

        let candidate_latency = holdout_pairs
            .iter()
            .map(|(candidate, _)| u128::from(candidate.latency_ms))
            .sum::<u128>();
        let stable_latency = holdout_pairs
            .iter()
            .map(|(_, stable)| u128::from(stable.latency_ms))
            .sum::<u128>();
        if exceeds_bps_regression(
            candidate_latency,
            stable_latency,
            config.maximum_holdout_latency_regression_bps,
        ) {
            blockers.insert(PromptPromotionBlocker::HoldoutLatencyRegression);
        }

        let candidate_tokens = holdout_pairs
            .iter()
            .map(|(candidate, _)| u128::from(candidate.total_tokens))
            .sum::<u128>();
        let stable_tokens = holdout_pairs
            .iter()
            .map(|(_, stable)| u128::from(stable.total_tokens))
            .sum::<u128>();
        if exceeds_bps_regression(
            candidate_tokens,
            stable_tokens,
            config.maximum_holdout_token_regression_bps,
        ) {
            blockers.insert(PromptPromotionBlocker::HoldoutTokenRegression);
        }
    }

    let confidence = prompt_promotion_confidence_from_relative_rewards(
        holdout
            .iter()
            .map(|observation| observation.relative_reward.unwrap_or_default())
            .chain(holdout_failures.iter().map(|_| -1.0)),
    );
    if confidence.wilson_lower_bound < config.minimum_wilson_lower_bound {
        blockers.insert(PromptPromotionBlocker::WeakConfidence);
    }
    let blockers = blockers.into_iter().collect::<Vec<_>>();
    PromptPromotionGateResult {
        eligible: blockers.is_empty(),
        train_runs,
        holdout_runs,
        unique_train_cases,
        unique_holdout_cases,
        train_task_classes,
        holdout_task_classes,
        train_average_reward,
        holdout_average_reward,
        confidence,
        blockers,
    }
}

fn exceeds_bps_regression(candidate: u128, baseline: u128, tolerance_bps: u64) -> bool {
    if baseline == 0 {
        return candidate > 0;
    }
    candidate.saturating_mul(10_000) > baseline.saturating_mul(10_000 + u128::from(tolerance_bps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        profile_id: &str,
        opponent_id: &str,
        evaluation_id: &str,
        case_id: &str,
        task_class: &str,
        split: PromptEvaluationSplit,
        relative_reward: f64,
    ) -> PromptEvolutionObservation {
        PromptEvolutionObservation {
            profile_id: profile_id.to_string(),
            evaluation_id: evaluation_id.to_string(),
            case_id: case_id.to_string(),
            opponent_profile_id: Some(opponent_id.to_string()),
            task_class: task_class.to_string(),
            split,
            mode: match split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
            },
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(relative_reward),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: crate::PromptEvaluationProvenance::blind_pairwise_swap(
                vec!["independent-judge".to_string()],
                vec!["candidate-worker".to_string()],
                "d".repeat(64),
                crate::sha256_hex(profile_id.as_bytes()),
                crate::sha256_hex(opponent_id.as_bytes()),
            ),
        }
    }

    fn pair(
        evaluation_id: &str,
        case_id: &str,
        task_class: &str,
        split: PromptEvaluationSplit,
        candidate_reward: f64,
    ) -> [PromptEvolutionObservation; 2] {
        [
            observation(
                "candidate",
                "stable",
                evaluation_id,
                case_id,
                task_class,
                split,
                candidate_reward,
            ),
            observation(
                "stable",
                "candidate",
                evaluation_id,
                case_id,
                task_class,
                split,
                -candidate_reward,
            ),
        ]
    }

    fn config() -> PromptPromotionGateConfig {
        PromptPromotionGateConfig {
            minimum_train_runs: 2,
            minimum_holdout_runs: 2,
            minimum_unique_train_cases: 2,
            minimum_unique_holdout_cases: 2,
            minimum_train_task_classes: 2,
            minimum_holdout_task_classes: 2,
            minimum_wilson_lower_bound: 0.0,
            maximum_generalization_gap: 0.15,
            maximum_holdout_task_class_regression: 0.05,
            maximum_holdout_quality_regression: 0.01,
            maximum_holdout_latency_regression_bps: 500,
            maximum_holdout_token_regression_bps: 200,
        }
    }

    fn complete_evidence() -> Vec<PromptEvolutionObservation> {
        [
            pair(
                "train-1",
                "train-a",
                "coding",
                PromptEvaluationSplit::Train,
                0.3,
            ),
            pair(
                "train-2",
                "train-b",
                "research",
                PromptEvaluationSplit::Train,
                0.3,
            ),
            pair(
                "holdout-1",
                "holdout-a",
                "coding",
                PromptEvaluationSplit::Holdout,
                0.3,
            ),
            pair(
                "holdout-2",
                "holdout-b",
                "research",
                PromptEvaluationSplit::Holdout,
                0.3,
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn bind_matched_cohort(observations: &mut [PromptEvolutionObservation], cohort_sha256: &str) {
        for observation in observations {
            observation.provenance.matched_evaluation =
                Some(crate::PromptMatchedEvaluationIdentityV1 {
                    schema: crate::PROMPT_MATCHED_EVALUATION_SCHEMA_V1.to_string(),
                    evaluation_id: observation.evaluation_id.clone(),
                    cohort_sha256: cohort_sha256.to_string(),
                    dataset_sha256: observation.provenance.dataset_sha256.clone(),
                    case_id: observation.case_id.clone(),
                    objective_sha256: crate::sha256_hex(observation.case_id.as_bytes()),
                    split: observation.split,
                    mode: observation.mode,
                });
        }
    }

    fn transfer_pair(
        evaluation_id: &str,
        case_id: &str,
        task_class: &str,
        split: PromptEvaluationSplit,
        candidate_reward: f64,
    ) -> [PromptEvolutionObservation; 2] {
        let auto_profile_sha256 = crate::sha256_hex(b"auto-stable-genome");
        let transfer = crate::PromptTransferProvenance::auto_to_pro(
            format!("source-{evaluation_id}"),
            0,
            "auto-stable",
            auto_profile_sha256.clone(),
            crate::sha256_hex(format!("output-{case_id}").as_bytes()),
        )
        .with_source_context(&crate::AutoTeacherSourceContextV1 {
            schema: crate::AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            system_prompt_sha256: "3".repeat(64),
            policy_sha256: "4".repeat(64),
            budget_sha256: "5".repeat(64),
            tool_contract_sha256: "6".repeat(64),
            source_revision_sha256: "7".repeat(64),
            workspace_revision_sha256: "8".repeat(64),
            evaluator_identity_sha256: "9".repeat(64),
            evaluator_receipt_sha256: "a".repeat(64),
            checkpoint_sha256: "b".repeat(64),
            learning_receipt_sha256: "c".repeat(64),
        })
        .unwrap();
        let mut candidate = observation(
            "candidate",
            "auto-stable",
            evaluation_id,
            case_id,
            task_class,
            split,
            candidate_reward,
        );
        candidate.provenance = crate::PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["independent-judge".to_string()],
            vec!["candidate-worker".to_string(), "auto-worker".to_string()],
            "f".repeat(64),
            crate::sha256_hex(b"candidate"),
            auto_profile_sha256.clone(),
        )
        .with_transfer(transfer.clone());
        let mut auto = observation(
            "auto-stable",
            "candidate",
            evaluation_id,
            case_id,
            task_class,
            split,
            -candidate_reward,
        );
        auto.provenance = crate::PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["independent-judge".to_string()],
            vec!["candidate-worker".to_string(), "auto-worker".to_string()],
            "f".repeat(64),
            auto_profile_sha256,
            crate::sha256_hex(b"candidate"),
        )
        .with_transfer(transfer);
        [candidate, auto]
    }

    fn complete_transfer_evidence() -> Vec<PromptEvolutionObservation> {
        [
            transfer_pair(
                "transfer-train-1",
                "train-a",
                "coding",
                PromptEvaluationSplit::Train,
                0.3,
            ),
            transfer_pair(
                "transfer-train-2",
                "train-b",
                "research",
                PromptEvaluationSplit::Train,
                0.3,
            ),
            transfer_pair(
                "transfer-holdout-1",
                "holdout-a",
                "coding",
                PromptEvaluationSplit::Holdout,
                0.3,
            ),
            transfer_pair(
                "transfer-holdout-2",
                "holdout-b",
                "research",
                PromptEvaluationSplit::Holdout,
                0.3,
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    #[test]
    fn complete_diverse_paired_evidence_is_eligible() {
        let result =
            evaluate_prompt_promotion_gate(&complete_evidence(), "candidate", "stable", config());
        assert!(result.eligible, "{:?}", result.blockers);
        assert_eq!(result.unique_train_cases, 2);
        assert_eq!(result.unique_holdout_cases, 2);
        assert_eq!(result.train_task_classes, 2);
        assert_eq!(result.holdout_task_classes, 2);
    }

    #[test]
    fn exact_replays_are_idempotent_but_conflicting_replays_fail_closed() {
        let evidence = complete_evidence();
        let exact_replay = evidence
            .iter()
            .find(|observation| {
                observation.profile_id == "candidate" && observation.evaluation_id == "holdout-1"
            })
            .expect("candidate holdout evidence exists")
            .clone();
        let mut exact = evidence.clone();
        exact.push(exact_replay.clone());
        let exact_result = evaluate_prompt_promotion_gate(&exact, "candidate", "stable", config());
        assert!(exact_result.eligible, "{:?}", exact_result.blockers);

        let mut conflicting_replay = exact_replay;
        conflicting_replay.quality_score = 0.1;
        conflicting_replay.latency_ms = 10_000;
        conflicting_replay.relative_reward = Some(-0.9);
        let mut conflict_after_original = evidence.clone();
        conflict_after_original.push(conflicting_replay.clone());
        let mut conflict_before_original = evidence;
        conflict_before_original.insert(0, conflicting_replay);
        for replayed in [conflict_after_original, conflict_before_original] {
            let result = evaluate_prompt_promotion_gate(&replayed, "candidate", "stable", config());
            assert!(!result.eligible);
            assert!(result
                .blockers
                .contains(&PromptPromotionBlocker::InvalidEvidenceShape));
        }
    }

    #[test]
    fn bounded_holdout_measurement_tolerance_preserves_eligibility() {
        let mut evidence = complete_evidence();
        for observation in &mut evidence {
            if observation.profile_id == "candidate"
                && observation.split == PromptEvaluationSplit::Holdout
            {
                observation.quality_score = 0.895;
                observation.latency_ms = 105;
                observation.total_tokens = 102;
            }
        }

        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());

        assert!(result.eligible, "{:?}", result.blockers);
    }

    #[test]
    fn absolute_holdout_regressions_override_a_relative_reward_win() {
        let mut evidence = complete_evidence();
        for observation in &mut evidence {
            if observation.profile_id == "candidate"
                && observation.split == PromptEvaluationSplit::Holdout
            {
                observation.quality_score = 0.88;
                observation.latency_ms = 106;
                observation.total_tokens = 103;
            }
        }

        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());

        assert!(result.confidence.wins > result.confidence.losses);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutQualityRegression));
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutLatencyRegression));
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutTokenRegression));
    }

    #[test]
    fn repeated_case_cannot_satisfy_diversity() {
        let mut evidence = complete_evidence();
        for observation in &mut evidence {
            if observation.split == PromptEvaluationSplit::Holdout {
                observation.case_id = "same-holdout".to_string();
            }
        }
        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());
        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InsufficientHoldoutCaseDiversity));
    }

    #[test]
    fn missing_counterpart_blocks_promotion() {
        let mut evidence = complete_evidence();
        evidence.retain(|observation| {
            !(observation.profile_id == "stable" && observation.evaluation_id == "holdout-2")
        });
        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::IncompletePairedEvidence));
    }

    #[test]
    fn repeated_task_class_cannot_satisfy_generalization_coverage() {
        let mut evidence = complete_evidence();
        for observation in &mut evidence {
            observation.task_class = "coding".to_string();
        }

        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());

        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InsufficientTrainTaskClassDiversity));
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InsufficientHoldoutTaskClassDiversity));
    }

    #[test]
    fn safety_violation_blocks_promotion() {
        let mut evidence = complete_evidence();
        evidence[0].safety_violations = 1;
        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::SafetyViolation));
    }

    #[test]
    fn mixed_dataset_cohorts_cannot_be_combined_for_promotion() {
        let mut evidence = complete_evidence();
        for observation in evidence.iter_mut().take(4) {
            observation.provenance.dataset_sha256 = "a".repeat(64);
        }
        for observation in evidence.iter_mut().skip(4) {
            observation.provenance.dataset_sha256 = "b".repeat(64);
        }

        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());

        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InsufficientTrainRuns));
    }

    #[test]
    fn matched_execution_cohorts_cannot_be_combined_even_on_the_same_dataset() {
        let mut evidence = complete_evidence();
        bind_matched_cohort(&mut evidence[..4], &"a".repeat(64));
        bind_matched_cohort(&mut evidence[4..], &"b".repeat(64));

        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());

        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InsufficientTrainRuns));
    }

    #[test]
    fn treatment_failure_cannot_be_overridden_by_a_later_pair_for_the_same_attempt() {
        let cohort = "c".repeat(64);
        let mut evidence = complete_evidence();
        bind_matched_cohort(&mut evidence, &cohort);
        let failure = PromptPromotionFailurePenalty {
            evaluation_id: "holdout-failed".to_string(),
            cohort_sha256: cohort.clone(),
            case_id: "holdout-failed-case".to_string(),
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Holdout,
            candidate_profile_id: "candidate".to_string(),
            stable_profile_id: "stable".to_string(),
            candidate_failed: true,
            stable_failed: false,
        };

        let failed = evaluate_prompt_promotion_gate_with_failures_in_cohort(
            &evidence,
            std::slice::from_ref(&failure),
            "candidate",
            "stable",
            &cohort,
            config(),
        );
        assert_eq!(failed.holdout_runs, 3);
        assert_eq!(failed.confidence.losses, 1);
        assert!(failed.holdout_average_reward < 0.9);
        assert!(failed
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutFailureRegression));

        let mut recovered = pair(
            "holdout-failed",
            "holdout-failed-case",
            "coding",
            PromptEvaluationSplit::Holdout,
            0.3,
        );
        bind_matched_cohort(&mut recovered, &cohort);
        evidence.extend(recovered);
        let still_failed = evaluate_prompt_promotion_gate_with_failures_in_cohort(
            &evidence,
            &[failure],
            "candidate",
            "stable",
            &cohort,
            config(),
        );
        assert_eq!(still_failed.holdout_runs, 3);
        assert_eq!(still_failed.confidence.losses, 1);
        assert!(still_failed.holdout_average_reward < 0.9);
    }

    #[test]
    fn train_and_holdout_case_identity_overlap_blocks_promotion() {
        let mut evidence = complete_evidence();
        for observation in &mut evidence {
            if observation.case_id == "holdout-a" {
                observation.case_id = "train-a".to_string();
            }
        }

        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());

        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InvalidEvidenceShape));
    }

    #[test]
    fn mismatched_prompt_lineage_blocks_promotion() {
        let mut evidence = complete_evidence();
        let stable = evidence
            .iter_mut()
            .find(|observation| observation.profile_id == "stable")
            .expect("stable evidence exists");
        stable.provenance.opponent_prompt_sha256 = crate::sha256_hex(b"other-candidate");

        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());

        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InvalidEvidenceShape));
    }

    #[test]
    fn holdout_task_class_regression_blocks_promotion() {
        let mut evidence = complete_evidence();
        for observation in &mut evidence {
            if observation.profile_id == "candidate"
                && observation.split == PromptEvaluationSplit::Holdout
                && observation.task_class == "research"
            {
                observation.relative_reward = Some(-0.2);
            }
        }
        let result = evaluate_prompt_promotion_gate(&evidence, "candidate", "stable", config());
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutTaskClassRegression));
    }

    #[test]
    fn pro_candidate_requires_complete_evidence_against_the_active_auto_profile() {
        let evidence = complete_transfer_evidence();
        let result = evaluate_prompt_auto_transfer_gate(
            &evidence,
            "candidate",
            "auto-stable",
            &crate::sha256_hex(b"auto-stable-genome"),
            config(),
        );

        assert!(result.eligible, "{:?}", result.blockers);
        assert_eq!(result.train_runs, 2);
        assert_eq!(result.holdout_runs, 2);
    }

    #[test]
    fn production_auto_transfer_gate_requires_one_strict_matched_cohort() {
        let auto_profile_sha256 = crate::sha256_hex(b"auto-stable-genome");
        let legacy = complete_transfer_evidence();
        let legacy_result = evaluate_prompt_auto_transfer_gate_in_cohort_with_failures(
            &legacy,
            &[],
            "candidate",
            "auto-stable",
            &auto_profile_sha256,
            &"c".repeat(64),
            config(),
        );
        assert!(!legacy_result.eligible);

        let cohort = "c".repeat(64);
        let mut matched = legacy;
        bind_matched_cohort(&mut matched, &cohort);
        let result = evaluate_prompt_auto_transfer_gate_in_cohort_with_failures(
            &matched,
            &[],
            "candidate",
            "auto-stable",
            &auto_profile_sha256,
            &cohort,
            config(),
        );
        assert!(result.eligible, "{:?}", result.blockers);

        bind_matched_cohort(&mut matched[..4], &"d".repeat(64));
        let mixed = evaluate_prompt_auto_transfer_gate_in_cohort_with_failures(
            &matched,
            &[],
            "candidate",
            "auto-stable",
            &auto_profile_sha256,
            &cohort,
            config(),
        );
        assert!(!mixed.eligible);
    }

    #[test]
    fn stale_auto_profile_evidence_cannot_promote_pro() {
        let result = evaluate_prompt_auto_transfer_gate(
            &complete_transfer_evidence(),
            "candidate",
            "auto-stable",
            &crate::sha256_hex(b"new-auto-genome"),
            config(),
        );

        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InsufficientHoldoutRuns));
    }

    #[test]
    fn mismatched_auto_source_pair_is_rejected() {
        let mut evidence = complete_transfer_evidence();
        let auto = evidence
            .iter_mut()
            .find(|observation| observation.profile_id == "auto-stable")
            .expect("Auto counterpart exists");
        auto.provenance
            .transfer
            .as_mut()
            .expect("transfer provenance exists")
            .source_output_sha256 = crate::sha256_hex(b"different-output");

        let result = evaluate_prompt_auto_transfer_gate(
            &evidence,
            "candidate",
            "auto-stable",
            &crate::sha256_hex(b"auto-stable-genome"),
            config(),
        );

        assert!(!result.eligible);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::InvalidEvidenceShape));
    }
}
