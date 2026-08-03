use super::*;

pub(crate) fn prompt_attempt_matches_profiles(
    attempt: &PromptEvaluationAttemptState,
    candidate_id: &str,
    stable_id: &str,
) -> bool {
    let profiles = attempt
        .started
        .treatments
        .iter()
        .map(|treatment| treatment.profile_id.as_str())
        .collect::<BTreeSet<_>>();
    profiles == BTreeSet::from([candidate_id, stable_id])
}

fn prompt_attempt_has_complete_pair(
    observations: &[(String, PromptEvolutionObservation)],
    attempt: &PromptEvaluationAttemptState,
    transfer: bool,
) -> bool {
    observations
        .iter()
        .filter(|(_, observation)| {
            (if transfer {
                observation.is_strict_source_attested_transfer_evidence()
            } else {
                observation.is_strict_matched_evidence()
            }) && crate::prompt_attempt_runtime::prompt_observation_matches_attempt(
                observation,
                &attempt.started,
            )
        })
        .count()
        == 2
}

pub(crate) fn prompt_active_pair_cohort(
    model: &PromptEvolutionReadModel,
    candidate_id: &str,
    stable_id: &str,
) -> Option<String> {
    model
        .attempts
        .values()
        .filter(|attempt| prompt_attempt_matches_profiles(attempt, candidate_id, stable_id))
        .filter_map(|attempt| {
            model
                .cohort_sequences
                .get(&attempt.started.identity.cohort_sha256)
                .map(|sequence| (sequence, attempt.started.identity.cohort_sha256.as_str()))
        })
        .max_by(
            |(left_sequence, left_digest), (right_sequence, right_digest)| {
                left_sequence
                    .cmp(right_sequence)
                    .then_with(|| left_digest.cmp(right_digest))
            },
        )
        .map(|(_, digest)| digest.to_string())
}

fn evaluate_trusted_prompt_promotion_gate(
    model: &PromptEvolutionReadModel,
    observations: &[PromptEvolutionObservation],
    candidate_id: &str,
    stable_id: &str,
) -> (PromptPromotionGateResult, Option<String>) {
    let active_cohort = prompt_active_pair_cohort(model, candidate_id, stable_id);
    let failures = active_cohort
        .as_deref()
        .map(|cohort_sha256| {
            prompt_promotion_failure_penalties(model, cohort_sha256, candidate_id, stable_id, false)
        })
        .unwrap_or_default();
    let result = evaluate_prompt_promotion_gate_with_failures_in_cohort(
        observations,
        &failures,
        candidate_id,
        stable_id,
        active_cohort.as_deref().unwrap_or("missing-matched-cohort"),
        prompt_promotion_gate_config(),
    );
    (result, active_cohort)
}

fn prompt_promotion_failure_penalties(
    model: &PromptEvolutionReadModel,
    cohort_sha256: &str,
    candidate_id: &str,
    stable_id: &str,
    transfer: bool,
) -> Vec<PromptPromotionFailurePenalty> {
    let Some(cohort) = model.cohorts.get(cohort_sha256) else {
        return Vec::new();
    };
    model
        .attempts
        .values()
        .filter(|attempt| attempt.started.identity.cohort_sha256 == cohort_sha256)
        .filter(|attempt| prompt_attempt_matches_profiles(attempt, candidate_id, stable_id))
        .filter(|attempt| {
            attempt.terminal.as_ref().is_some_and(|terminal| {
                terminal.status == PromptEvaluationAttemptStatus::TreatmentFailure
            }) || !prompt_attempt_has_complete_pair(&model.observations, attempt, transfer)
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
            let stable_index = attempt
                .started
                .treatments
                .iter()
                .position(|treatment| treatment.profile_id == stable_id)?;
            Some(PromptPromotionFailurePenalty {
                evaluation_id: attempt.started.identity.evaluation_id.clone(),
                cohort_sha256: cohort_sha256.to_string(),
                case_id: case.case_id.clone(),
                task_class: case.task_family_sha256.clone(),
                split: case.split,
                candidate_profile_id: candidate_id.to_string(),
                stable_profile_id: stable_id.to_string(),
                candidate_failed: treatment_failures[candidate_index],
                stable_failed: treatment_failures[stable_index],
            })
        })
        .collect()
}

pub(crate) fn evaluate_trusted_prompt_auto_transfer_gate(
    model: &PromptEvolutionReadModel,
    observations: &[PromptEvolutionObservation],
    candidate_id: &str,
    auto_profile_id: &str,
    auto_profile_sha256: &str,
) -> (PromptPromotionGateResult, Option<String>) {
    let active_cohort = prompt_active_pair_cohort(model, candidate_id, auto_profile_id);
    let failures = active_cohort
        .as_deref()
        .map(|cohort_sha256| {
            prompt_promotion_failure_penalties(
                model,
                cohort_sha256,
                candidate_id,
                auto_profile_id,
                true,
            )
        })
        .unwrap_or_default();
    let result = evaluate_prompt_auto_transfer_gate_in_cohort_with_failures(
        observations,
        &failures,
        candidate_id,
        auto_profile_id,
        auto_profile_sha256,
        active_cohort.as_deref().unwrap_or("missing-matched-cohort"),
        prompt_promotion_gate_config(),
    );
    (result, active_cohort)
}

pub(crate) fn evaluate_prompt_evolution_read_model(
    model: &PromptEvolutionReadModel,
    effort: &str,
) -> Result<PromptEvolutionEvaluation, String> {
    evaluate_prompt_evolution_with_observations(
        &prompt_evolution_profile_events(model),
        effort,
        &model.observations,
    )
}

pub(crate) fn default_prompt_rollout(effort: &str) -> PromptRolloutState {
    PromptRolloutState {
        stable_profile_id: ConductorPromptGenome::seed_for_effort(effort).id,
        canary_profile_id: None,
        canary_percent: 0,
        evidence_checkpoint: 0,
        live_checkpoint: 0,
        rollback_count: 0,
        status: "stable".to_string(),
        last_reason: None,
        promotion_confidence: None,
        frozen_profile: None,
    }
}

pub(crate) fn prompt_live_observations<'a>(
    model: &'a PromptEvolutionReadModel,
    effort: &str,
    profile_id: &str,
) -> Vec<&'a PromptEvolutionObservation> {
    let mut seen = BTreeSet::new();
    model
        .observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort
                && observation.profile_id == profile_id
                && observation.mode == PromptEvaluationMode::Live
        })
        .map(|(_, observation)| observation)
        .filter(|observation| seen.insert(observation.evidence_identity()))
        .collect()
}

pub(crate) fn prompt_canary_degraded(
    model: &PromptEvolutionReadModel,
    effort: &str,
    stable_profile_id: &str,
    canary_profile_id: &str,
) -> Option<String> {
    let canary = prompt_live_observations(model, effort, canary_profile_id);
    if canary
        .iter()
        .rev()
        .take(4)
        .any(|observation| observation.safety_violations > 0 || !observation.format_valid)
    {
        return Some("canary_safety_regression".to_string());
    }
    let recent_canary = canary.iter().rev().take(4).copied().collect::<Vec<_>>();
    if recent_canary.len() >= 2 {
        let success_rate = recent_canary
            .iter()
            .filter(|observation| observation.succeeded)
            .count() as f64
            / recent_canary.len() as f64;
        if success_rate < 0.5 {
            return Some("canary_success_regression".to_string());
        }
    }
    let stable = prompt_live_observations(model, effort, stable_profile_id);
    let recent_stable = stable.iter().rev().take(4).copied().collect::<Vec<_>>();
    if recent_canary.len() >= 2 && recent_stable.len() >= 2 {
        let average = |entries: &[&PromptEvolutionObservation]| {
            entries.iter().map(|entry| entry.reward()).sum::<f64>() / entries.len() as f64
        };
        if average(&recent_canary) + 0.08 < average(&recent_stable) {
            return Some("canary_reward_regression".to_string());
        }
    }
    None
}

pub(crate) fn next_prompt_canary_stage(current: u8) -> u8 {
    match current {
        0..=9 => 10,
        10..=24 => 25,
        _ => 50,
    }
}

pub(crate) fn prompt_promotion_gate_config() -> PromptPromotionGateConfig {
    PromptPromotionGateConfig {
        minimum_train_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        minimum_holdout_runs: PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
        minimum_unique_train_cases: 4,
        minimum_unique_holdout_cases: 4,
        minimum_train_task_classes: 2,
        minimum_holdout_task_classes: 2,
        minimum_wilson_lower_bound: PROMPT_EVOLUTION_MIN_PROMOTION_WILSON,
        maximum_generalization_gap: 0.15,
        maximum_holdout_task_class_regression: 0.05,
    }
}

pub(crate) fn stable_prompt_profile_fingerprint(
    model: &PromptEvolutionReadModel,
    effort: &str,
) -> Result<(ConductorPromptGenome, String), String> {
    let default = ConductorPromptGenome::seed_for_effort(effort);
    let stable_id = model
        .rollouts
        .get(effort)
        .map(|rollout| rollout.stable_profile_id.clone())
        .unwrap_or_else(|| default.id.clone());
    let frozen = model
        .rollouts
        .get(effort)
        .and_then(|rollout| rollout.frozen_profile.as_ref())
        .filter(|snapshot| {
            snapshot.validate().is_ok()
                && snapshot.effort == effort
                && snapshot.genome.id == stable_id
        })
        .map(|snapshot| snapshot.genome.clone());
    let genome = frozen
        .or_else(|| {
            model
                .genomes
                .iter()
                .find(|record| record.effort == effort && record.genome.id == stable_id)
                .map(|record| record.genome.clone())
        })
        .or_else(|| (default.id == stable_id).then_some(default))
        .ok_or_else(|| format!("stable {effort} prompt profile {stable_id} is unavailable"))?;
    let fingerprint = prompt_genome_sha256(&genome)?;
    Ok((genome, fingerprint))
}

fn frozen_auto_source_profile_lineage(
    model: &PromptEvolutionReadModel,
    auto_profile: &ConductorPromptGenome,
    auto_profile_sha256: &str,
) -> Result<orchestrator::FrozenPromptSourceProfileLineageV1, String> {
    if prompt_genome_sha256(auto_profile)? != auto_profile_sha256 {
        return Err("Auto source profile fingerprint does not match".to_string());
    }
    if auto_profile.id == ConductorPromptGenome::seed_for_effort("auto").id {
        return orchestrator::FrozenPromptSourceProfileLineageV1::undistilled(
            auto_profile_sha256.to_string(),
        );
    }

    let mut canonical = model
        .rollouts
        .values()
        .filter_map(|rollout| rollout.frozen_profile.as_ref())
        .filter(|snapshot| {
            snapshot.validate().is_ok()
                && snapshot.effort == "auto"
                && snapshot.genome.id == auto_profile.id
                && snapshot.candidate_sha256 == auto_profile_sha256
        })
        .map(|snapshot| {
            snapshot.pro_teacher_evidence.as_ref().map_or_else(
                || {
                    orchestrator::FrozenPromptSourceProfileLineageV1::undistilled(
                        auto_profile_sha256.to_string(),
                    )
                },
                |evidence| {
                    orchestrator::FrozenPromptSourceProfileLineageV1::from_distillation(
                        auto_profile_sha256.to_string(),
                        evidence,
                    )
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    canonical.sort_by(|left, right| left.lineage_sha256.cmp(&right.lineage_sha256));
    canonical.dedup_by(|left, right| left.lineage_sha256 == right.lineage_sha256);
    match canonical.as_slice() {
        [lineage] => Ok(lineage.clone()),
        [] => Err("non-seed Auto transfer source has no canonical frozen lineage".to_string()),
        _ => Err("Auto transfer source has ambiguous frozen lineage".to_string()),
    }
}

pub(crate) fn frozen_prompt_profile_for_promotion(
    model: &PromptEvolutionReadModel,
    effort: &str,
    candidate_id: &str,
    stable_profile_id: &str,
) -> Result<FrozenPromptProfileSnapshot, String> {
    let Some(record) = model
        .genomes
        .iter()
        .find(|record| record.effort == effort && record.genome.id == candidate_id)
    else {
        return Err(format!(
            "GEPA promotion profile {candidate_id} is unavailable"
        ));
    };
    if record.evolution_method == Some(PromptEvolutionMethod::ProToAutoDistillation) {
        if effort != "auto" {
            return Err("Pro-to-Auto distillation can only promote Auto".to_string());
        }
        return crate::prompt_distillation_rollout::frozen_distillation_profile_for_promotion(
            model,
            candidate_id,
            stable_profile_id,
        );
    }
    if record.evolution_method != Some(PromptEvolutionMethod::GepaReflectivePaired) {
        return Err(format!(
            "profile {candidate_id} is not a GEPA reflective paired candidate"
        ));
    }

    let observations = model
        .observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort
                && observation.mode.is_execution()
                && observation.is_scientific_evidence()
                && ((observation.profile_id == candidate_id
                    && observation.opponent_profile_id.as_deref() == Some(stable_profile_id))
                    || (observation.profile_id == stable_profile_id
                        && observation.opponent_profile_id.as_deref() == Some(candidate_id)))
        })
        .map(|(_, observation)| observation.clone())
        .collect::<Vec<_>>();
    let (gate, active_cohort) = evaluate_trusted_prompt_promotion_gate(
        model,
        &observations,
        candidate_id,
        stable_profile_id,
    );
    let Some(cohort_sha256) = active_cohort else {
        return Err("active GEPA cohort is unavailable".to_string());
    };
    let observations = observations
        .into_iter()
        .filter(|observation| {
            observation.scientific_cohort_sha256() == Some(cohort_sha256.as_str())
        })
        .collect::<Vec<_>>();
    let dataset_sha256 = observations
        .first()
        .map(|observation| observation.provenance.dataset_sha256.clone())
        .ok_or_else(|| "active GEPA cohort has no evidence".to_string())?;
    if !gate.eligible {
        let blockers = gate
            .blockers
            .iter()
            .map(|blocker| blocker.label())
            .collect::<Vec<_>>()
            .join(",");
        return Err(format!(
            "cannot freeze GEPA profile before the promotion gate passes: {blockers}"
        ));
    }

    let candidate_prompt_sha256 = serde_json::to_vec(&record.genome)
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("promotion genome serialization failed: {error}"))?;
    if observations.iter().any(|observation| {
        observation.profile_id == candidate_id
            && observation.provenance.candidate_prompt_sha256 != candidate_prompt_sha256
    }) {
        return Err("cannot freeze GEPA profile with mismatched prompt lineage".to_string());
    }
    let mut evidence = observations;
    evidence.sort_by_key(PromptEvolutionObservation::evidence_identity);
    let paired_evidence_sha256 = sha256_hex(
        &serde_json::to_vec(&evidence)
            .map_err(|error| format!("promotion evidence serialization failed: {error}"))?,
    );
    let snapshot = FrozenPromptProfileSnapshot::new_gepa(
        effort,
        record.genome.clone(),
        stable_profile_id,
        dataset_sha256,
        paired_evidence_sha256,
    )?;
    if effort != "pro" {
        return Ok(snapshot);
    }

    let (auto_profile, auto_profile_sha256) = stable_prompt_profile_fingerprint(model, "auto")?;
    let transfer_observations = model
        .observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort
                && observation.is_strict_source_attested_transfer_evidence()
                && ((observation.profile_id == candidate_id
                    && observation.opponent_profile_id.as_deref()
                        == Some(auto_profile.id.as_str()))
                    || (observation.profile_id == auto_profile.id
                        && observation.opponent_profile_id.as_deref() == Some(candidate_id)))
        })
        .map(|(_, observation)| observation.clone())
        .collect::<Vec<_>>();
    let (transfer_gate, active_transfer_cohort) = evaluate_trusted_prompt_auto_transfer_gate(
        model,
        &transfer_observations,
        candidate_id,
        &auto_profile.id,
        &auto_profile_sha256,
    );
    let active_transfer_cohort = active_transfer_cohort
        .ok_or_else(|| "Auto transfer evidence has no active matched cohort".to_string())?;
    if !transfer_gate.eligible {
        let blockers = transfer_gate
            .blockers
            .iter()
            .map(|blocker| blocker.label())
            .collect::<Vec<_>>()
            .join(",");
        return Err(format!(
            "cannot freeze GEPA Pro profile before the Auto transfer gate passes: {blockers}"
        ));
    }
    let transfer_dataset_sha256 = transfer_observations
        .iter()
        .find(|observation| {
            observation.scientific_cohort_sha256() == Some(active_transfer_cohort.as_str())
        })
        .map(|observation| observation.provenance.dataset_sha256.clone())
        .ok_or_else(|| "Auto transfer evidence has no active dataset".to_string())?;
    let mut transfer_evidence = transfer_observations
        .into_iter()
        .filter(|observation| {
            observation.scientific_cohort_sha256() == Some(active_transfer_cohort.as_str())
        })
        .collect::<Vec<_>>();
    transfer_evidence.sort_by_key(PromptEvolutionObservation::evidence_identity);
    let transfer_evidence_sha256 = sha256_hex(
        &serde_json::to_vec(&transfer_evidence)
            .map_err(|error| format!("Auto transfer evidence serialization failed: {error}"))?,
    );
    let source_profile_lineage =
        frozen_auto_source_profile_lineage(model, &auto_profile, &auto_profile_sha256)?;
    snapshot.with_auto_teacher_evidence(FrozenPromptTransferEvidence {
        source_effort: "auto".to_string(),
        source_profile_id: auto_profile.id,
        source_profile_sha256: auto_profile_sha256,
        dataset_sha256: transfer_dataset_sha256,
        cohort_sha256: Some(active_transfer_cohort),
        paired_evidence_sha256: transfer_evidence_sha256,
        promotion_gate_protocol: orchestrator::PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
        source_profile_lineage: Some(source_profile_lineage),
    })
}

pub(crate) fn prompt_rollout_transition_has_canonical_evidence(
    model: &PromptEvolutionReadModel,
    effort: &str,
    previous: Option<&PromptRolloutState>,
    next: &PromptRolloutState,
) -> bool {
    match next.status.as_str() {
        "canary" => next
            .canary_profile_id
            .as_deref()
            .is_some_and(|candidate_id| {
                frozen_prompt_profile_for_promotion(
                    model,
                    effort,
                    candidate_id,
                    &next.stable_profile_id,
                )
                .is_ok()
            }),
        "promoted" => {
            let seed_profile_id = ConductorPromptGenome::seed_for_effort(effort).id;
            let stable_profile_id = previous
                .map(|rollout| rollout.stable_profile_id.as_str())
                .unwrap_or(seed_profile_id.as_str());
            next.frozen_profile.as_ref().is_some_and(|snapshot| {
                frozen_prompt_profile_for_promotion(
                    model,
                    effort,
                    &next.stable_profile_id,
                    stable_profile_id,
                )
                .is_ok_and(|expected| expected == *snapshot)
            })
        }
        _ => true,
    }
}

pub(crate) fn reconcile_prompt_rollout(
    model: &mut PromptEvolutionReadModel,
    effort: &str,
    evaluation: &PromptEvolutionEvaluation,
) -> PromptRolloutState {
    let mut rollout = model
        .rollouts
        .get(effort)
        .cloned()
        .unwrap_or_else(|| default_prompt_rollout(effort));
    let distillation_candidate = if effort == "auto" {
        rollout
            .canary_profile_id
            .as_ref()
            .filter(|candidate_id| {
                crate::prompt_distillation_rollout::prompt_candidate_is_distillation(
                    model,
                    effort,
                    candidate_id,
                )
            })
            .cloned()
            .or_else(|| {
                let candidate_id =
                    crate::prompt_distillation_rollout::latest_prompt_distillation_candidate(
                        model,
                        &rollout.stable_profile_id,
                    )?;
                crate::prompt_distillation_rollout::evaluate_trusted_prompt_distillation_gate(
                    model,
                    &candidate_id,
                    &rollout.stable_profile_id,
                )
                .ok()
                .filter(|trusted| trusted.result.eligible)
                .map(|_| candidate_id)
            })
    } else {
        None
    };
    let candidate_id = distillation_candidate
        .as_deref()
        .or(evaluation.champion_id.as_deref());
    let Some(candidate_id) = candidate_id else {
        model.rollouts.insert(effort.to_string(), rollout.clone());
        return rollout;
    };
    if candidate_id == rollout.stable_profile_id {
        if rollout.canary_profile_id.is_some() {
            rollout.canary_profile_id = None;
            rollout.canary_percent = 0;
            rollout.status = "rolled_back".to_string();
            rollout.last_reason = Some("stable_profile_regained_frontier".to_string());
            rollout.rollback_count = rollout.rollback_count.saturating_add(1);
        } else {
            rollout.status = "stable".to_string();
        }
        model.rollouts.insert(effort.to_string(), rollout.clone());
        return rollout;
    }

    let effort_observations = model
        .observations
        .iter()
        .filter(|(observed_effort, _)| observed_effort == effort)
        .map(|(_, observation)| observation.clone())
        .collect::<Vec<_>>();
    let candidate_is_distillation =
        crate::prompt_distillation_rollout::prompt_candidate_is_distillation(
            model,
            effort,
            candidate_id,
        );
    let promotion_gate = if candidate_is_distillation {
        match crate::prompt_distillation_rollout::evaluate_trusted_prompt_distillation_gate(
            model,
            candidate_id,
            &rollout.stable_profile_id,
        ) {
            Ok(trusted) => trusted.result,
            Err(error) => {
                if rollout.canary_profile_id.as_deref() == Some(candidate_id) {
                    rollout.canary_profile_id = None;
                    rollout.canary_percent = 0;
                    rollout.rollback_count = rollout.rollback_count.saturating_add(1);
                    rollout.status = "rolled_back".to_string();
                    rollout.last_reason = Some(format!("distillation_gate_regressed:{error}"));
                } else {
                    rollout.status = "evaluating".to_string();
                    rollout.last_reason = Some(format!("distillation_gate_pending:{error}"));
                }
                model.rollouts.insert(effort.to_string(), rollout.clone());
                return rollout;
            }
        }
    } else {
        evaluate_trusted_prompt_promotion_gate(
            model,
            &effort_observations,
            candidate_id,
            &rollout.stable_profile_id,
        )
        .0
    };
    let confidence = &promotion_gate.confidence;
    rollout.promotion_confidence = Some(confidence.wilson_lower_bound);
    if !promotion_gate.eligible {
        let blocker = promotion_gate
            .blockers
            .first()
            .map(|blocker| blocker.label())
            .unwrap_or("unknown");
        if rollout.canary_profile_id.as_deref() == Some(candidate_id) {
            rollout.canary_profile_id = None;
            rollout.canary_percent = 0;
            rollout.rollback_count = rollout.rollback_count.saturating_add(1);
            rollout.status = "rolled_back".to_string();
            rollout.last_reason = Some(format!("promotion_gate_regressed:{blocker}"));
        } else {
            rollout.status = "evaluating".to_string();
            rollout.last_reason = Some(format!("promotion_gate_pending:{blocker}"));
        }
        model.rollouts.insert(effort.to_string(), rollout.clone());
        return rollout;
    }

    let transfer_gate = if effort == "pro" {
        let (auto_profile, auto_profile_sha256) =
            match stable_prompt_profile_fingerprint(model, "auto") {
                Ok(profile) => profile,
                Err(error) => {
                    if rollout.canary_profile_id.as_deref() == Some(candidate_id) {
                        rollout.canary_profile_id = None;
                        rollout.canary_percent = 0;
                        rollout.rollback_count = rollout.rollback_count.saturating_add(1);
                        rollout.status = "rolled_back".to_string();
                        rollout.last_reason = Some(format!("auto_transfer_gate_regressed:{error}"));
                    } else {
                        rollout.status = "evaluating".to_string();
                        rollout.last_reason = Some(format!("auto_transfer_gate_pending:{error}"));
                    }
                    model.rollouts.insert(effort.to_string(), rollout.clone());
                    return rollout;
                }
            };
        Some(
            evaluate_trusted_prompt_auto_transfer_gate(
                model,
                &effort_observations,
                candidate_id,
                &auto_profile.id,
                &auto_profile_sha256,
            )
            .0,
        )
    } else {
        None
    };
    if let Some(transfer_gate) = transfer_gate.as_ref() {
        rollout.promotion_confidence = Some(
            confidence
                .wilson_lower_bound
                .min(transfer_gate.confidence.wilson_lower_bound),
        );
        if !transfer_gate.eligible {
            let blocker = transfer_gate
                .blockers
                .first()
                .map(|blocker| blocker.label())
                .unwrap_or("unknown");
            if rollout.canary_profile_id.as_deref() == Some(candidate_id) {
                rollout.canary_profile_id = None;
                rollout.canary_percent = 0;
                rollout.rollback_count = rollout.rollback_count.saturating_add(1);
                rollout.status = "rolled_back".to_string();
                rollout.last_reason = Some(format!("auto_transfer_gate_regressed:{blocker}"));
            } else {
                rollout.status = "evaluating".to_string();
                rollout.last_reason = Some(format!("auto_transfer_gate_pending:{blocker}"));
            }
            model.rollouts.insert(effort.to_string(), rollout.clone());
            return rollout;
        }
    }

    if rollout.canary_profile_id.as_deref() != Some(candidate_id) {
        rollout.canary_profile_id = Some(candidate_id.to_string());
        rollout.canary_percent = 10;
        rollout.evidence_checkpoint = confidence.comparisons;
        rollout.live_checkpoint = prompt_live_observations(model, effort, candidate_id).len();
        rollout.status = "canary".to_string();
        rollout.last_reason = Some("confidence_gate_passed".to_string());
        model.rollouts.insert(effort.to_string(), rollout.clone());
        return rollout;
    }

    let canary_degradation =
        prompt_canary_degraded(model, effort, &rollout.stable_profile_id, candidate_id).or_else(
            || {
                if candidate_is_distillation {
                    crate::prompt_distillation_rollout::prompt_distillation_canary_degraded(
                        model,
                        &rollout.stable_profile_id,
                        candidate_id,
                    )
                } else {
                    None
                }
            },
        );
    if let Some(reason) = canary_degradation {
        rollout.canary_profile_id = None;
        rollout.canary_percent = 0;
        rollout.rollback_count = rollout.rollback_count.saturating_add(1);
        rollout.status = "rolled_back".to_string();
        rollout.last_reason = Some(reason);
        model.rollouts.insert(effort.to_string(), rollout.clone());
        return rollout;
    }

    let live_runs = prompt_live_observations(model, effort, candidate_id).len();
    // Distillation campaigns are terminal once their matched gate passes; later stages
    // revalidate that frozen gate and require fresh live canary traffic instead.
    let enough_new_evidence = candidate_is_distillation
        || confidence
            .comparisons
            .saturating_sub(rollout.evidence_checkpoint)
            >= 2;
    let enough_live_traffic = live_runs.saturating_sub(rollout.live_checkpoint) >= 1;
    if enough_new_evidence && enough_live_traffic {
        if rollout.canary_percent >= 50 {
            let frozen_profile = match frozen_prompt_profile_for_promotion(
                model,
                effort,
                candidate_id,
                &rollout.stable_profile_id,
            ) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    rollout.canary_profile_id = None;
                    rollout.canary_percent = 0;
                    rollout.rollback_count = rollout.rollback_count.saturating_add(1);
                    rollout.status = "rolled_back".to_string();
                    rollout.last_reason = Some(format!("freeze_failed:{error}"));
                    model.rollouts.insert(effort.to_string(), rollout.clone());
                    return rollout;
                }
            };
            rollout.stable_profile_id = candidate_id.to_string();
            rollout.frozen_profile = Some(frozen_profile);
            rollout.canary_profile_id = None;
            rollout.canary_percent = 0;
            rollout.status = "promoted".to_string();
            rollout.last_reason = Some("canary_completed".to_string());
        } else {
            rollout.canary_percent = next_prompt_canary_stage(rollout.canary_percent);
            rollout.evidence_checkpoint = confidence.comparisons;
            rollout.live_checkpoint = live_runs;
            rollout.status = "canary".to_string();
            rollout.last_reason = Some("canary_stage_advanced".to_string());
        }
    }
    model.rollouts.insert(effort.to_string(), rollout.clone());
    rollout
}

pub(crate) fn prompt_rollout_bucket(value: &str) -> u8 {
    let hash = value
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    (hash % 100) as u8
}

pub(crate) fn apply_prompt_rollout_selection(
    evaluation: &mut PromptEvolutionEvaluation,
    rollout: &PromptRolloutState,
    model: &PromptEvolutionReadModel,
    run_context: &Metadata,
    effort: &str,
) {
    let rollout_key = format!(
        "{}:{}:{}",
        effort,
        run_context
            .get("session_id")
            .map(String::as_str)
            .unwrap_or("session"),
        run_context
            .get("agent_run_id")
            .map(String::as_str)
            .unwrap_or("run")
    );
    let effective_canary_percent = if rollout.canary_percent <= 50 {
        rollout.canary_percent
    } else {
        0
    };
    let canary_selected = rollout
        .canary_profile_id
        .as_ref()
        .is_some_and(|_| prompt_rollout_bucket(&rollout_key) < effective_canary_percent);
    let frozen_stable = rollout
        .frozen_profile
        .as_ref()
        .filter(|snapshot| {
            snapshot.validate().is_ok()
                && snapshot.effort == effort
                && snapshot.genome.id == rollout.stable_profile_id
        })
        .map(|snapshot| snapshot.genome.clone());
    let stable = frozen_stable
        .or_else(|| {
            evaluation
                .population
                .iter()
                .find(|profile| profile.id == rollout.stable_profile_id)
                .cloned()
                .or_else(|| {
                    model
                        .genomes
                        .iter()
                        .find(|record| {
                            record.effort == effort && record.genome.id == rollout.stable_profile_id
                        })
                        .map(|record| record.genome.clone())
                })
        })
        .unwrap_or_else(|| ConductorPromptGenome::seed_for_effort(effort));
    let canary = canary_selected
        .then(|| {
            rollout.canary_profile_id.as_deref().and_then(|id| {
                evaluation
                    .population
                    .iter()
                    .find(|profile| profile.id == id)
                    .cloned()
                    .or_else(|| {
                        model
                            .genomes
                            .iter()
                            .find(|record| record.effort == effort && record.genome.id == id)
                            .map(|record| record.genome.clone())
                    })
                    .filter(|profile| profile.validate().is_ok())
            })
        })
        .flatten();
    let canary_applied = canary.is_some();
    evaluation.next_profile = canary.unwrap_or(stable);
    evaluation.next_mode = if canary_applied {
        format!("canary_{effective_canary_percent}")
    } else {
        "stable".to_string()
    };
    evaluation.status = rollout.status.clone();
}
