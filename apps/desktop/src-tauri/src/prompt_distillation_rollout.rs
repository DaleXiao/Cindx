use crate::runtime_constants::PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1;
use crate::view_models::{
    PromptDistillationCanaryLeaseV1, PromptEvolutionReadModel,
};
use orchestrator::{
    evaluate_prompt_pro_to_auto_distillation_gate_in_cohort_with_failures, prompt_genome_sha256,
    sha256_hex, FrozenPromptProToAutoDistillationEvidence, FrozenPromptProfileSnapshot,
    PromptEvaluationAttemptStatus, PromptEvaluationSplit, PromptEvolutionMethod,
    PromptEvolutionObservation, PromptProToAutoDistillationProvenanceV1,
    PromptPromotionFailurePenalty, PromptPromotionGateResult,
    AUTO_DISTILLATION_MAX_HOLDOUT_LATENCY_REGRESSION_BPS,
    AUTO_DISTILLATION_MAX_HOLDOUT_QUALITY_REGRESSION,
    AUTO_DISTILLATION_MAX_HOLDOUT_TOKEN_REGRESSION_BPS,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct TrustedPromptDistillationGate {
    pub(crate) result: PromptPromotionGateResult,
    pub(crate) cohort_sha256: String,
    pub(crate) provenance: PromptProToAutoDistillationProvenanceV1,
    pub(crate) observations: Vec<PromptEvolutionObservation>,
}

pub(crate) enum PromptDistillationCanaryAssessment {
    Pending,
    Healthy,
    Degraded(String),
}

pub(crate) fn prompt_candidate_is_distillation(
    model: &PromptEvolutionReadModel,
    effort: &str,
    candidate_id: &str,
) -> bool {
    effort == "auto"
        && model.genomes.iter().any(|record| {
            record.effort == effort
                && record.genome.id == candidate_id
                && record.evolution_method == Some(PromptEvolutionMethod::ProToAutoDistillation)
        })
}

pub(crate) fn latest_prompt_distillation_candidate(
    model: &PromptEvolutionReadModel,
    auto_parent_id: &str,
) -> Option<String> {
    let mut cohorts = BTreeMap::<(u64, String), BTreeSet<String>>::new();
    for (_, observation) in model.observations.iter().filter(|(effort, observation)| {
        effort == "auto"
            && observation.is_strict_pro_to_auto_distillation_evidence()
            && observation.opponent_profile_id.as_deref() == Some(auto_parent_id)
    }) {
        let Some(provenance) = observation.provenance.pro_to_auto_distillation.as_ref() else {
            continue;
        };
        if observation.profile_id != provenance.auto_child_profile_id
            || provenance.auto_parent_profile_id != auto_parent_id
            || !prompt_candidate_is_distillation(model, "auto", &observation.profile_id)
        {
            continue;
        }
        let Some(cohort) = observation.scientific_cohort_sha256() else {
            continue;
        };
        let Some(sequence) = model.cohort_sequences.get(cohort).copied() else {
            continue;
        };
        cohorts
            .entry((sequence, cohort.to_string()))
            .or_default()
            .insert(observation.profile_id.clone());
    }
    let (_, candidates) = cohorts.into_iter().next_back()?;
    if candidates.len() != 1 {
        return None;
    }
    candidates.into_iter().next()
}

pub(crate) fn evaluate_trusted_prompt_distillation_gate(
    model: &PromptEvolutionReadModel,
    candidate_id: &str,
    auto_parent_id: &str,
) -> Result<TrustedPromptDistillationGate, String> {
    let active_cohort = model
        .observations
        .iter()
        .filter(|(effort, observation)| {
            effort == "auto"
                && observation.is_strict_pro_to_auto_distillation_evidence()
                && ((observation.profile_id == candidate_id
                    && observation.opponent_profile_id.as_deref() == Some(auto_parent_id))
                    || (observation.profile_id == auto_parent_id
                        && observation.opponent_profile_id.as_deref() == Some(candidate_id)))
        })
        .filter_map(|(_, observation)| {
            let cohort = observation.scientific_cohort_sha256()?;
            let sequence = model.cohort_sequences.get(cohort).copied()?;
            Some((sequence, cohort))
        })
        .max_by(|left, right| left.cmp(right))
        .map(|(_, cohort)| cohort.to_string())
        .ok_or_else(|| "active Auto distillation cohort is unavailable".to_string())?;
    let observations = model
        .observations
        .iter()
        .filter(|(effort, observation)| {
            effort == "auto"
                && observation.is_strict_pro_to_auto_distillation_evidence()
                && observation.scientific_cohort_sha256() == Some(active_cohort.as_str())
                && ((observation.profile_id == candidate_id
                    && observation.opponent_profile_id.as_deref() == Some(auto_parent_id))
                    || (observation.profile_id == auto_parent_id
                        && observation.opponent_profile_id.as_deref() == Some(candidate_id)))
        })
        .map(|(_, observation)| observation.clone())
        .collect::<Vec<_>>();
    let provenance = observations
        .iter()
        .find(|observation| observation.profile_id == candidate_id)
        .and_then(|observation| observation.provenance.pro_to_auto_distillation.clone())
        .ok_or_else(|| "Auto distillation provenance is unavailable".to_string())?;
    if observations.iter().any(|observation| {
        observation.provenance.pro_to_auto_distillation.as_ref() != Some(&provenance)
    }) {
        return Err("Auto distillation cohort mixes lineage".to_string());
    }
    let failures = prompt_distillation_failure_penalties(
        model,
        &active_cohort,
        candidate_id,
        auto_parent_id,
        &provenance,
    );
    let result = evaluate_prompt_pro_to_auto_distillation_gate_in_cohort_with_failures(
        &observations,
        &failures,
        candidate_id,
        auto_parent_id,
        &provenance,
        &active_cohort,
        crate::prompt_rollout_runtime::prompt_promotion_gate_config(),
    );
    Ok(TrustedPromptDistillationGate {
        result,
        cohort_sha256: active_cohort,
        provenance,
        observations,
    })
}

pub(crate) fn prompt_distillation_canary_lease(
    model: &PromptEvolutionReadModel,
    candidate_id: &str,
    auto_parent_id: &str,
    trusted: &TrustedPromptDistillationGate,
) -> Result<PromptDistillationCanaryLeaseV1, String> {
    if !trusted.result.eligible
        || trusted.provenance.auto_child_profile_id != candidate_id
        || trusted.provenance.auto_parent_profile_id != auto_parent_id
    {
        return Err("Auto distillation canary lease requires an eligible matched gate".to_string());
    }
    let candidate = model
        .genomes
        .iter()
        .find(|record| {
            record.effort == "auto"
                && record.genome.id == candidate_id
                && record.evolution_method == Some(PromptEvolutionMethod::ProToAutoDistillation)
        })
        .ok_or_else(|| "Auto distillation canary candidate is unavailable".to_string())?;
    let (stable, stable_sha256) =
        crate::prompt_rollout_runtime::stable_prompt_profile_fingerprint(model, "auto")?;
    if stable.id != auto_parent_id {
        return Err("Auto distillation canary stable profile drifted".to_string());
    }
    let candidate_sha256 = prompt_genome_sha256(&candidate.genome)?;
    if candidate_sha256 != trusted.provenance.auto_child_profile_sha256
        || stable_sha256 != trusted.provenance.auto_parent_profile_sha256
    {
        return Err("Auto distillation canary profile fingerprint drifted".to_string());
    }
    let mut observations = trusted.observations.clone();
    observations.sort_by_key(PromptEvolutionObservation::evidence_identity);
    let paired_evidence_sha256 = serde_json::to_vec(&("paired", &observations))
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("Auto distillation canary evidence serialization failed: {error}"))?;
    Ok(PromptDistillationCanaryLeaseV1 {
        schema: PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1.to_string(),
        candidate_profile_id: candidate_id.to_string(),
        candidate_profile_sha256: candidate_sha256,
        stable_profile_id: auto_parent_id.to_string(),
        stable_profile_sha256: stable_sha256,
        cohort_sha256: trusted.cohort_sha256.clone(),
        paired_evidence_sha256,
    })
}

fn prompt_distillation_failure_penalties(
    model: &PromptEvolutionReadModel,
    cohort_sha256: &str,
    candidate_id: &str,
    auto_parent_id: &str,
    provenance: &PromptProToAutoDistillationProvenanceV1,
) -> Vec<PromptPromotionFailurePenalty> {
    let Some(cohort) = model.cohorts.get(cohort_sha256) else {
        return Vec::new();
    };
    model
        .attempts
        .values()
        .filter(|attempt| attempt.started.identity.cohort_sha256 == cohort_sha256)
        .filter(|attempt| {
            crate::prompt_rollout_runtime::prompt_attempt_matches_profiles(
                attempt,
                candidate_id,
                auto_parent_id,
            )
        })
        .filter(|attempt| {
            let complete_pair = model
                .observations
                .iter()
                .filter(|(_, observation)| {
                    observation.is_strict_pro_to_auto_distillation_evidence()
                        && observation.provenance.pro_to_auto_distillation.as_ref()
                            == Some(provenance)
                        && crate::prompt_attempt_runtime::prompt_observation_matches_attempt(
                            observation,
                            &attempt.started,
                        )
                })
                .count()
                == 2;
            attempt.terminal.as_ref().is_some_and(|terminal| {
                terminal.status == PromptEvaluationAttemptStatus::TreatmentFailure
            }) || !complete_pair
        })
        .filter_map(|attempt| {
            let treatment_failures = match attempt.terminal.as_ref() {
                None => [true; 2],
                Some(terminal) if terminal.treatment_failures.into_iter().any(|failed| failed) => {
                    terminal.treatment_failures
                }
                Some(terminal) if terminal.status.enters_effect_denominator() => [true; 2],
                Some(_) => return None,
            };
            let case = cohort.dataset.case(&attempt.started.identity.case_id)?;
            let candidate_index = attempt
                .started
                .treatments
                .iter()
                .position(|treatment| treatment.profile_id == candidate_id)?;
            let parent_index = attempt
                .started
                .treatments
                .iter()
                .position(|treatment| treatment.profile_id == auto_parent_id)?;
            Some(PromptPromotionFailurePenalty {
                evaluation_id: attempt.started.identity.evaluation_id.clone(),
                cohort_sha256: cohort_sha256.to_string(),
                case_id: case.case_id.clone(),
                task_class: case.task_family_sha256.clone(),
                split: case.split,
                candidate_profile_id: candidate_id.to_string(),
                stable_profile_id: auto_parent_id.to_string(),
                candidate_failed: treatment_failures[candidate_index],
                stable_failed: treatment_failures[parent_index],
            })
        })
        .collect()
}

pub(crate) fn frozen_distillation_profile_for_promotion(
    model: &PromptEvolutionReadModel,
    candidate_id: &str,
    auto_parent_id: &str,
) -> Result<FrozenPromptProfileSnapshot, String> {
    let record = model
        .genomes
        .iter()
        .find(|record| {
            record.effort == "auto"
                && record.genome.id == candidate_id
                && record.evolution_method == Some(PromptEvolutionMethod::ProToAutoDistillation)
        })
        .ok_or_else(|| format!("Auto distillation profile {candidate_id} is unavailable"))?;
    let trusted = evaluate_trusted_prompt_distillation_gate(model, candidate_id, auto_parent_id)?;
    if !trusted.result.eligible {
        let blockers = trusted
            .result
            .blockers
            .iter()
            .map(|blocker| blocker.label())
            .collect::<Vec<_>>()
            .join(",");
        return Err(format!(
            "cannot freeze Auto distillation before its promotion gate passes: {blockers}"
        ));
    }
    let dataset_sha256 = trusted
        .observations
        .first()
        .map(|observation| observation.provenance.dataset_sha256.clone())
        .ok_or_else(|| "Auto distillation cohort has no evidence".to_string())?;
    let candidate_sha256 = prompt_genome_sha256(&record.genome)?;
    if trusted.provenance.auto_child_profile_sha256 != candidate_sha256
        || trusted.provenance.auto_child_profile_id != candidate_id
        || trusted.provenance.auto_parent_profile_id != auto_parent_id
    {
        return Err("Auto distillation candidate lineage does not match its genome".to_string());
    }

    let mut observations = trusted.observations;
    observations.sort_by_key(PromptEvolutionObservation::evidence_identity);
    let train = observations
        .iter()
        .filter(|observation| observation.split == PromptEvaluationSplit::Train)
        .cloned()
        .collect::<Vec<_>>();
    let holdout = observations
        .iter()
        .filter(|observation| observation.split == PromptEvaluationSplit::Holdout)
        .cloned()
        .collect::<Vec<_>>();
    let evidence_sha256 = |label: &str, evidence: &[PromptEvolutionObservation]| {
        serde_json::to_vec(&(label, evidence))
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("Auto distillation evidence serialization failed: {error}"))
    };
    let train_evidence_sha256 = evidence_sha256("train", &train)?;
    let holdout_evidence_sha256 = evidence_sha256("holdout", &holdout)?;
    let paired_evidence_sha256 = evidence_sha256("paired", &observations)?;
    let frozen_evidence = FrozenPromptProToAutoDistillationEvidence::new(
        &trusted.provenance,
        dataset_sha256.clone(),
        trusted.cohort_sha256,
        train_evidence_sha256,
        holdout_evidence_sha256,
        paired_evidence_sha256.clone(),
    )?;
    FrozenPromptProfileSnapshot::new_pro_to_auto_distillation(
        record.genome.clone(),
        auto_parent_id,
        dataset_sha256,
        paired_evidence_sha256,
        frozen_evidence,
    )
}

pub(crate) fn prompt_distillation_canary_assessment(
    model: &PromptEvolutionReadModel,
    auto_parent_id: &str,
    candidate_id: &str,
    candidate_checkpoint: usize,
    parent_checkpoint: usize,
) -> PromptDistillationCanaryAssessment {
    let candidate_all =
        crate::prompt_rollout_runtime::prompt_live_observations(model, "auto", candidate_id);
    let parent_all =
        crate::prompt_rollout_runtime::prompt_live_observations(model, "auto", auto_parent_id);
    if candidate_all.len() < candidate_checkpoint || parent_all.len() < parent_checkpoint {
        return PromptDistillationCanaryAssessment::Degraded(
            "canary_checkpoint_regressed".to_string(),
        );
    }
    let candidate_fresh = &candidate_all[candidate_checkpoint..];
    let parent_fresh = &parent_all[parent_checkpoint..];
    if candidate_fresh
        .iter()
        .any(|observation| observation.safety_violations > 0)
    {
        return PromptDistillationCanaryAssessment::Degraded(
            "canary_safety_regression".to_string(),
        );
    }
    let candidate_classes = candidate_fresh
        .iter()
        .map(|observation| observation.task_class.as_str())
        .collect::<BTreeSet<_>>();
    let parent_classes = parent_fresh
        .iter()
        .map(|observation| observation.task_class.as_str())
        .collect::<BTreeSet<_>>();
    if !candidate_classes.is_subset(&parent_classes) {
        return PromptDistillationCanaryAssessment::Pending;
    }
    let common_classes = candidate_classes
        .intersection(&parent_classes)
        .copied()
        .collect::<BTreeSet<_>>();
    let candidate = candidate_fresh
        .iter()
        .copied()
        .filter(|observation| common_classes.contains(observation.task_class.as_str()))
        .collect::<Vec<_>>();
    let parent = parent_fresh
        .iter()
        .copied()
        .filter(|observation| common_classes.contains(observation.task_class.as_str()))
        .collect::<Vec<_>>();
    if candidate.len() < 2 || parent.len() < 2 {
        return PromptDistillationCanaryAssessment::Pending;
    }
    let rate_regressed = |candidate_passes: usize,
                          candidate_runs: usize,
                          parent_passes: usize,
                          parent_runs: usize| {
        candidate_passes.saturating_mul(parent_runs)
            < parent_passes.saturating_mul(candidate_runs)
    };
    for task_class in &candidate_classes {
        let candidate_class = candidate
            .iter()
            .copied()
            .filter(|entry| entry.task_class == *task_class)
            .collect::<Vec<_>>();
        let parent_class = parent
            .iter()
            .copied()
            .filter(|entry| entry.task_class == *task_class)
            .collect::<Vec<_>>();
        if parent_class.is_empty() {
            return PromptDistillationCanaryAssessment::Pending;
        }
        if rate_regressed(
            candidate_class.iter().filter(|entry| entry.succeeded).count(),
            candidate_class.len(),
            parent_class.iter().filter(|entry| entry.succeeded).count(),
            parent_class.len(),
        ) {
            return PromptDistillationCanaryAssessment::Degraded(format!(
                "canary_completion_regression:{task_class}"
            ));
        }
        if rate_regressed(
            candidate_class
                .iter()
                .filter(|entry| entry.format_valid)
                .count(),
            candidate_class.len(),
            parent_class.iter().filter(|entry| entry.format_valid).count(),
            parent_class.len(),
        ) {
            return PromptDistillationCanaryAssessment::Degraded(format!(
                "canary_format_regression:{task_class}"
            ));
        }
        let candidate_class_quality = candidate_class
            .iter()
            .map(|entry| entry.quality_score)
            .sum::<f64>()
            / candidate_class.len() as f64;
        let parent_class_quality = parent_class
            .iter()
            .map(|entry| entry.quality_score)
            .sum::<f64>()
            / parent_class.len() as f64;
        if candidate_class_quality + AUTO_DISTILLATION_MAX_HOLDOUT_QUALITY_REGRESSION
            < parent_class_quality
        {
            return PromptDistillationCanaryAssessment::Degraded(format!(
                "canary_quality_regression:{task_class}"
            ));
        }
    }
    if rate_regressed(
        candidate.iter().filter(|entry| entry.succeeded).count(),
        candidate.len(),
        parent.iter().filter(|entry| entry.succeeded).count(),
        parent.len(),
    ) {
        return PromptDistillationCanaryAssessment::Degraded(
            "canary_completion_regression".to_string(),
        );
    }
    if rate_regressed(
        candidate.iter().filter(|entry| entry.format_valid).count(),
        candidate.len(),
        parent.iter().filter(|entry| entry.format_valid).count(),
        parent.len(),
    ) {
        return PromptDistillationCanaryAssessment::Degraded(
            "canary_format_regression".to_string(),
        );
    }
    let candidate_quality = candidate
        .iter()
        .map(|observation| observation.quality_score)
        .sum::<f64>()
        / candidate.len() as f64;
    let parent_quality = parent
        .iter()
        .map(|observation| observation.quality_score)
        .sum::<f64>()
        / parent.len() as f64;
    if candidate_quality + AUTO_DISTILLATION_MAX_HOLDOUT_QUALITY_REGRESSION < parent_quality {
        return PromptDistillationCanaryAssessment::Degraded(
            "canary_quality_regression".to_string(),
        );
    }
    let average_exceeds = |candidate_total: u128,
                           candidate_count: usize,
                           parent_total: u128,
                           parent_count: usize,
                           tolerance_bps: u64| {
        parent_total == 0 && candidate_total > 0
            || parent_total > 0
                && candidate_total
                    .saturating_mul(parent_count as u128)
                    .saturating_mul(10_000)
                    > parent_total
                        .saturating_mul(candidate_count as u128)
                        .saturating_mul(10_000 + u128::from(tolerance_bps))
    };
    let candidate_latency = candidate
        .iter()
        .map(|observation| u128::from(observation.latency_ms))
        .sum();
    let parent_latency = parent
        .iter()
        .map(|observation| u128::from(observation.latency_ms))
        .sum();
    if average_exceeds(
        candidate_latency,
        candidate.len(),
        parent_latency,
        parent.len(),
        AUTO_DISTILLATION_MAX_HOLDOUT_LATENCY_REGRESSION_BPS,
    ) {
        return PromptDistillationCanaryAssessment::Degraded(
            "canary_latency_regression".to_string(),
        );
    }
    let candidate_tokens = candidate
        .iter()
        .map(|observation| u128::from(observation.total_tokens))
        .sum();
    let parent_tokens = parent
        .iter()
        .map(|observation| u128::from(observation.total_tokens))
        .sum();
    if average_exceeds(
        candidate_tokens,
        candidate.len(),
        parent_tokens,
        parent.len(),
        AUTO_DISTILLATION_MAX_HOLDOUT_TOKEN_REGRESSION_BPS,
    ) {
        return PromptDistillationCanaryAssessment::Degraded(
            "canary_token_regression".to_string(),
        );
    }
    PromptDistillationCanaryAssessment::Healthy
}

#[cfg(test)]
mod tests {
    use super::evaluate_trusted_prompt_distillation_gate;
    use crate::prompt_evolution_models::PromptEvolutionEvaluation;
    use crate::prompt_evolution_read_model::build_prompt_evolution_read_model;
    use crate::prompt_rollout_runtime::{prompt_active_pair_cohort, reconcile_prompt_rollout};
    use crate::tests::bind_matched_prompt_evidence;
    use crate::view_models::{
        PromptEvaluationAttemptState, PromptEvolutionReadModel, PromptGenomeRecord,
    };
    use orchestrator::{
        derive_pro_to_auto_distillation_child, prompt_genome_sha256, sha256_hex,
        ConductorPromptGenome, FrozenPromptProfileSnapshot, FrozenPromptSourceProfileLineageV1,
        FrozenPromptTransferEvidence, ProTeacherAttestationV1, PromptDatasetCaseIdentityV1,
        PromptDatasetIdentityV1, PromptEvaluationAttemptEventV1, PromptEvaluationAttemptStatus,
        PromptEvaluationMode, PromptEvaluationProvenance, PromptEvaluationSplit,
        PromptEvolutionMethod, PromptEvolutionObservation, PromptLearningCohortV1,
        PromptProToAutoDistillationProvenanceV1, PromptRetryPolicy, PromptTreatmentIdentityV1,
        PROMPT_AUTO_TRANSFER_GATE_PROTOCOL,
    };
    use std::collections::BTreeSet;

    struct DistillationRolloutFixture {
        model: PromptEvolutionReadModel,
        auto_parent: ConductorPromptGenome,
        auto_child: ConductorPromptGenome,
        provenance: PromptProToAutoDistillationProvenanceV1,
        evaluation: PromptEvolutionEvaluation,
    }

    fn certified_distillation_fixture() -> DistillationRolloutFixture {
        let auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        let defeated_stable_pro = ConductorPromptGenome::seed_for_effort("pro");
        let mut pro_teacher = defeated_stable_pro.clone();
        pro_teacher.id = "certified-pro-g1".to_string();
        pro_teacher.generation = 1;
        pro_teacher.parents = vec![defeated_stable_pro.id.clone()];
        pro_teacher.retry_policy = PromptRetryPolicy::SameModel;
        let pro_snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            pro_teacher,
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
        let auto_child = derive_pro_to_auto_distillation_child(
            &auto_parent,
            &pro_snapshot,
            &defeated_stable_pro,
            &pro_snapshot.genome.id,
        )
        .unwrap();
        let provenance = PromptProToAutoDistillationProvenanceV1::new(
            ProTeacherAttestationV1::from_stable_snapshot(&pro_snapshot, &pro_snapshot.genome.id)
                .unwrap(),
            auto_parent.id.clone(),
            prompt_genome_sha256(&auto_parent).unwrap(),
            auto_child.id.clone(),
            prompt_genome_sha256(&auto_child).unwrap(),
        )
        .unwrap();
        let mut model = build_prompt_evolution_read_model(&[], 0, 0);
        model.genomes.push(PromptGenomeRecord {
            scope: "global".to_string(),
            effort: "auto".to_string(),
            genome: auto_child.clone(),
            evolution_method: Some(PromptEvolutionMethod::ProToAutoDistillation),
        });
        for index in 0..14 {
            let split = if index < 6 {
                PromptEvaluationSplit::Train
            } else {
                PromptEvaluationSplit::Holdout
            };
            append_distillation_pair(&mut model, &provenance, index, split, 100, 100);
        }
        bind_matched_prompt_evidence(&mut model, "auto", &auto_child.id, &auto_parent.id);
        let evaluation = PromptEvolutionEvaluation {
            population: vec![auto_parent.clone(), auto_child.clone()],
            observations: Vec::new(),
            frontier_ids: BTreeSet::from([auto_parent.id.clone()]),
            champion_id: Some(auto_parent.id.clone()),
            champion_score: Some(0.9),
            champion_confidence: None,
            status: "stable".to_string(),
            freeze_reason: None,
            stagnant_generations: 0,
            evaluated_generations: 1,
            next_mode: "exploit".to_string(),
            next_profile: auto_parent.clone(),
            mutation_parent: None,
            mutation_trajectories: Vec::new(),
        };
        DistillationRolloutFixture {
            model,
            auto_parent,
            auto_child,
            provenance,
            evaluation,
        }
    }

    fn append_distillation_pair(
        model: &mut PromptEvolutionReadModel,
        provenance: &PromptProToAutoDistillationProvenanceV1,
        index: usize,
        split: PromptEvaluationSplit,
        candidate_latency_ms: u64,
        candidate_tokens: u64,
    ) {
        let evaluation_id = format!("distill-{index}");
        let case_id = format!("distill-case-{index}");
        let task_class = if index.is_multiple_of(2) {
            "coding"
        } else {
            "research"
        };
        let mode = match split {
            PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
            PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
        };
        let scientific_provenance = |candidate_sha256: String, opponent_sha256: String| {
            PromptEvaluationProvenance::blind_pairwise_swap(
                vec!["independent-judge".to_string()],
                vec!["candidate-worker".to_string(), "auto-worker".to_string()],
                "1".repeat(64),
                candidate_sha256,
                opponent_sha256,
            )
            .with_pro_to_auto_distillation(provenance.clone())
        };
        let observation =
            |profile_id: String,
             opponent_id: String,
             quality_score: f64,
             latency_ms: u64,
             total_tokens: u64,
             relative_reward: f64,
             provenance: PromptEvaluationProvenance| PromptEvolutionObservation {
                profile_id,
                evaluation_id: evaluation_id.clone(),
                case_id: case_id.clone(),
                opponent_profile_id: Some(opponent_id),
                task_class: task_class.to_string(),
                split,
                mode,
                format_valid: true,
                succeeded: true,
                quality_score,
                latency_ms,
                total_tokens,
                estimated_cost_microusd: 0,
                safety_violations: 0,
                relative_reward: Some(relative_reward),
                step_credits: Vec::new(),
                reflection_packet: None,
                provenance,
            };
        let candidate = observation(
            provenance.auto_child_profile_id.clone(),
            provenance.auto_parent_profile_id.clone(),
            0.95,
            candidate_latency_ms,
            candidate_tokens,
            0.5,
            scientific_provenance(
                provenance.auto_child_profile_sha256.clone(),
                provenance.auto_parent_profile_sha256.clone(),
            ),
        );
        let parent = observation(
            provenance.auto_parent_profile_id.clone(),
            provenance.auto_child_profile_id.clone(),
            0.9,
            100,
            100,
            -0.5,
            scientific_provenance(
                provenance.auto_parent_profile_sha256.clone(),
                provenance.auto_child_profile_sha256.clone(),
            ),
        );
        model.observations.extend([
            ("auto".to_string(), candidate),
            ("auto".to_string(), parent),
        ]);
    }

    fn append_live_pair(fixture: &mut DistillationRolloutFixture, index: usize) {
        let live =
            |profile: &ConductorPromptGenome, quality_score: f64| PromptEvolutionObservation {
                profile_id: profile.id.clone(),
                evaluation_id: format!("live-{}-{index}", profile.id),
                case_id: format!("live-case-{}-{index}", profile.id),
                opponent_profile_id: None,
                task_class: "coding".to_string(),
                split: PromptEvaluationSplit::Train,
                mode: PromptEvaluationMode::Live,
                format_valid: true,
                succeeded: true,
                quality_score,
                latency_ms: 100,
                total_tokens: 100,
                estimated_cost_microusd: 0,
                safety_violations: 0,
                relative_reward: None,
                step_credits: Vec::new(),
                reflection_packet: None,
                provenance: PromptEvaluationProvenance::blind_pairwise_swap(
                    vec!["independent-live-judge".to_string()],
                    vec!["live-worker".to_string()],
                    "2".repeat(64),
                    prompt_genome_sha256(profile).unwrap(),
                    "3".repeat(64),
                ),
            };
        fixture.model.observations.extend([
            ("auto".to_string(), live(&fixture.auto_child, 0.95)),
            ("auto".to_string(), live(&fixture.auto_parent, 0.9)),
        ]);
    }

    fn insert_later_ordinary_pair_attempt(fixture: &mut DistillationRolloutFixture) -> String {
        let execution = fixture
            .model
            .cohorts
            .values()
            .next()
            .unwrap()
            .execution
            .clone();
        let dataset = PromptDatasetIdentityV1::new(
            "later-ordinary-scope",
            1,
            vec![PromptDatasetCaseIdentityV1 {
                case_id: "later-ordinary-case".to_string(),
                objective_sha256: sha256_hex(b"later ordinary objective"),
                task_family_sha256: sha256_hex(b"coding"),
                split: PromptEvaluationSplit::Train,
            }],
        )
        .unwrap();
        let cohort = PromptLearningCohortV1::new(dataset, execution).unwrap();
        let identity = orchestrator::PromptMatchedEvaluationIdentityV1::new(
            "later-ordinary-evaluation",
            &cohort,
            "later-ordinary-case",
            PromptEvaluationSplit::Train,
            PromptEvaluationMode::PairedExecution,
        )
        .unwrap();
        let started = PromptEvaluationAttemptEventV1::started(
            identity,
            &cohort,
            [
                PromptTreatmentIdentityV1 {
                    profile_id: fixture.auto_child.id.clone(),
                    prompt_sha256: fixture.provenance.auto_child_profile_sha256.clone(),
                },
                PromptTreatmentIdentityV1 {
                    profile_id: fixture.auto_parent.id.clone(),
                    prompt_sha256: fixture.provenance.auto_parent_profile_sha256.clone(),
                },
            ],
        )
        .unwrap();
        let terminal = PromptEvaluationAttemptEventV1::terminal(
            &started,
            PromptEvaluationAttemptStatus::CompletedPair,
            [false; 2],
            "",
        )
        .unwrap();
        fixture.model.attempts.insert(
            started.identity.evaluation_id.clone(),
            PromptEvaluationAttemptState {
                started,
                terminal: Some(terminal),
            },
        );
        fixture
            .model
            .cohort_sequences
            .insert(cohort.cohort_sha256.clone(), 99);
        fixture
            .model
            .cohorts
            .insert(cohort.cohort_sha256.clone(), cohort.clone());
        cohort.cohort_sha256
    }

    #[test]
    fn distillation_reaches_canary_ignores_ordinary_track_and_promotes_on_fresh_live_traffic() {
        let mut fixture = certified_distillation_fixture();
        let strict_cohort = evaluate_trusted_prompt_distillation_gate(
            &fixture.model,
            &fixture.auto_child.id,
            &fixture.auto_parent.id,
        )
        .unwrap()
        .cohort_sha256;
        let ordinary_cohort = insert_later_ordinary_pair_attempt(&mut fixture);
        assert_eq!(
            prompt_active_pair_cohort(
                &fixture.model,
                &fixture.auto_child.id,
                &fixture.auto_parent.id,
            )
            .as_deref(),
            Some(ordinary_cohort.as_str())
        );
        assert_eq!(
            evaluate_trusted_prompt_distillation_gate(
                &fixture.model,
                &fixture.auto_child.id,
                &fixture.auto_parent.id,
            )
            .unwrap()
            .cohort_sha256,
            strict_cohort
        );

        let started = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(
            started.canary_profile_id,
            Some(fixture.auto_child.id.clone())
        );
        assert_eq!(started.canary_percent, 10);

        let without_live =
            reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(without_live.canary_percent, 10);

        let mut regressed = fixture.model.clone();
        for (_, observation) in &mut regressed.observations {
            if observation.profile_id == fixture.auto_child.id
                && observation.split == PromptEvaluationSplit::Holdout
            {
                observation.latency_ms = 200;
            }
        }
        let rolled_back = reconcile_prompt_rollout(&mut regressed, "auto", &fixture.evaluation);
        assert_eq!(rolled_back.status, "rolled_back");
        assert!(rolled_back.canary_profile_id.is_none());
        assert_eq!(rolled_back.rollback_count, 1);
        assert_eq!(
            rolled_back.last_reason.as_deref(),
            Some("promotion_gate_regressed:holdout_latency_regression")
        );
        let quarantined = reconcile_prompt_rollout(&mut regressed, "auto", &fixture.evaluation);
        assert_eq!(quarantined.status, "stable");
        assert!(quarantined.canary_profile_id.is_none());
        assert_eq!(
            quarantined.quarantined_profile_ids,
            vec![fixture.auto_child.id.clone()]
        );

        append_live_pair(&mut fixture, 1);
        append_live_pair(&mut fixture, 2);
        let stage_25 = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(stage_25.canary_percent, 25);
        let still_25 = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(still_25.canary_percent, 25);

        append_live_pair(&mut fixture, 3);
        append_live_pair(&mut fixture, 4);
        let stage_50 = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(stage_50.canary_percent, 50);
        append_live_pair(&mut fixture, 5);
        append_live_pair(&mut fixture, 6);
        let promoted = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(promoted.status, "promoted");
        assert_eq!(promoted.stable_profile_id, fixture.auto_child.id);
        assert!(promoted.canary_profile_id.is_none());
        let frozen = promoted.frozen_profile.unwrap();
        frozen.validate().unwrap();
        assert_eq!(
            frozen.evolution_method,
            PromptEvolutionMethod::ProToAutoDistillation
        );
        assert!(frozen.pro_teacher_evidence.is_some());
    }

    #[test]
    fn distillation_canary_waits_for_shared_task_classes() {
        let mut fixture = certified_distillation_fixture();
        let started = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(started.canary_percent, 10);
        append_live_pair(&mut fixture, 1);
        append_live_pair(&mut fixture, 2);
        for (_, observation) in &mut fixture.model.observations {
            if observation.mode == PromptEvaluationMode::Live
                && observation.profile_id == fixture.auto_parent.id
            {
                observation.task_class = "research".to_string();
            }
        }

        let pending = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);

        assert_eq!(pending.status, "canary");
        assert_eq!(pending.canary_percent, 10);
        assert!(pending.quarantined_profile_ids.is_empty());
    }

    #[test]
    fn distillation_canary_compares_resource_averages_with_unequal_samples() {
        let mut fixture = certified_distillation_fixture();
        let started = reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);
        assert_eq!(started.canary_percent, 10);
        append_live_pair(&mut fixture, 1);
        append_live_pair(&mut fixture, 2);
        for (_, observation) in &mut fixture.model.observations {
            if observation.mode == PromptEvaluationMode::Live
                && observation.profile_id == fixture.auto_child.id
            {
                observation.latency_ms = 190;
            }
        }
        let parent_template = fixture
            .model
            .observations
            .iter()
            .find(|(_, observation)| {
                observation.mode == PromptEvaluationMode::Live
                    && observation.profile_id == fixture.auto_parent.id
            })
            .cloned()
            .unwrap();
        for index in 3..=4 {
            let mut extra = parent_template.clone();
            extra.1.evaluation_id = format!("extra-parent-live-{index}");
            extra.1.case_id = format!("extra-parent-live-case-{index}");
            fixture.model.observations.push(extra);
        }

        let rolled_back =
            reconcile_prompt_rollout(&mut fixture.model, "auto", &fixture.evaluation);

        assert_eq!(rolled_back.status, "rolled_back");
        assert_eq!(
            rolled_back.last_reason.as_deref(),
            Some("canary_latency_regression")
        );
        assert_eq!(
            rolled_back.quarantined_profile_ids,
            vec![fixture.auto_child.id.clone()]
        );
    }
}
