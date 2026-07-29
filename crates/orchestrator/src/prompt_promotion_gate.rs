use crate::{
    prompt_promotion_confidence, PromptEvaluationMode, PromptEvaluationSplit,
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
        }
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
struct PairKey {
    evaluation_id: String,
    case_id: String,
    split: PromptEvaluationSplit,
    mode: PromptEvaluationMode,
    dataset_sha256: String,
}

impl PairKey {
    fn from_observation(observation: &PromptEvolutionObservation) -> Option<Self> {
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
            dataset_sha256: observation.provenance.dataset_sha256.clone(),
        })
    }
}

pub fn evaluate_prompt_promotion_gate(
    observations: &[PromptEvolutionObservation],
    candidate_id: &str,
    stable_id: &str,
    config: PromptPromotionGateConfig,
) -> PromptPromotionGateResult {
    let mut blockers = BTreeSet::new();
    let mut candidate_by_pair = BTreeMap::new();
    let mut stable_by_pair = BTreeMap::new();
    let mut candidate_seen = BTreeSet::new();
    let mut stable_seen = BTreeSet::new();
    let active_dataset_sha256 = crate::latest_scientific_dataset_digest(observations);

    for observation in observations
        .iter()
        .filter(|observation| observation.is_scientific_evidence())
        .filter(|observation| {
            active_dataset_sha256
                .is_some_and(|digest| observation.provenance.dataset_sha256 == digest)
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

    let complete_candidate = candidate_keys
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
            if !mirrored_prompt_lineage || !same_evaluator_protocol {
                blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
                return None;
            }
            if !candidate.format_valid || !stable.format_valid {
                blockers.insert(PromptPromotionBlocker::InvalidFormat);
            }
            if candidate.safety_violations > 0 || stable.safety_violations > 0 {
                blockers.insert(PromptPromotionBlocker::SafetyViolation);
            }
            Some(candidate)
        })
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

    if train.len() < config.minimum_train_runs {
        blockers.insert(PromptPromotionBlocker::InsufficientTrainRuns);
    }
    if holdout.len() < config.minimum_holdout_runs {
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

    let average_reward = |entries: &[&PromptEvolutionObservation]| {
        entries.iter().map(|entry| entry.reward()).sum::<f64>() / entries.len().max(1) as f64
    };
    let train_average_reward = average_reward(&train);
    let holdout_average_reward = average_reward(&holdout);
    if !train.is_empty()
        && !holdout.is_empty()
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
    if holdout_classes.values().any(|rewards| {
        rewards.iter().sum::<f64>() / (rewards.len().max(1) as f64)
            < -config.maximum_holdout_task_class_regression
    }) {
        blockers.insert(PromptPromotionBlocker::HoldoutTaskClassRegression);
    }

    let confidence = prompt_promotion_confidence(holdout.iter().copied());
    if confidence.wilson_lower_bound < config.minimum_wilson_lower_bound {
        blockers.insert(PromptPromotionBlocker::WeakConfidence);
    }
    let blockers = blockers.into_iter().collect::<Vec<_>>();
    PromptPromotionGateResult {
        eligible: blockers.is_empty(),
        train_runs: train.len(),
        holdout_runs: holdout.len(),
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
}
