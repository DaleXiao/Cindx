use crate::prompt_promotion_gate::evaluate_prompt_pair_gate;
use crate::{
    PromptEvolutionObservation, PromptProToAutoDistillationProvenanceV1, PromptPromotionBlocker,
    PromptPromotionFailurePenalty, PromptPromotionGateConfig, PromptPromotionGateResult,
};
use std::collections::BTreeSet;

pub const AUTO_DISTILLATION_MAX_HOLDOUT_LATENCY_REGRESSION_BPS: u64 = 500;
pub const AUTO_DISTILLATION_MAX_HOLDOUT_TOKEN_REGRESSION_BPS: u64 = 200;
pub const AUTO_DISTILLATION_MAX_HOLDOUT_QUALITY_REGRESSION: f64 = 0.01;

#[allow(clippy::too_many_arguments)]
pub fn evaluate_prompt_pro_to_auto_distillation_gate_in_cohort_with_failures(
    observations: &[PromptEvolutionObservation],
    failures: &[PromptPromotionFailurePenalty],
    candidate_id: &str,
    auto_parent_id: &str,
    provenance: &PromptProToAutoDistillationProvenanceV1,
    cohort_sha256: &str,
    config: PromptPromotionGateConfig,
) -> PromptPromotionGateResult {
    let lineage_is_valid = provenance.validate().is_ok()
        && provenance.auto_child_profile_id == candidate_id
        && provenance.auto_parent_profile_id == auto_parent_id;
    let accepts_lineage = |observation: &PromptEvolutionObservation| {
        lineage_is_valid
            && observation.is_strict_pro_to_auto_distillation_evidence()
            && observation.provenance.pro_to_auto_distillation.as_ref() == Some(provenance)
    };
    let mut result = evaluate_prompt_pair_gate(
        observations,
        candidate_id,
        auto_parent_id,
        config,
        Some(cohort_sha256),
        failures,
        accepts_lineage,
        |candidate, parent| {
            candidate.provenance.pro_to_auto_distillation
                == parent.provenance.pro_to_auto_distillation
        },
    );
    let mut blockers = result.blockers.iter().copied().collect::<BTreeSet<_>>();
    if !lineage_is_valid {
        blockers.insert(PromptPromotionBlocker::InvalidEvidenceShape);
    }

    result.blockers = blockers.into_iter().collect();
    result.eligible = result.blockers.is_empty();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

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
            maximum_holdout_quality_regression: AUTO_DISTILLATION_MAX_HOLDOUT_QUALITY_REGRESSION,
            maximum_holdout_latency_regression_bps:
                AUTO_DISTILLATION_MAX_HOLDOUT_LATENCY_REGRESSION_BPS,
            maximum_holdout_token_regression_bps:
                AUTO_DISTILLATION_MAX_HOLDOUT_TOKEN_REGRESSION_BPS,
        }
    }

    fn distillation_provenance() -> PromptProToAutoDistillationProvenanceV1 {
        let auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        let defeated_stable_pro = ConductorPromptGenome::seed_for_effort("pro");
        let mut teacher = defeated_stable_pro.clone();
        teacher.id = "certified-pro-g1".to_string();
        teacher.generation = 1;
        teacher.parents = vec![defeated_stable_pro.id.clone()];
        teacher.retry_policy = PromptRetryPolicy::SameModel;
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            teacher,
            defeated_stable_pro.id.clone(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .unwrap()
        .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: "auto-source".to_string(),
            source_profile_sha256: "c".repeat(64),
            dataset_sha256: "d".repeat(64),
            cohort_sha256: Some("e".repeat(64)),
            paired_evidence_sha256: "f".repeat(64),
            promotion_gate_protocol: PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(
                FrozenPromptSourceProfileLineageV1::undistilled("c".repeat(64)).unwrap(),
            ),
        })
        .unwrap();
        let attestation =
            ProTeacherAttestationV1::from_stable_snapshot(&snapshot, &snapshot.genome.id).unwrap();
        let child = derive_pro_to_auto_distillation_child(
            &auto_parent,
            &snapshot,
            &defeated_stable_pro,
            &snapshot.genome.id,
        )
        .unwrap();
        PromptProToAutoDistillationProvenanceV1::new(
            attestation,
            auto_parent.id.clone(),
            prompt_genome_sha256(&auto_parent).unwrap(),
            child.id.clone(),
            prompt_genome_sha256(&child).unwrap(),
        )
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn observation(
        profile_id: &str,
        opponent_id: &str,
        evaluation_id: &str,
        case_id: &str,
        task_class: &str,
        split: PromptEvaluationSplit,
        relative_reward: f64,
        provenance: PromptEvaluationProvenance,
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
            provenance,
        }
    }

    fn distillation_pair(
        provenance: &PromptProToAutoDistillationProvenanceV1,
        evaluation_id: &str,
        case_id: &str,
        task_class: &str,
        split: PromptEvaluationSplit,
    ) -> [PromptEvolutionObservation; 2] {
        let candidate_provenance = PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["independent-judge".to_string()],
            vec!["candidate-worker".to_string(), "auto-worker".to_string()],
            "1".repeat(64),
            provenance.auto_child_profile_sha256.clone(),
            provenance.auto_parent_profile_sha256.clone(),
        )
        .with_pro_to_auto_distillation(provenance.clone());
        let parent_provenance = PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["independent-judge".to_string()],
            vec!["candidate-worker".to_string(), "auto-worker".to_string()],
            "1".repeat(64),
            provenance.auto_parent_profile_sha256.clone(),
            provenance.auto_child_profile_sha256.clone(),
        )
        .with_pro_to_auto_distillation(provenance.clone());
        [
            observation(
                &provenance.auto_child_profile_id,
                &provenance.auto_parent_profile_id,
                evaluation_id,
                case_id,
                task_class,
                split,
                0.3,
                candidate_provenance,
            ),
            observation(
                &provenance.auto_parent_profile_id,
                &provenance.auto_child_profile_id,
                evaluation_id,
                case_id,
                task_class,
                split,
                -0.3,
                parent_provenance,
            ),
        ]
    }

    fn complete_distillation_evidence() -> (
        PromptProToAutoDistillationProvenanceV1,
        String,
        Vec<PromptEvolutionObservation>,
    ) {
        let provenance = distillation_provenance();
        let cohort = "2".repeat(64);
        let mut evidence = [
            distillation_pair(
                &provenance,
                "distill-train-1",
                "train-a",
                "coding",
                PromptEvaluationSplit::Train,
            ),
            distillation_pair(
                &provenance,
                "distill-train-2",
                "train-b",
                "research",
                PromptEvaluationSplit::Train,
            ),
            distillation_pair(
                &provenance,
                "distill-holdout-1",
                "holdout-a",
                "coding",
                PromptEvaluationSplit::Holdout,
            ),
            distillation_pair(
                &provenance,
                "distill-holdout-2",
                "holdout-b",
                "research",
                PromptEvaluationSplit::Holdout,
            ),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        for observation in &mut evidence {
            observation.provenance.matched_evaluation = Some(PromptMatchedEvaluationIdentityV1 {
                schema: PROMPT_MATCHED_EVALUATION_SCHEMA_V1.to_string(),
                evaluation_id: observation.evaluation_id.clone(),
                cohort_sha256: cohort.clone(),
                dataset_sha256: observation.provenance.dataset_sha256.clone(),
                case_id: observation.case_id.clone(),
                objective_sha256: sha256_hex(observation.case_id.as_bytes()),
                split: observation.split,
                mode: observation.mode,
            });
        }
        (provenance, cohort, evidence)
    }

    fn evaluate_distillation(
        evidence: &[PromptEvolutionObservation],
        failures: &[PromptPromotionFailurePenalty],
        provenance: &PromptProToAutoDistillationProvenanceV1,
        cohort: &str,
    ) -> PromptPromotionGateResult {
        evaluate_prompt_pro_to_auto_distillation_gate_in_cohort_with_failures(
            evidence,
            failures,
            &provenance.auto_child_profile_id,
            &provenance.auto_parent_profile_id,
            provenance,
            cohort,
            config(),
        )
    }

    #[test]
    fn bounded_distillation_with_fresh_matched_evidence_is_eligible() {
        let (provenance, cohort, evidence) = complete_distillation_evidence();
        let result = evaluate_distillation(&evidence, &[], &provenance, &cohort);
        assert!(result.eligible, "{:?}", result.blockers);
    }

    #[test]
    fn documented_small_measurement_tolerance_is_allowed() {
        let (provenance, cohort, mut evidence) = complete_distillation_evidence();
        for observation in &mut evidence {
            if observation.profile_id == provenance.auto_child_profile_id
                && observation.split == PromptEvaluationSplit::Holdout
            {
                observation.latency_ms = 105;
                observation.total_tokens = 102;
                observation.quality_score = 0.895;
            }
        }
        let result = evaluate_distillation(&evidence, &[], &provenance, &cohort);
        assert!(result.eligible, "{:?}", result.blockers);
    }

    #[test]
    fn holdout_resource_and_quality_regressions_are_hard_blockers() {
        let (provenance, cohort, mut evidence) = complete_distillation_evidence();
        for observation in &mut evidence {
            if observation.profile_id == provenance.auto_child_profile_id
                && observation.split == PromptEvaluationSplit::Holdout
            {
                observation.latency_ms = 106;
                observation.total_tokens = 103;
                observation.quality_score = 0.88;
            }
        }
        let result = evaluate_distillation(&evidence, &[], &provenance, &cohort);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutLatencyRegression));
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutTokenRegression));
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutQualityRegression));
    }

    #[test]
    fn candidate_failure_remains_in_the_non_regression_denominator() {
        let (provenance, cohort, evidence) = complete_distillation_evidence();
        let failure = PromptPromotionFailurePenalty {
            evaluation_id: "distill-holdout-failed".to_string(),
            cohort_sha256: cohort.clone(),
            case_id: "holdout-failed".to_string(),
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Holdout,
            candidate_profile_id: provenance.auto_child_profile_id.clone(),
            stable_profile_id: provenance.auto_parent_profile_id.clone(),
            candidate_failed: true,
            stable_failed: false,
        };
        let result = evaluate_distillation(&evidence, &[failure], &provenance, &cohort);
        assert!(result
            .blockers
            .contains(&PromptPromotionBlocker::HoldoutFailureRegression));
    }
}
