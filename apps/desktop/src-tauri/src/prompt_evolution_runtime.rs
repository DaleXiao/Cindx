use super::*;

pub(crate) fn prompt_mutation_reflection_packets(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    effort: &str,
) -> Vec<AgentEvaluationReflectionPacket> {
    let mut transfer = if effort == "pro" {
        prompt_transfer_reflection_packets(observations, profile_id, 3)
    } else {
        Vec::new()
    };
    let ordinary_limit = if transfer.is_empty() { 6 } else { 3 };
    let mut packets = prompt_reflection_packets(observations, profile_id, ordinary_limit);
    packets.append(&mut transfer);
    let mut seen = BTreeSet::new();
    packets.retain(|packet| seen.insert((packet.run_id.clone(), packet.case_id.clone())));
    packets.truncate(6);
    packets
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
    let rejected_proposals = known_population
        .iter()
        .filter_map(|genome| {
            prompt_proposal_minibatch_decision(
                genome,
                &observations,
                PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
                PROMPT_EVOLUTION_MINIBATCH_RELATIVE_IMPROVEMENT,
            )
            .ok()
            .filter(PromptProposalMinibatchDecision::is_rejected)
            .map(|_| genome.id.clone())
        })
        .collect::<BTreeSet<_>>();
    let proposal_is_active =
        |genome: &ConductorPromptGenome| !rejected_proposals.contains(genome.id.as_str());
    let active_population = known_population
        .iter()
        .filter(|genome| proposal_is_active(genome))
        .cloned()
        .collect::<Vec<_>>();
    let active_ids = active_population
        .iter()
        .map(|genome| genome.id.as_str())
        .collect::<BTreeSet<_>>();
    let active_observations = observations
        .iter()
        .filter(|observation| active_ids.contains(observation.profile_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let archive = PromptParetoArchive::build(
        &active_population,
        &active_observations,
        PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
    )?;
    let search_archive = PromptSearchArchive::build(
        &active_population,
        &active_observations,
        PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
    )?;
    let convergence = evaluate_prompt_convergence(
        &active_population,
        &active_observations,
        PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
        PROMPT_EVOLUTION_STAGNATION_PATIENCE,
        PROMPT_EVOLUTION_MIN_IMPROVEMENT,
        PROMPT_EVOLUTION_MAX_GENERATION,
    )?;
    let champion = convergence.champion.as_ref();
    let instance_scores = prompt_instance_pareto_scores(&active_population, &active_observations);
    let instance_archive = PromptInstanceParetoArchive::build(
        &active_population,
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
            let (_, holdout) = prompt_profile_evidence_counts(&observations, &genome.id);
            (
                genome.id.clone(),
                (
                    prompt_profile_training_evidence_count(&observations, &genome.id),
                    holdout,
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let profile_search_complete = |genome: &ConductorPromptGenome| {
        split_counts
            .get(&genome.id)
            .is_some_and(|(train, _)| *train >= PROMPT_EVOLUTION_MIN_TRAIN_RUNS)
    };
    let aggregate_breeding_parent = if let Some(generation) = active_population
        .iter()
        .filter(|genome| profile_search_complete(genome))
        .map(|genome| genome.generation)
        .max()
    {
        let generation_genomes = active_population
            .iter()
            .filter(|genome| genome.generation == generation)
            .cloned()
            .collect::<Vec<_>>();
        let generation_ids = generation_genomes
            .iter()
            .map(|genome| genome.id.as_str())
            .collect::<BTreeSet<_>>();
        let generation_observations = active_observations
            .iter()
            .filter(|observation| generation_ids.contains(observation.profile_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        PromptSearchArchive::build(
            &generation_genomes,
            &generation_observations,
            PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        )?
        .champion()
        .map(|candidate| candidate.genome.clone())
    } else {
        None
    };
    let instance_breeding_parent = instance_archive
        .select_for_mutation(instance_scores.len() as u64)
        .and_then(|candidate| {
            active_population
                .iter()
                .find(|genome| genome.id == candidate.profile_id)
                .cloned()
        });
    let breeding_parent = instance_breeding_parent.or(aggregate_breeding_parent);
    let mut population = Vec::new();
    population.extend(
        known_population
            .iter()
            .filter(|genome| proposal_is_active(genome) && !profile_search_complete(genome))
            .cloned(),
    );
    if search_archive.candidates.is_empty() {
        population.extend(
            known_population
                .iter()
                .filter(|genome| proposal_is_active(genome))
                .cloned(),
        );
    } else {
        if let Some(parent) = breeding_parent.as_ref() {
            population.extend(parent.mutations());
        }
        population.extend(search_archive.next_generation(PROMPT_EVOLUTION_POPULATION_LIMIT));
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
        let search_complete = train >= PROMPT_EVOLUTION_MIN_TRAIN_RUNS;
        let priority = if (genome.id.starts_with("learned-")
            || genome.id.starts_with("merge-"))
            && !search_complete
        {
            0
        } else if !search_complete {
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
            let search_complete = train >= PROMPT_EVOLUTION_MIN_TRAIN_RUNS;
            let complete = search_complete
                && holdout >= PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS;
            let priority = if !search_complete && runs > 0 {
                0
            } else if !search_complete {
                1
            } else if !complete {
                2
            } else {
                3
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
            && proposal_is_active(genome)
            && !profile_search_complete(genome)
    });
    let learned_child_exists = breeding_parent.as_ref().is_some_and(|parent| {
        known_population.iter().any(|genome| {
            genome.id.starts_with("learned-")
                && proposal_is_active(genome)
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
        .map(|parent| prompt_mutation_reflection_packets(&observations, &parent.id, effort))
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
    let evidence_scope = run_context
        .get("project_id")
        .cloned()
        .unwrap_or_else(|| "global".to_string());
    let mut scoped_model = prompt_evolution_read_model_for_scope(&model, &evidence_scope);
    let mut evaluation = evaluate_prompt_evolution_read_model(&scoped_model, effort)?;
    let previous_rollout = scoped_model.rollouts.get(effort).cloned();
    let rollout = reconcile_prompt_rollout(&mut scoped_model, effort, &evaluation);
    persist_scoped_prompt_rollout(&mut model, &evidence_scope, effort, rollout.clone());
    apply_prompt_rollout_selection(
        &mut evaluation,
        &rollout,
        &scoped_model,
        run_context,
        effort,
    );
    if previous_rollout.as_ref() != Some(&rollout) {
        save_prompt_evolution_read_model(&mut store, &model).map_err(|error| error.to_string())?;
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Conductor prompt rollout updated",
            metadata_with_context(
                [
                    ("prompt_effort".to_string(), effort.to_string()),
                    ("prompt_rollout_scope".to_string(), evidence_scope.clone()),
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
                    (
                        "frozen_prompt_profile".to_string(),
                        rollout
                            .frozen_profile
                            .as_ref()
                            .and_then(|snapshot| serde_json::to_string(snapshot).ok())
                            .unwrap_or_default(),
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

#[cfg(test)]
pub(crate) struct PromptEvolutionReadinessInput<'a> {
    pub(crate) applicable: bool,
    pub(crate) enabled: bool,
    pub(crate) dataset_cases: usize,
    pub(crate) paired_runs: usize,
    pub(crate) replay_runs: usize,
    pub(crate) ready_profiles: usize,
    pub(crate) evaluation_inflight: bool,
    pub(crate) rollout_status: &'a str,
}

#[cfg(test)]
pub(crate) fn prompt_evolution_readiness(input: PromptEvolutionReadinessInput<'_>) -> String {
    let PromptEvolutionReadinessInput {
        applicable,
        enabled,
        dataset_cases,
        paired_runs,
        replay_runs,
        ready_profiles,
        evaluation_inflight,
        rollout_status,
    } = input;
    derive_prompt_evolution_campaign(&PromptEvolutionCampaignInput {
        effort: "legacy".to_string(),
        applicable,
        enabled,
        evaluation_inflight,
        dataset_digest: String::new(),
        dataset_cases,
        minimum_dataset_cases: PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
        reflection_packets: usize::from(dataset_cases >= PROMPT_EVOLUTION_OFFLINE_MIN_CASES),
        learned_profiles: usize::from(dataset_cases >= PROMPT_EVOLUTION_OFFLINE_MIN_CASES),
        paired_runs,
        required_paired_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        replay_runs,
        required_replay_runs: PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
        ready_profiles,
        stable_profile_id: "legacy-stable".to_string(),
        canary_profile_id: (rollout_status == "canary").then(|| "legacy-canary".to_string()),
        canary_percent: u8::from(rollout_status == "canary") * 10,
        rollout_status: rollout_status.to_string(),
        frozen: false,
    })
    .map(|campaign| campaign.readiness().to_string())
    .unwrap_or_else(|_| "collecting_dataset".to_string())
}

pub(crate) fn prompt_evolution_state(
    store: &mut SqliteStore,
    config: &ProviderConfig,
) -> Result<PromptEvolutionState, StorageError> {
    let model = load_prompt_evolution_read_model(store)?;
    let events = prompt_evolution_profile_events(&model);
    let rollouts = ["fast", "auto", "pro"]
        .into_iter()
        .filter_map(|effort| {
            visible_prompt_rollout(&model, effort).map(|rollout| (effort.to_string(), rollout))
        })
        .collect::<BTreeMap<_, _>>();
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
        .snapshot()
        .unwrap_or_default();
    for effort in ["fast", "auto", "pro"] {
        let evaluation =
            evaluate_prompt_evolution_with_observations(&events, effort, &observations)
                .map_err(StorageError::new)?;
        observed_runs += evaluation.observations.len();
        population_size += evaluation.population.len();
        frontier_profiles += evaluation.frontier_ids.len();
        let active_dataset_sha256 =
            orchestrator::latest_scientific_dataset_digest(&evaluation.observations);
        let is_active_scientific = |observation: &&PromptEvolutionObservation| {
            observation.is_scientific_evidence()
                && active_dataset_sha256
                    .is_some_and(|digest| observation.provenance.dataset_sha256 == digest)
        };
        let effort_paired_runs = evaluation
            .observations
            .iter()
            .filter(is_active_scientific)
            .filter(|observation| observation.mode.is_paired_execution())
            .map(|observation| observation.evaluation_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let effort_replay_runs = evaluation
            .observations
            .iter()
            .filter(is_active_scientific)
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
                    && observation.is_scientific_evidence()
                    && active_dataset_sha256
                        .is_some_and(|digest| observation.provenance.dataset_sha256 == digest)
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
        let dataset_digest = dataset
            .map(|dataset| dataset.digest.clone())
            .unwrap_or_default();
        let applicable = effort != "fast";
        let evaluation_inflight = inflight_efforts.contains(effort);
        let campaign = derive_prompt_evolution_campaign(&PromptEvolutionCampaignInput {
            effort: effort.to_string(),
            applicable,
            enabled: config.prompt_evolution_enabled,
            evaluation_inflight,
            dataset_digest,
            dataset_cases,
            minimum_dataset_cases: PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
            reflection_packets: effort_reflection_packets,
            learned_profiles: effort_learned_profiles,
            paired_runs: effort_paired_runs,
            required_paired_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            replay_runs: effort_replay_runs,
            required_replay_runs: PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
            ready_profiles,
            stable_profile_id: rollout.stable_profile_id.clone(),
            canary_profile_id: rollout.canary_profile_id.clone(),
            canary_percent: rollout.canary_percent,
            rollout_status: rollout.status.clone(),
            frozen: evaluation.freeze_reason.is_some(),
        })
        .map_err(StorageError::new)?;
        let readiness = campaign.readiness().to_string();
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
            campaign_stage: campaign.stage.as_str().to_string(),
            campaign_next_action: campaign.next_action,
            campaign_resume_token: campaign.resume_token,
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
                .filter(|observation| {
                    observation.mode == PromptEvaluationMode::Live
                        || (observation.is_scientific_evidence()
                            && active_dataset_sha256.is_some_and(|digest| {
                                observation.provenance.dataset_sha256 == digest
                            }))
                })
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
