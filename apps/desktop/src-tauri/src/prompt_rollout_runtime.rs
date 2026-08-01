use super::*;

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
        25..=49 => 50,
        _ => 100,
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

fn frozen_prompt_profile_for_promotion(
    model: &PromptEvolutionReadModel,
    effort: &str,
    candidate_id: &str,
    stable_profile_id: &str,
) -> Result<Option<FrozenPromptProfileSnapshot>, String> {
    let Some(record) = model
        .genomes
        .iter()
        .find(|record| record.effort == effort && record.genome.id == candidate_id)
    else {
        return Ok(None);
    };
    if record.evolution_method != Some(PromptEvolutionMethod::GepaReflectivePaired) {
        return Ok(None);
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
    let Some(dataset_sha256) =
        orchestrator::latest_scientific_dataset_digest(&observations).map(str::to_string)
    else {
        return Ok(None);
    };
    let observations = observations
        .into_iter()
        .filter(|observation| observation.provenance.dataset_sha256 == dataset_sha256)
        .collect::<Vec<_>>();
    let gate = evaluate_prompt_promotion_gate(
        &observations,
        candidate_id,
        stable_profile_id,
        prompt_promotion_gate_config(),
    );
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
        return Ok(Some(snapshot));
    }

    let (auto_profile, auto_profile_sha256) = stable_prompt_profile_fingerprint(model, "auto")?;
    let transfer_observations = model
        .observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort
                && observation.is_scientific_transfer_evidence()
                && ((observation.profile_id == candidate_id
                    && observation.opponent_profile_id.as_deref()
                        == Some(auto_profile.id.as_str()))
                    || (observation.profile_id == auto_profile.id
                        && observation.opponent_profile_id.as_deref() == Some(candidate_id)))
        })
        .map(|(_, observation)| observation.clone())
        .collect::<Vec<_>>();
    let transfer_gate = evaluate_prompt_auto_transfer_gate(
        &transfer_observations,
        candidate_id,
        &auto_profile.id,
        &auto_profile_sha256,
        prompt_promotion_gate_config(),
    );
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
        .rev()
        .find(|observation| observation.is_scientific_transfer_evidence())
        .map(|observation| observation.provenance.dataset_sha256.clone())
        .ok_or_else(|| "Auto transfer evidence has no active dataset".to_string())?;
    let mut transfer_evidence = transfer_observations
        .into_iter()
        .filter(|observation| observation.provenance.dataset_sha256 == transfer_dataset_sha256)
        .collect::<Vec<_>>();
    transfer_evidence.sort_by_key(PromptEvolutionObservation::evidence_identity);
    let transfer_evidence_sha256 = sha256_hex(
        &serde_json::to_vec(&transfer_evidence)
            .map_err(|error| format!("Auto transfer evidence serialization failed: {error}"))?,
    );
    snapshot
        .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: auto_profile.id,
            source_profile_sha256: auto_profile_sha256,
            dataset_sha256: transfer_dataset_sha256,
            paired_evidence_sha256: transfer_evidence_sha256,
            promotion_gate_protocol: orchestrator::PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
        })
        .map(Some)
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
    let Some(candidate_id) = evaluation.champion_id.as_deref() else {
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
    let promotion_gate = evaluate_prompt_promotion_gate(
        &effort_observations,
        candidate_id,
        &rollout.stable_profile_id,
        prompt_promotion_gate_config(),
    );
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
                    rollout.status = "evaluating".to_string();
                    rollout.last_reason = Some(format!("auto_transfer_gate_pending:{error}"));
                    model.rollouts.insert(effort.to_string(), rollout.clone());
                    return rollout;
                }
            };
        Some(evaluate_prompt_auto_transfer_gate(
            &effort_observations,
            candidate_id,
            &auto_profile.id,
            &auto_profile_sha256,
            prompt_promotion_gate_config(),
        ))
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
                rollout.last_reason =
                    Some(format!("auto_transfer_gate_regressed:{blocker}"));
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

    if let Some(reason) =
        prompt_canary_degraded(model, effort, &rollout.stable_profile_id, candidate_id)
    {
        rollout.canary_profile_id = None;
        rollout.canary_percent = 0;
        rollout.rollback_count = rollout.rollback_count.saturating_add(1);
        rollout.status = "rolled_back".to_string();
        rollout.last_reason = Some(reason);
        model.rollouts.insert(effort.to_string(), rollout.clone());
        return rollout;
    }

    let live_runs = prompt_live_observations(model, effort, candidate_id).len();
    let enough_new_evidence = confidence
        .comparisons
        .saturating_sub(rollout.evidence_checkpoint)
        >= 2;
    let enough_live_traffic = live_runs.saturating_sub(rollout.live_checkpoint) >= 1;
    if enough_new_evidence && enough_live_traffic {
        if rollout.canary_percent >= 100 {
            let frozen_profile = match frozen_prompt_profile_for_promotion(
                model,
                effort,
                candidate_id,
                &rollout.stable_profile_id,
            ) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    rollout.status = "evaluating".to_string();
                    rollout.last_reason = Some(format!("freeze_failed:{error}"));
                    model.rollouts.insert(effort.to_string(), rollout.clone());
                    return rollout;
                }
            };
            rollout.stable_profile_id = candidate_id.to_string();
            rollout.frozen_profile = frozen_profile;
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
    let canary_selected = rollout
        .canary_profile_id
        .as_ref()
        .is_some_and(|_| prompt_rollout_bucket(&rollout_key) < rollout.canary_percent);
    let selected_id = if canary_selected {
        rollout.canary_profile_id.as_deref()
    } else {
        Some(rollout.stable_profile_id.as_str())
    };
    let frozen_stable = (!canary_selected)
        .then_some(rollout.frozen_profile.as_ref())
        .flatten()
        .filter(|snapshot| {
            snapshot.validate().is_ok()
                && snapshot.effort == effort
                && snapshot.genome.id == rollout.stable_profile_id
        })
        .map(|snapshot| snapshot.genome.clone());
    let selected = frozen_stable
        .or_else(|| {
            selected_id.and_then(|id| {
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
            })
        })
        .unwrap_or_else(|| ConductorPromptGenome::seed_for_effort(effort));
    evaluation.next_profile = selected;
    evaluation.next_mode = if canary_selected {
        format!("canary_{}", rollout.canary_percent)
    } else {
        "stable".to_string()
    };
    evaluation.status = rollout.status.clone();
}
