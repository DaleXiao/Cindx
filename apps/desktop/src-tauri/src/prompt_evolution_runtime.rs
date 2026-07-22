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

pub(crate) fn prompt_direct_promotion_evidence<'a>(
    model: &'a PromptEvolutionReadModel,
    effort: &str,
    candidate_id: &str,
    stable_id: &str,
) -> Vec<&'a PromptEvolutionObservation> {
    let mut seen = BTreeSet::new();
    model
        .observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort
                && observation.profile_id == candidate_id
                && observation.opponent_profile_id.as_deref() == Some(stable_id)
                && observation.mode.is_execution()
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

    let direct_evidence =
        prompt_direct_promotion_evidence(model, effort, candidate_id, &rollout.stable_profile_id);
    let direct_train_runs = direct_evidence
        .iter()
        .filter(|observation| observation.mode.is_paired_execution())
        .count();
    let direct_holdout_runs = direct_evidence
        .iter()
        .filter(|observation| observation.mode.is_replay_execution())
        .count();
    let confidence = prompt_promotion_confidence(
        direct_evidence
            .iter()
            .copied()
            .filter(|observation| observation.mode.is_replay_execution()),
    );
    rollout.promotion_confidence = Some(confidence.wilson_lower_bound);
    if direct_train_runs < PROMPT_EVOLUTION_MIN_TRAIN_RUNS
        || direct_holdout_runs < PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS
        || confidence.wilson_lower_bound < PROMPT_EVOLUTION_MIN_PROMOTION_WILSON
    {
        if rollout.canary_profile_id.as_deref() == Some(candidate_id) {
            rollout.canary_profile_id = None;
            rollout.canary_percent = 0;
            rollout.rollback_count = rollout.rollback_count.saturating_add(1);
            rollout.status = "rolled_back".to_string();
            rollout.last_reason = Some("direct_stable_evidence_regressed".to_string());
        } else {
            rollout.status = "evaluating".to_string();
            rollout.last_reason = Some("direct_stable_evidence_pending".to_string());
        }
        model.rollouts.insert(effort.to_string(), rollout.clone());
        return rollout;
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
            rollout.stable_profile_id = candidate_id.to_string();
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
    let selected = selected_id
        .and_then(|id| {
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
        .unwrap_or_else(|| ConductorPromptGenome::seed_for_effort(effort));
    evaluation.next_profile = selected;
    evaluation.next_mode = if canary_selected {
        format!("canary_{}", rollout.canary_percent)
    } else {
        "stable".to_string()
    };
    evaluation.status = rollout.status.clone();
}

pub(crate) fn prompt_instance_pareto_scores(
    population: &[ConductorPromptGenome],
    observations: &[PromptEvolutionObservation],
) -> Vec<AgentEvaluationCaseScore> {
    let fingerprints = population
        .iter()
        .map(|genome| {
            let fingerprint = serde_json::to_vec(genome)
                .map(|encoded| sha256_hex(&encoded))
                .unwrap_or_else(|_| genome.id.clone());
            (genome.id.as_str(), fingerprint)
        })
        .collect::<BTreeMap<_, _>>();
    observations
        .iter()
        .filter(|observation| observation.mode == PromptEvaluationMode::ReplayExecution)
        .filter(|observation| !observation.case_id.trim().is_empty())
        .filter_map(|observation| {
            let fingerprint = fingerprints.get(observation.profile_id.as_str())?;
            let safe = observation.format_valid && observation.safety_violations == 0;
            let identity = observation.evidence_identity();
            let digest = sha256_hex(identity.as_bytes());
            let seed = u64::from_str_radix(&digest[..16], 16).unwrap_or_default();
            Some(AgentEvaluationCaseScore {
                suite_id: "runtime-prompt-evolution".to_string(),
                suite_version: 2,
                case_id: observation.case_id.clone(),
                category: observation.task_class.clone(),
                split: AgentEvaluationSplit::Pareto,
                run_id: observation.evaluation_id.clone(),
                seed,
                candidate_id: observation.profile_id.clone(),
                candidate_fingerprint: fingerprint.clone(),
                evidence_source: AgentEvaluationEvidenceSource::Judge,
                score: if safe {
                    observation.quality_score.clamp(0.0, 1.0)
                } else {
                    0.0
                },
                verified_success: safe && observation.succeeded,
                latency_ms: observation.latency_ms,
                total_tokens: observation.total_tokens,
                safety_violations: observation.safety_violations,
            })
        })
        .collect()
}

pub(crate) fn prompt_instance_merge_candidate(
    archive: &PromptInstanceParetoArchive,
    population: &[ConductorPromptGenome],
) -> Option<ConductorPromptGenome> {
    let by_id = population
        .iter()
        .map(|genome| (genome.id.as_str(), genome))
        .collect::<BTreeMap<_, _>>();
    for (left_index, left_candidate) in archive.candidates.iter().enumerate() {
        let Some(left) = by_id.get(left_candidate.profile_id.as_str()).copied() else {
            continue;
        };
        for right_candidate in archive.candidates.iter().skip(left_index + 1) {
            let Some(right) = by_id.get(right_candidate.profile_id.as_str()).copied() else {
                continue;
            };
            for ancestor_id in left
                .parents
                .iter()
                .filter(|parent| right.parents.iter().any(|candidate| candidate == *parent))
            {
                let Some(ancestor) = by_id.get(ancestor_id.as_str()).copied() else {
                    continue;
                };
                let merge_id = format!(
                    "merge-g{}-{}-{}",
                    left.generation.max(right.generation).saturating_add(1),
                    left.id,
                    right.id
                );
                if by_id.contains_key(merge_id.as_str()) {
                    continue;
                }
                if let Ok(merged) = archive.merge_complementary(merge_id, ancestor, left, right) {
                    return Some(merged);
                }
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn evaluate_prompt_evolution(
    events: &[Event],
    effort: &str,
) -> Result<PromptEvolutionEvaluation, String> {
    let observations = prompt_evolution_observations_from_events(events);
    evaluate_prompt_evolution_with_observations(events, effort, &observations)
}

pub(crate) fn evaluate_prompt_evolution_with_observations(
    events: &[Event],
    effort: &str,
    all_observations: &[(String, PromptEvolutionObservation)],
) -> Result<PromptEvolutionEvaluation, String> {
    let known_population = prompt_genomes_from_events(events, effort);
    let known_ids = known_population
        .iter()
        .map(|genome| genome.id.as_str())
        .collect::<BTreeSet<_>>();
    let observations = all_observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort && known_ids.contains(observation.profile_id.as_str())
        })
        .map(|(_, observation)| observation.clone())
        .collect::<Vec<_>>();
    let archive = PromptParetoArchive::build(
        &known_population,
        &observations,
        PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
    )?;
    let convergence = evaluate_prompt_convergence(
        &known_population,
        &observations,
        PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
        PROMPT_EVOLUTION_STAGNATION_PATIENCE,
        PROMPT_EVOLUTION_MIN_IMPROVEMENT,
        PROMPT_EVOLUTION_MAX_GENERATION,
    )?;
    let champion = convergence.champion.as_ref();
    let instance_scores = prompt_instance_pareto_scores(&known_population, &observations);
    let instance_archive = PromptInstanceParetoArchive::build(
        &known_population,
        &instance_scores,
        PROMPT_EVOLUTION_MIN_PARETO_REPEATS,
    )?;
    let mut frontier_ids = archive
        .candidates
        .iter()
        .map(|candidate| candidate.genome.id.clone())
        .collect::<BTreeSet<_>>();
    frontier_ids.extend(
        instance_archive
            .candidates
            .iter()
            .map(|candidate| candidate.profile_id.clone()),
    );
    let split_counts = known_population
        .iter()
        .map(|genome| {
            (
                genome.id.clone(),
                prompt_profile_evidence_counts(&observations, &genome.id),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let profile_complete = |genome: &ConductorPromptGenome| {
        let (train, holdout) = split_counts.get(&genome.id).copied().unwrap_or_default();
        train >= PROMPT_EVOLUTION_MIN_TRAIN_RUNS && holdout >= PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS
    };
    let aggregate_breeding_parent = if let Some(generation) = known_population
        .iter()
        .filter(|genome| profile_complete(genome))
        .map(|genome| genome.generation)
        .max()
    {
        let generation_genomes = known_population
            .iter()
            .filter(|genome| genome.generation == generation)
            .cloned()
            .collect::<Vec<_>>();
        let generation_ids = generation_genomes
            .iter()
            .map(|genome| genome.id.as_str())
            .collect::<BTreeSet<_>>();
        let generation_observations = observations
            .iter()
            .filter(|observation| generation_ids.contains(observation.profile_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        PromptParetoArchive::build(
            &generation_genomes,
            &generation_observations,
            PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
        )?
        .champion()
        .map(|candidate| candidate.genome.clone())
    } else {
        None
    };
    let instance_breeding_parent = instance_archive
        .select_for_mutation(observations.len() as u64)
        .and_then(|candidate| {
            known_population
                .iter()
                .find(|genome| genome.id == candidate.profile_id)
                .cloned()
        });
    let breeding_parent = instance_breeding_parent.or(aggregate_breeding_parent);
    let mut population = Vec::new();
    if let Some(champion) = convergence.champion.as_ref() {
        population.push(champion.genome.clone());
    }
    population.extend(
        known_population
            .iter()
            .filter(|genome| !profile_complete(genome))
            .cloned(),
    );
    if archive.candidates.is_empty() {
        population.extend(known_population.iter().cloned());
    } else {
        if let Some(parent) = breeding_parent.as_ref() {
            population.extend(parent.mutations());
        }
        population.extend(archive.next_generation(PROMPT_EVOLUTION_POPULATION_LIMIT));
    }
    if let Some(merged) = prompt_instance_merge_candidate(&instance_archive, &known_population) {
        population.push(merged);
    }
    if population.is_empty() {
        population = initial_prompt_population(effort);
    }
    let mut ids = BTreeSet::new();
    population.retain(|genome| ids.insert(genome.id.clone()));
    population.sort_by_key(|genome| {
        let (train, holdout) = split_counts.get(&genome.id).copied().unwrap_or_default();
        let complete = train >= PROMPT_EVOLUTION_MIN_TRAIN_RUNS
            && holdout >= PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS;
        let priority = if champion.is_some_and(|candidate| candidate.genome.id == genome.id) {
            0
        } else if (genome.id.starts_with("learned-") || genome.id.starts_with("merge-"))
            && !complete
        {
            1
        } else if !complete {
            2
        } else {
            3
        };
        (
            priority,
            train + holdout,
            genome.generation,
            genome.id.clone(),
        )
    });
    population.truncate(PROMPT_EVOLUTION_POPULATION_LIMIT);
    let exploration_profile = population
        .iter()
        .min_by_key(|genome| {
            let (train, holdout) = split_counts.get(&genome.id).copied().unwrap_or_default();
            let runs = train + holdout;
            let complete = train >= PROMPT_EVOLUTION_MIN_TRAIN_RUNS
                && holdout >= PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS;
            let priority = if !complete && runs > 0 {
                0
            } else if !complete {
                1
            } else {
                2
            };
            (priority, runs, genome.generation, genome.id.clone())
        })
        .cloned()
        .unwrap_or_else(|| ConductorPromptGenome::seed_for_effort(effort));
    let shadow_due =
        convergence.frozen && (observations.len() + 1) % PROMPT_EVOLUTION_SHADOW_INTERVAL == 0;
    let shadow_profile = shadow_due
        .then(|| {
            population
                .iter()
                .filter(|genome| champion.is_none_or(|candidate| candidate.genome.id != genome.id))
                .min_by_key(|genome| {
                    let (train, holdout) =
                        split_counts.get(&genome.id).copied().unwrap_or_default();
                    (train + holdout, genome.generation, genome.id.clone())
                })
                .cloned()
        })
        .flatten();
    let (status, next_mode, next_profile) = if convergence.frozen {
        if let Some(profile) = shadow_profile {
            ("shadow", "shadow", profile)
        } else if let Some(champion) = champion {
            ("frozen", "champion", champion.genome.clone())
        } else {
            ("exploring", "explore", exploration_profile.clone())
        }
    } else {
        ("exploring", "explore", exploration_profile.clone())
    };
    let next_runs = split_counts
        .get(&next_profile.id)
        .map(|(train, holdout)| train + holdout)
        .unwrap_or_default();
    let pending_evolved_profile = population.iter().any(|genome| {
        (genome.id.starts_with("learned-") || genome.id.starts_with("merge-"))
            && !profile_complete(genome)
    });
    let learned_child_exists = breeding_parent.as_ref().is_some_and(|parent| {
        known_population.iter().any(|genome| {
            genome.id.starts_with("learned-")
                && genome
                    .parents
                    .iter()
                    .any(|candidate| candidate == &parent.id)
        })
    });
    let mutation_candidate = (!convergence.frozen
        && next_runs == 0
        && !pending_evolved_profile
        && !learned_child_exists)
        .then(|| breeding_parent.clone())
        .flatten()
        .filter(|genome| genome.generation < PROMPT_EVOLUTION_MAX_GENERATION);
    let mutation_trajectories = mutation_candidate
        .as_ref()
        .map(|parent| prompt_reflection_packets(&observations, &parent.id, 6))
        .unwrap_or_default();
    let mutation_parent = mutation_candidate.filter(|_| !mutation_trajectories.is_empty());
    Ok(PromptEvolutionEvaluation {
        population,
        observations,
        frontier_ids,
        champion_id: champion.map(|candidate| candidate.genome.id.clone()),
        champion_score: champion.map(|candidate| candidate.robust_score()),
        champion_confidence: champion.map(|candidate| candidate.confidence.clone()),
        status: status.to_string(),
        freeze_reason: convergence.reason,
        stagnant_generations: convergence.stagnant_generations,
        evaluated_generations: convergence.evaluated_generations,
        next_mode: next_mode.to_string(),
        next_profile,
        mutation_parent,
        mutation_trajectories,
    })
}

pub(crate) fn prompt_evolution_evaluation_for_run(
    state: &tauri::State<'_, AppState>,
    effort: &str,
    run_context: &Metadata,
) -> Result<PromptEvolutionEvaluation, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let mut model =
        load_prompt_evolution_read_model(&mut store).map_err(|error| error.to_string())?;
    let mut evaluation = evaluate_prompt_evolution_read_model(&model, effort)?;
    let previous_rollout = model.rollouts.get(effort).cloned();
    let rollout = reconcile_prompt_rollout(&mut model, effort, &evaluation);
    apply_prompt_rollout_selection(&mut evaluation, &rollout, &model, run_context, effort);
    save_prompt_evolution_read_model(&mut store, &model).map_err(|error| error.to_string())?;
    if previous_rollout.as_ref() != Some(&rollout) {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Conductor prompt rollout updated",
            metadata_with_context(
                [
                    ("prompt_effort".to_string(), effort.to_string()),
                    (
                        "stable_profile".to_string(),
                        rollout.stable_profile_id.clone(),
                    ),
                    (
                        "canary_profile".to_string(),
                        rollout.canary_profile_id.clone().unwrap_or_default(),
                    ),
                    (
                        "canary_percent".to_string(),
                        rollout.canary_percent.to_string(),
                    ),
                    ("rollout_status".to_string(), rollout.status.clone()),
                    (
                        "rollout_reason".to_string(),
                        rollout.last_reason.clone().unwrap_or_default(),
                    ),
                    (
                        "promotion_confidence".to_string(),
                        rollout
                            .promotion_confidence
                            .map(|value| format!("{value:.4}"))
                            .unwrap_or_default(),
                    ),
                    (
                        "evidence_checkpoint".to_string(),
                        rollout.evidence_checkpoint.to_string(),
                    ),
                    (
                        "live_checkpoint".to_string(),
                        rollout.live_checkpoint.to_string(),
                    ),
                    (
                        "rollback_count".to_string(),
                        rollout.rollback_count.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(evaluation)
}

pub(crate) fn prompt_evolution_readiness(
    applicable: bool,
    enabled: bool,
    dataset_cases: usize,
    paired_runs: usize,
    replay_runs: usize,
    ready_profiles: usize,
    evaluation_inflight: bool,
    rollout_status: &str,
) -> String {
    if !applicable {
        return "not_applicable".to_string();
    }
    if !enabled {
        return "disabled".to_string();
    }
    if evaluation_inflight {
        return "evaluating".to_string();
    }
    if dataset_cases < PROMPT_EVOLUTION_OFFLINE_MIN_CASES {
        return "collecting_dataset".to_string();
    }
    if paired_runs < PROMPT_EVOLUTION_MIN_TRAIN_RUNS {
        return "collecting_train_evidence".to_string();
    }
    if replay_runs < PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS {
        return "collecting_holdout_evidence".to_string();
    }
    if ready_profiles == 0 {
        return "selecting_frontier".to_string();
    }
    match rollout_status {
        "canary" => "canary".to_string(),
        "rolled_back" => "rolled_back".to_string(),
        "promoted" => "promoted".to_string(),
        _ => "ready".to_string(),
    }
}

pub(crate) fn prompt_evolution_state(
    store: &mut SqliteStore,
    config: &ProviderConfig,
) -> Result<PromptEvolutionState, StorageError> {
    let model = load_prompt_evolution_read_model(store)?;
    let events = prompt_evolution_profile_events(&model);
    let rollouts = model.rollouts.clone();
    let datasets = model.datasets.clone();
    let observations = model.observations;
    let mut profile_rows = Vec::new();
    let mut effort_rows = Vec::new();
    let mut observed_runs = 0usize;
    let mut generation = 0u32;
    let mut population_size = 0usize;
    let mut frontier_profiles = 0usize;
    let mut paired_runs = 0usize;
    let mut replay_runs = 0usize;
    let mut reflection_packets = 0usize;
    let mut learned_profiles = 0usize;
    let inflight_efforts = prompt_evaluation_inflight()
        .lock()
        .map(|inflight| inflight.clone())
        .unwrap_or_default();
    for effort in ["fast", "auto", "pro"] {
        let evaluation =
            evaluate_prompt_evolution_with_observations(&events, effort, &observations)
                .map_err(StorageError::new)?;
        observed_runs += evaluation.observations.len();
        population_size += evaluation.population.len();
        frontier_profiles += evaluation.frontier_ids.len();
        let effort_paired_runs = evaluation
            .observations
            .iter()
            .filter(|observation| observation.mode.is_paired_execution())
            .map(|observation| observation.evaluation_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let effort_replay_runs = evaluation
            .observations
            .iter()
            .filter(|observation| observation.mode.is_replay_execution())
            .map(|observation| observation.evaluation_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let effort_reflection_packets = evaluation
            .observations
            .iter()
            .filter(|observation| {
                observation.split == PromptEvaluationSplit::Train
                    && observation.mode.is_paired_execution()
                    && observation.reflection_packet.is_some()
            })
            .count();
        let effort_learned_profiles = evaluation
            .population
            .iter()
            .filter(|genome| genome.id.starts_with("learned-"))
            .count();
        paired_runs += effort_paired_runs;
        replay_runs += effort_replay_runs;
        reflection_packets += effort_reflection_packets;
        learned_profiles += effort_learned_profiles;
        let ready_profiles = evaluation.frontier_ids.len();
        let rollout = rollouts
            .get(effort)
            .cloned()
            .unwrap_or_else(|| default_prompt_rollout(effort));
        let dataset = datasets
            .values()
            .filter(|dataset| dataset.effort == effort)
            .max_by_key(|dataset| {
                (
                    dataset.case_count >= PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
                    dataset.case_count,
                    dataset.updated_at_ms,
                )
            });
        let dataset_cases = dataset
            .map(|dataset| dataset.case_count)
            .unwrap_or_default();
        let dataset_train_cases = dataset
            .map(|dataset| dataset.train_count)
            .unwrap_or_default();
        let dataset_holdout_cases = dataset
            .map(|dataset| dataset.holdout_count)
            .unwrap_or_default();
        let applicable = effort != "fast";
        let evaluation_inflight = inflight_efforts.contains(effort);
        let readiness = prompt_evolution_readiness(
            applicable,
            config.prompt_evolution_enabled,
            dataset_cases,
            effort_paired_runs,
            effort_replay_runs,
            ready_profiles,
            evaluation_inflight,
            &rollout.status,
        );
        effort_rows.push(PromptEvolutionEffortState {
            effort: effort.to_string(),
            applicable,
            status: if config.prompt_evolution_enabled {
                evaluation.status.clone()
            } else {
                "disabled".to_string()
            },
            champion_id: evaluation.champion_id.clone(),
            champion_score: evaluation.champion_score,
            stagnant_generations: evaluation.stagnant_generations,
            evaluated_generations: evaluation.evaluated_generations,
            freeze_reason: evaluation.freeze_reason.clone(),
            shadow_rate_percent: (100 / PROMPT_EVOLUTION_SHADOW_INTERVAL) as u8,
            next_mode: evaluation.next_mode.clone(),
            paired_runs: effort_paired_runs,
            replay_runs: effort_replay_runs,
            reflection_packets: effort_reflection_packets,
            learned_profiles: effort_learned_profiles,
            ready_profiles,
            evaluation_inflight,
            stable_profile_id: rollout.stable_profile_id,
            canary_profile_id: rollout.canary_profile_id,
            canary_percent: rollout.canary_percent,
            promotion_confidence: evaluation
                .champion_confidence
                .as_ref()
                .map(|confidence| confidence.wilson_lower_bound),
            rollback_count: rollout.rollback_count,
            rollout_status: rollout.status,
            readiness,
            dataset_cases,
            dataset_train_cases,
            dataset_holdout_cases,
            required_paired_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            required_replay_runs: PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
        });
        generation = generation.max(
            evaluation
                .population
                .iter()
                .map(|genome| genome.generation)
                .max()
                .unwrap_or_default(),
        );
        for genome in evaluation.population {
            let mut profile = PromptEvolutionAccumulator {
                effort: effort.to_string(),
                generation: genome.generation,
                ..PromptEvolutionAccumulator::default()
            };
            for observation in evaluation
                .observations
                .iter()
                .filter(|observation| observation.profile_id == genome.id)
            {
                profile.runs += 1;
                if observation.mode.is_paired_execution() {
                    profile.train_runs += 1;
                } else if observation.mode.is_replay_execution() {
                    profile.holdout_runs += 1;
                }
                if observation.split == PromptEvaluationSplit::Train
                    && observation.mode.is_paired_execution()
                    && observation.reflection_packet.is_some()
                {
                    profile.reflection_runs += 1;
                }
                profile.succeeded += usize::from(observation.succeeded);
                profile.reward_total += observation.reward();
                if observation.mode != PromptEvaluationMode::Live {
                    profile.relative_reward_total += observation.group_relative_reward();
                    profile.relative_reward_runs += 1;
                }
                for step in &observation.step_credits {
                    profile.step_credit_total += step.credit.clamp(0.0, 1.0);
                    profile.step_credit_count += 1;
                }
                profile.quality_total += observation.quality_score.clamp(0.0, 1.0);
                profile.latency_total_ms = profile
                    .latency_total_ms
                    .saturating_add(observation.latency_ms);
                profile.token_total = profile.token_total.saturating_add(observation.total_tokens);
            }
            profile_rows.push(PromptEvolutionProfileState {
                id: genome.id.clone(),
                effort: profile.effort,
                generation: profile.generation,
                runs: profile.runs,
                train_runs: profile.train_runs,
                holdout_runs: profile.holdout_runs,
                reflection_runs: profile.reflection_runs,
                success_rate: if profile.runs == 0 {
                    0.0
                } else {
                    profile.succeeded as f64 / profile.runs as f64
                },
                average_reward: (profile.runs > 0)
                    .then_some(profile.reward_total / profile.runs as f64),
                average_relative_reward: (profile.relative_reward_runs > 0)
                    .then_some(profile.relative_reward_total / profile.relative_reward_runs as f64),
                average_step_credit: (profile.step_credit_count > 0)
                    .then_some(profile.step_credit_total / profile.step_credit_count as f64),
                average_quality: (profile.runs > 0)
                    .then_some(profile.quality_total / profile.runs as f64),
                average_latency_ms: profile
                    .latency_total_ms
                    .checked_div(profile.runs as u64)
                    .unwrap_or_default(),
                average_tokens: profile
                    .token_total
                    .checked_div(profile.runs as u64)
                    .unwrap_or_default(),
                frontier: evaluation.frontier_ids.contains(&genome.id),
                champion: evaluation.champion_id.as_deref() == Some(genome.id.as_str()),
                learned: genome.id.starts_with("learned-"),
                next: evaluation.next_profile.id == genome.id,
            });
        }
    }
    profile_rows.sort_by_key(|profile| match profile.effort.as_str() {
        "fast" => (0, !profile.next, profile.generation, profile.id.clone()),
        "auto" => (1, !profile.next, profile.generation, profile.id.clone()),
        "pro" => (2, !profile.next, profile.generation, profile.id.clone()),
        _ => (3, !profile.next, profile.generation, profile.id.clone()),
    });

    Ok(PromptEvolutionState {
        enabled: config.prompt_evolution_enabled,
        observed_runs,
        generation,
        population_size,
        frontier_profiles,
        paired_runs,
        replay_runs,
        reflection_packets,
        learned_profiles,
        evaluation_inflight: !inflight_efforts.is_empty(),
        efforts: effort_rows,
        profiles: profile_rows,
    })
}
