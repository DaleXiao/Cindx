use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule_prompt_pairwise_evaluation(
    app: tauri::AppHandle,
    config: ProviderConfig,
    task_id: TaskId,
    run_context: Metadata,
    effort: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
) {
    let lease_key = effort.clone();
    let Ok(Some(inflight_lease)) = ExclusiveKeyLease::try_acquire(
        prompt_evaluation_inflight(),
        lease_key.clone(),
        "prompt evaluation inflight",
    ) else {
        return;
    };
    let spawn_result = std::thread::Builder::new()
        .name(format!("cindx-prompt-evaluation-{lease_key}"))
        .spawn(move || {
            let _inflight_lease = inflight_lease;
            let state = app.state::<AppState>();
            let control = Arc::new(AgentRunControl::new("pro"));
            let Ok(_control_lease) = RegisteredRunControl::register(
                &state.prompt_evaluation_controls,
                lease_key.clone(),
                Arc::clone(&control),
                "prompt evaluation control",
                "prompt evaluation is already active for this effort",
            ) else {
                return;
            };
            match wait_for_prompt_evaluation_idle(&state, &control) {
                Ok(true) => {}
                Ok(false) | Err(_) => return,
            }
            for _ in 0..PROMPT_EVOLUTION_BACKGROUND_BATCH_LIMIT {
                let result = run_background_prompt_pairwise_evaluation(
                    &state,
                    &config,
                    &task_id,
                    &run_context,
                    &effort,
                    &policy,
                    &worker_models,
                    agent_budget,
                    &current_profile,
                    &control,
                );
                match result {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(error) => {
                        if error != MODEL_REQUEST_CANCELLED {
                            if let Ok(mut store) = state.store.lock() {
                                let _ = append_event(
                                    &mut store,
                                    &task_id,
                                    EventKind::TaskStatusChanged,
                                    "Conductor pairwise evaluation failed",
                                    metadata_with_context(
                                        [
                                            (
                                                "background_evaluation".to_string(),
                                                "true".to_string(),
                                            ),
                                            ("prompt_effort".to_string(), effort.clone()),
                                            (
                                                "error".to_string(),
                                                truncate_for_collaboration(&error, 2_000),
                                            ),
                                        ]
                                        .into_iter()
                                        .collect(),
                                        &run_context,
                                    ),
                                );
                            }
                        }
                        break;
                    }
                }
            }
        });
    if let Err(error) = spawn_result {
        eprintln!("prompt evaluation worker could not start: {error}");
    }
}

fn prompt_pairwise_campaign_snapshot(
    config: &ProviderConfig,
    effort: &str,
    dataset: &[PromptOfflineCase],
    evaluation: &PromptEvolutionEvaluation,
    rollout: Option<&PromptRolloutState>,
) -> Result<PromptEvolutionCampaignSnapshot, String> {
    let paired_runs = evaluation
        .observations
        .iter()
        .filter(|observation| observation.mode.is_paired_execution())
        .map(|observation| observation.evaluation_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let replay_runs = evaluation
        .observations
        .iter()
        .filter(|observation| observation.mode.is_replay_execution())
        .map(|observation| observation.evaluation_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let reflection_packets = evaluation
        .observations
        .iter()
        .filter(|observation| {
            observation.split == PromptEvaluationSplit::Train
                && observation.mode.is_paired_execution()
                && observation.reflection_packet.is_some()
        })
        .count();
    let learned_profiles = evaluation
        .population
        .iter()
        .filter(|genome| genome.id.starts_with("learned-"))
        .count();
    let rollout = rollout
        .cloned()
        .unwrap_or_else(|| default_prompt_rollout(effort));
    derive_prompt_evolution_campaign(&PromptEvolutionCampaignInput {
        effort: effort.to_string(),
        applicable: effort != "fast",
        enabled: config.prompt_evolution_enabled,
        evaluation_inflight: true,
        dataset_digest: prompt_offline_dataset_digest(dataset),
        dataset_cases: dataset.len(),
        minimum_dataset_cases: PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
        reflection_packets,
        learned_profiles,
        paired_runs,
        required_paired_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
        replay_runs,
        required_replay_runs: PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
        ready_profiles: evaluation.frontier_ids.len(),
        stable_profile_id: rollout.stable_profile_id,
        canary_profile_id: rollout.canary_profile_id,
        canary_percent: rollout.canary_percent,
        rollout_status: rollout.status,
        frozen: evaluation.freeze_reason.is_some(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_background_prompt_pairwise_evaluation(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    policy: &str,
    worker_models: &[String],
    agent_budget: usize,
    current_profile: &ConductorPromptGenome,
    control: &Arc<AgentRunControl>,
) -> Result<bool, String> {
    if control.should_stop() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    if worker_models.is_empty() {
        return Ok(false);
    }
    let Some(project_id) = run_context.get("project_id") else {
        return Ok(false);
    };
    let workspace_root = run_context
        .get("project_root")
        .map(|root| validate_workspace_root(root))
        .transpose()?
        .unwrap_or(active_workspace_root(state)?);
    let (evaluation, dataset, rollout, known_profiles, previous_dataset) = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let model =
            load_prompt_evolution_read_model(&mut store).map_err(|error| error.to_string())?;
        let events = store
            .list_by_task(task_id)
            .map_err(|error| error.to_string())?;
        let rollout = model.rollouts.get(effort).cloned();
        let known_profiles = model
            .genomes
            .iter()
            .filter(|record| record.effort == effort)
            .map(|record| record.genome.clone())
            .collect::<Vec<_>>();
        let previous_dataset = model
            .datasets
            .get(&prompt_dataset_key(effort, project_id))
            .cloned();
        (
            evaluate_prompt_evolution_read_model(&model, effort)?,
            prompt_offline_dataset(&events, project_id),
            rollout,
            known_profiles,
            previous_dataset,
        )
    };
    if dataset.len() < PROMPT_EVOLUTION_OFFLINE_MIN_CASES {
        let digest = prompt_offline_dataset_digest(&dataset);
        if previous_dataset.as_ref().is_none_or(|snapshot| {
            snapshot.digest != digest || snapshot.status != "insufficient_cases"
        }) {
            append_prompt_offline_dataset_snapshot(
                state,
                task_id,
                run_context,
                effort,
                &dataset,
                None,
            )?;
        }
        return Ok(false);
    }
    let campaign =
        prompt_pairwise_campaign_snapshot(config, effort, &dataset, &evaluation, rollout.as_ref())?;
    let mut campaign_run_context = run_context.clone();
    campaign_run_context.insert(
        "prompt_campaign_stage".to_string(),
        campaign.stage.as_str().to_string(),
    );
    campaign_run_context.insert(
        "prompt_campaign_next_action".to_string(),
        campaign.next_action.clone(),
    );
    campaign_run_context.insert(
        "prompt_campaign_resume_token".to_string(),
        campaign.resume_token.clone(),
    );
    let run_context = &campaign_run_context;
    let attempted_mutation_parent = evaluation
        .mutation_parent
        .as_ref()
        .map(|parent| parent.id.clone());
    if let Some(parent) = evaluation.mutation_parent.as_ref() {
        if generate_background_prompt_mutation(
            state,
            config,
            task_id,
            run_context,
            effort,
            parent,
            &evaluation.mutation_trajectories,
            control,
        )? {
            return Ok(true);
        }
    }
    let current_task_class = dataset
        .first()
        .map(|case| case.task_class.as_str())
        .unwrap_or("general");
    let Some(challenger) =
        prompt_rollout_counterpart(rollout.as_ref(), &known_profiles, current_profile).or_else(
            || prompt_evolution_challenger(&evaluation, current_profile, current_task_class),
        )
    else {
        return Ok(false);
    };
    let current_counts = prompt_direct_profile_evidence_counts(
        &evaluation.observations,
        &current_profile.id,
        &challenger.id,
    );
    let challenger_counts = prompt_direct_profile_evidence_counts(
        &evaluation.observations,
        &challenger.id,
        &current_profile.id,
    );
    let required_holdout_runs = rollout
        .as_ref()
        .filter(|rollout| {
            rollout.canary_profile_id.as_ref().is_some_and(|canary| {
                (current_profile.id == rollout.stable_profile_id && challenger.id == *canary)
                    || (challenger.id == rollout.stable_profile_id && current_profile.id == *canary)
            })
        })
        .and_then(|rollout| {
            let canary = rollout.canary_profile_id.as_deref()?;
            let live_runs = evaluation
                .observations
                .iter()
                .filter(|observation| {
                    observation.profile_id == canary
                        && observation.mode == PromptEvaluationMode::Live
                })
                .count();
            (live_runs > rollout.live_checkpoint).then(|| {
                PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS.max(rollout.evidence_checkpoint.saturating_add(2))
            })
        })
        .unwrap_or(PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS);
    let split = if current_counts.0 < PROMPT_EVOLUTION_MIN_TRAIN_RUNS
        || challenger_counts.0 < PROMPT_EVOLUTION_MIN_TRAIN_RUNS
    {
        PromptEvaluationSplit::Train
    } else if current_counts.1 < required_holdout_runs
        || challenger_counts.1 < required_holdout_runs
    {
        PromptEvaluationSplit::Holdout
    } else {
        let live_observations = evaluation
            .observations
            .iter()
            .filter(|observation| observation.mode == PromptEvaluationMode::Live)
            .count();
        if live_observations == 0 || live_observations % PROMPT_EVOLUTION_SHADOW_INTERVAL != 0 {
            return Ok(false);
        }
        PromptEvaluationSplit::Train
    };
    let Some(selected_case) = select_prompt_offline_case(
        &dataset,
        &evaluation.observations,
        &current_profile.id,
        &challenger.id,
        split,
    ) else {
        return Ok(false);
    };
    append_prompt_offline_dataset_snapshot(
        state,
        task_id,
        run_context,
        effort,
        &dataset,
        Some(&selected_case),
    )?;
    let mode = match split {
        PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
        PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
    };
    let objective = selected_case.objective.clone();
    let task_class = selected_case.task_class.clone();
    let evaluation_id = unique_id(match mode {
        PromptEvaluationMode::PairedShadow | PromptEvaluationMode::PairedExecution => "prompt-pair",
        PromptEvaluationMode::ReplayHoldout | PromptEvaluationMode::ReplayExecution => {
            "prompt-replay"
        }
        PromptEvaluationMode::Live => "prompt-live",
    });
    append_prompt_evaluation_status(
        state,
        task_id,
        run_context,
        "Conductor pairwise evaluation started",
        &evaluation_id,
        effort,
        mode,
        &current_profile.id,
        &challenger.id,
        None,
    )?;

    let (current_plan, challenger_plan) = std::thread::scope(|scope| {
        let current = scope.spawn(|| {
            evaluate_conductor_prompt_profile(
                config,
                &objective,
                effort,
                policy,
                worker_models,
                agent_budget,
                current_profile,
                &evaluation_id,
                control,
            )
        });
        let challenger_handle = scope.spawn(|| {
            evaluate_conductor_prompt_profile(
                config,
                &objective,
                effort,
                policy,
                worker_models,
                agent_budget,
                &challenger,
                &evaluation_id,
                control,
            )
        });
        (
            current.join().unwrap_or_else(|_| PromptPlanCandidate {
                genome: current_profile.clone(),
                plan: None,
                raw_output: "conductor evaluation panicked".to_string(),
                latency_ms: 0,
                total_tokens: 0,
            }),
            challenger_handle
                .join()
                .unwrap_or_else(|_| PromptPlanCandidate {
                    genome: challenger.clone(),
                    plan: None,
                    raw_output: "challenger evaluation panicked".to_string(),
                    latency_ms: 0,
                    total_tokens: 0,
                }),
        )
    });
    if control.should_stop() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let (current, challenger) = std::thread::scope(|scope| {
        let current_handle = scope.spawn(|| {
            execute_prompt_workflow_candidate(
                config,
                &workspace_root,
                &objective,
                current_plan,
                control,
            )
        });
        let challenger_handle = scope.spawn(|| {
            execute_prompt_workflow_candidate(
                config,
                &workspace_root,
                &objective,
                challenger_plan,
                control,
            )
        });
        (
            current_handle
                .join()
                .expect("current evaluation worker joined"),
            challenger_handle
                .join()
                .expect("challenger evaluation worker joined"),
        )
    });
    if control.should_stop() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let candidate_a = &current;
    let candidate_b = &challenger;
    let objective_ref = objective.as_str();
    let (forward, reverse) = std::thread::scope(|scope| {
        let forward_id = format!("{evaluation_id}-forward");
        let reverse_id = format!("{evaluation_id}-reverse");
        let forward = scope.spawn(move || {
            evaluate_prompt_candidate_pair(
                config,
                objective_ref,
                candidate_a,
                candidate_b,
                &forward_id,
                control,
            )
        });
        let reverse = scope.spawn(move || {
            evaluate_prompt_candidate_pair(
                config,
                objective_ref,
                candidate_b,
                candidate_a,
                &reverse_id,
                control,
            )
        });
        (
            forward
                .join()
                .unwrap_or_else(|_| Err("forward pairwise reviewer panicked".to_string())),
            reverse
                .join()
                .unwrap_or_else(|_| Err("reverse pairwise reviewer panicked".to_string())),
        )
    });
    let (forward, reverse) = match (forward, reverse) {
        (Ok(forward), Ok(reverse)) => (forward, reverse_prompt_pairwise_payload(reverse)),
        (Err(forward), Err(reverse)) => {
            return Err(format!(
                "both pairwise reviewers failed: forward={forward}; reverse={reverse}"
            ));
        }
        (Err(error), Ok(_)) => {
            return Err(format!("forward pairwise reviewer failed: {error}"));
        }
        (Ok(_), Err(error)) => {
            return Err(format!("reverse pairwise reviewer failed: {error}"));
        }
    };
    validate_prompt_pairwise_agreement(&forward, &reverse)?;
    let judge = aggregate_prompt_pairwise_payloads(forward, reverse);
    if control.should_stop() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let split = match mode {
        PromptEvaluationMode::ReplayHoldout | PromptEvaluationMode::ReplayExecution => {
            PromptEvaluationSplit::Holdout
        }
        PromptEvaluationMode::PairedShadow
        | PromptEvaluationMode::PairedExecution
        | PromptEvaluationMode::Live => PromptEvaluationSplit::Train,
    };
    let observation_a = prompt_pairwise_observation(
        candidate_a,
        candidate_b,
        &objective,
        &evaluation_id,
        &task_class,
        split,
        mode,
        judge.score_a,
        judge.score_b,
        judge.safety_violations_a,
        &judge.step_scores_a,
        judge.feedback_a,
        std::slice::from_ref(&config.api_key),
    );
    let observation_b = prompt_pairwise_observation(
        candidate_b,
        candidate_a,
        &objective,
        &evaluation_id,
        &task_class,
        split,
        mode,
        judge.score_b,
        judge.score_a,
        judge.safety_violations_b,
        &judge.step_scores_b,
        judge.feedback_b,
        std::slice::from_ref(&config.api_key),
    );
    append_prompt_pairwise_observations(
        state,
        task_id,
        run_context,
        effort,
        mode,
        [&observation_a, &observation_b],
        [&candidate_a.plan.genome, &candidate_b.plan.genome],
    )?;
    let next_evaluation = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let model =
            load_prompt_evolution_read_model(&mut store).map_err(|error| error.to_string())?;
        evaluate_prompt_evolution_read_model(&model, effort)?
    };
    if let Some(parent) = next_evaluation.mutation_parent.as_ref() {
        if attempted_mutation_parent.as_deref() != Some(parent.id.as_str()) {
            let _ = generate_background_prompt_mutation(
                state,
                config,
                task_id,
                run_context,
                effort,
                parent,
                &next_evaluation.mutation_trajectories,
                control,
            )?;
        }
    }
    Ok(true)
}
