use super::*;

pub(crate) struct PromptTreatmentControlGroup<'a> {
    parent: &'a AgentRunControl,
    lanes: Vec<Arc<AgentRunControl>>,
    absorbed: bool,
}

impl<'a> PromptTreatmentControlGroup<'a> {
    pub(crate) fn new(
        parent: &'a AgentRunControl,
        lane_count: usize,
        allocation_divisor: usize,
    ) -> Result<Self, String> {
        let lanes = (0..lane_count)
            .map(|_| {
                parent
                    .isolated_treatment(allocation_divisor)
                    .map(Arc::new)
                    .map_err(|reason| {
                        format!("matched treatment budget unavailable: {}", reason.code())
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            parent,
            lanes,
            absorbed: false,
        })
    }

    pub(crate) fn lane(&self, index: usize) -> Arc<AgentRunControl> {
        Arc::clone(&self.lanes[index])
    }

    pub(crate) fn lane_budget(&self) -> RunBudget {
        self.lanes[0].budget()
    }

    pub(crate) fn absorb(&mut self) -> Result<(), String> {
        if self.absorbed {
            return Ok(());
        }
        self.absorbed = true;
        let lanes = self.lanes.iter().map(Arc::as_ref).collect::<Vec<_>>();
        self.parent
            .absorb_isolated_treatments(&lanes)
            .map_err(|reason| format!("matched treatment accounting failed: {}", reason.code()))
    }
}

impl Drop for PromptTreatmentControlGroup<'_> {
    fn drop(&mut self) {
        let _ = self.absorb();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule_prompt_pairwise_evaluation(
    app: tauri::AppHandle,
    task_id: TaskId,
    run_context: Metadata,
    effort: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
) {
    let schedule_auto_transfer = effort == "auto";
    if schedule_auto_transfer {
        if let Err(error) =
            crate::prompt_evolution_transfer_outbox::persist_prompt_auto_transfer_intent(
                &app,
                &task_id,
                &run_context,
            )
        {
            eprintln!("prompt Auto transfer intent could not be persisted: {error}");
            return;
        }
    }
    if let Err(error) = enqueue_prompt_pairwise_evaluation(
        &app,
        &task_id,
        &run_context,
        None,
        effort,
        policy,
        worker_models,
        agent_budget,
        current_profile,
    ) {
        eprintln!("prompt evaluation request could not be persisted: {error}");
    }
}

fn prompt_auto_transfer_request(
    config: &ProviderConfig,
    model: &PromptEvolutionReadModel,
) -> Result<(String, String, Vec<String>, usize, ConductorPromptGenome), String> {
    let effort = AgentPolicy::Pro.label().to_string();
    let agent_budget = AgentPolicy::Pro.max_parallelism();
    let worker_models = collaboration_candidate_models(config, agent_budget);
    if worker_models.is_empty() {
        return Err("Pro prompt evaluation has no configured worker model".to_string());
    }
    let (current_profile, _) = stable_prompt_profile_fingerprint(model, &effort)?;
    Ok((
        effort,
        OrchestrationPolicy::BestOfN {
            candidates: agent_budget,
        }
        .label()
        .to_string(),
        worker_models,
        agent_budget,
        current_profile.with_effort_delivery_contract(AgentPolicy::Pro.label()),
    ))
}

pub(crate) fn enqueue_prompt_auto_transfer_evaluation(
    app: &tauri::AppHandle,
    task_id: &TaskId,
    run_context: &Metadata,
    request_id: Option<String>,
) -> Result<bool, String> {
    let Some(project_id) = run_context
        .get("project_id")
        .map(String::as_str)
        .filter(|project_id| !project_id.trim().is_empty())
    else {
        return Ok(false);
    };
    let state = app.state::<AppState>();
    let config = state
        .provider_config
        .lock()
        .map_err(|error| format!("provider config lock poisoned: {error}"))?
        .clone();
    if !config.prompt_evolution_enabled || !config.is_ready() {
        return Ok(false);
    }
    let scoped_model =
        crate::prompt_evolution_store_runtime::with_prompt_evolution_store(&state, |store| {
            let model =
                load_prompt_evolution_read_model(store).map_err(|error| error.to_string())?;
            Ok(prompt_evolution_read_model_for_scope(&model, project_id))
        })?;
    let (effort, policy, worker_models, agent_budget, current_profile) =
        prompt_auto_transfer_request(&config, &scoped_model)?;
    enqueue_prompt_pairwise_evaluation(
        app,
        task_id,
        run_context,
        request_id,
        effort,
        policy,
        worker_models,
        agent_budget,
        current_profile,
    )?;
    Ok(true)
}

fn prompt_evaluation_model_partition(
    config: &ProviderConfig,
    worker_models: &[String],
    additionally_excluded: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let conductor_model = config.model_for_conductor();
    let mut reviewer_candidates = Vec::new();
    for model in [
        config.model_for_role(&ModelRole::Reviewer),
        config.model_for_role(&ModelRole::Summarizer),
        config.model_for_role(&ModelRole::Planner),
        config.model.clone(),
        config.model_for_role(&ModelRole::Executor),
    ] {
        if !model.trim().is_empty()
            && model != conductor_model
            && !additionally_excluded.contains(&model)
            && !reviewer_candidates
                .iter()
                .any(|existing| existing == &model)
        {
            reviewer_candidates.push(model);
        }
    }

    for reviewer_model in reviewer_candidates {
        let retained_workers = worker_models
            .iter()
            .filter(|model| *model != &reviewer_model)
            .cloned()
            .collect::<Vec<_>>();
        if !retained_workers.is_empty() {
            return (retained_workers, vec![reviewer_model]);
        }
        if conductor_model != reviewer_model && !conductor_model.trim().is_empty() {
            return (vec![conductor_model.clone()], vec![reviewer_model]);
        }
    }

    (worker_models.to_vec(), Vec::new())
}

pub(crate) fn prompt_candidate_models(
    candidates: [&PromptExecutionCandidate; 2],
) -> BTreeSet<String> {
    let mut models = BTreeSet::new();
    for candidate in candidates {
        if let Some(plan) = candidate.plan.plan.as_ref() {
            models.insert(plan.coordinator_model.clone());
            models.extend(plan.steps.iter().map(|step| step.model.clone()));
        }
        models.extend(
            candidate
                .execution
                .steps
                .iter()
                .map(|step| step.model.clone()),
        );
    }
    models.retain(|model| !model.trim().is_empty());
    models
}

fn prompt_pairwise_campaign_snapshot(
    config: &ProviderConfig,
    effort: &str,
    dataset: &[PromptOfflineCase],
    evaluation: &PromptEvolutionEvaluation,
    rollout: Option<&PromptRolloutState>,
) -> Result<PromptEvolutionCampaignSnapshot, String> {
    let active_cohort_sha256 =
        orchestrator::latest_scientific_dataset_digest(&evaluation.observations);
    let is_active_scientific = |observation: &&PromptEvolutionObservation| {
        observation.is_scientific_evidence()
            && active_cohort_sha256
                .is_some_and(|digest| observation.scientific_cohort_sha256() == Some(digest))
    };
    let paired_runs = evaluation
        .observations
        .iter()
        .filter(is_active_scientific)
        .filter(|observation| observation.mode.is_paired_execution())
        .map(|observation| observation.evaluation_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let replay_runs = evaluation
        .observations
        .iter()
        .filter(is_active_scientific)
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
                && is_active_scientific(observation)
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
        .unwrap_or_else(|| crate::prompt_canary_runtime::default_prompt_rollout(effort));
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
    pro_teacher_snapshot: Option<&FrozenPromptProfileSnapshot>,
    control: &Arc<AgentRunControl>,
) -> Result<PromptPairwiseEvaluationOutcome, String> {
    if control.should_stop() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    if worker_models.is_empty() {
        return Ok(PromptPairwiseEvaluationOutcome::no_work());
    }
    let Some(project_id) = run_context.get("project_id") else {
        return Ok(PromptPairwiseEvaluationOutcome::no_work());
    };
    let workspace_root = run_context
        .get("project_root")
        .map(|root| validate_workspace_root(root))
        .transpose()?
        .unwrap_or(active_workspace_root(state)?);
    let (model, scoped_model, events) =
        crate::prompt_evolution_store_runtime::with_prompt_evolution_store(state, |store| {
            let model =
                load_prompt_evolution_read_model(store).map_err(|error| error.to_string())?;
            let scoped_model = prompt_evolution_read_model_for_scope(&model, project_id);
            let events = store
                .list_by_task_and_metadata(task_id, "project_id", project_id)
                .map_err(|error| error.to_string())?;
            Ok((model, scoped_model, events))
        })?;
    let rollout = scoped_model.rollouts.get(effort).cloned();
    let known_profiles = scoped_model
        .genomes
        .iter()
        .filter(|record| record.effort == effort)
        .map(|record| record.genome.clone())
        .collect::<Vec<_>>();
    let previous_dataset = model
        .datasets
        .get(&prompt_dataset_key(effort, project_id))
        .cloned();
    let auto_stable_profile = stable_prompt_profile_fingerprint(&scoped_model, "auto").ok();
    let preferred_auto_profile = auto_stable_profile
        .as_ref()
        .map(|(profile, sha256)| (profile.id.as_str(), sha256.as_str()));
    let discovered_dataset = prompt_learning_dataset(&events, project_id, preferred_auto_profile)
        .into_iter()
        .filter(|case| crate::prompt_learning_runtime::prompt_learning_case_is_safe(case, config))
        .collect();
    let evaluation = evaluate_prompt_evolution_read_model(&scoped_model, effort)?;
    let campaign_generation = evaluation
        .population
        .iter()
        .map(|profile| profile.generation)
        .chain(std::iter::once(current_profile.generation))
        .max()
        .unwrap_or_default();
    let dataset = if pro_teacher_snapshot.is_some() {
        discovered_dataset
    } else {
        prompt_offline_dataset_for_generation(
            discovered_dataset,
            previous_dataset.as_ref(),
            campaign_generation,
        )?
    };
    if dataset
        .iter()
        .any(|case| !crate::prompt_learning_runtime::prompt_learning_case_is_safe(case, config))
    {
        return Err("frozen prompt dataset contains residual sensitive data".to_string());
    }
    if dataset.len() < PROMPT_EVOLUTION_OFFLINE_MIN_CASES {
        let digest = prompt_offline_dataset_digest(&dataset);
        if pro_teacher_snapshot.is_none()
            && previous_dataset.as_ref().is_none_or(|snapshot| {
                snapshot.digest != digest || snapshot.status != "insufficient_cases"
            })
        {
            append_prompt_offline_dataset_snapshot(
                state,
                task_id,
                run_context,
                effort,
                &dataset,
                campaign_generation,
                None,
            )?;
        }
        return Ok(PromptPairwiseEvaluationOutcome::no_work());
    }
    let distillation = pro_teacher_snapshot
        .map(|snapshot| {
            crate::prompt_distillation_runtime::prepare_prompt_distillation_evaluation(
                &scoped_model,
                current_profile,
                snapshot,
                &dataset,
            )
        })
        .transpose()?;
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
    if distillation.is_none() {
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
                return Ok(PromptPairwiseEvaluationOutcome::progressed(None));
            }
        }
    }
    let current_task_class = dataset
        .first()
        .map(|case| case.task_class.as_str())
        .unwrap_or("general");
    let Some(challenger) = distillation
        .as_ref()
        .map(|evaluation| evaluation.child.clone())
        .or_else(|| {
            prompt_rollout_counterpart(rollout.as_ref(), &known_profiles, current_profile).or_else(
                || prompt_evolution_challenger(&evaluation, current_profile, current_task_class),
            )
        })
    else {
        return Ok(PromptPairwiseEvaluationOutcome::no_work());
    };
    let (current_counts, challenger_counts) = if let Some(distillation) = distillation.as_ref() {
        let counts = crate::prompt_distillation_runtime::prompt_distillation_evidence_counts(
            &evaluation.observations,
            &distillation.provenance,
            &distillation.dataset.dataset_sha256,
        );
        (counts, counts)
    } else {
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
        (current_counts, challenger_counts)
    };
    let required_holdout_runs = distillation
        .as_ref()
        .map(|_| PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS)
        .unwrap_or_else(|| {
            rollout
                .as_ref()
                .filter(|rollout| {
                    rollout.canary_profile_id.as_ref().is_some_and(|canary| {
                        (current_profile.id == rollout.stable_profile_id
                            && challenger.id == *canary)
                            || (challenger.id == rollout.stable_profile_id
                                && current_profile.id == *canary)
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
                        PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS
                            .max(rollout.evidence_checkpoint.saturating_add(2))
                    })
                })
                .unwrap_or(PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS)
        });
    let proposal_gate = if distillation.is_some() {
        None
    } else if current_profile
        .parents
        .iter()
        .any(|parent| parent == &challenger.id)
    {
        Some(prompt_proposal_minibatch_decision(
            current_profile,
            &evaluation.observations,
            PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            PROMPT_EVOLUTION_MINIBATCH_RELATIVE_IMPROVEMENT,
        )?)
    } else if challenger
        .parents
        .iter()
        .any(|parent| parent == &current_profile.id)
    {
        Some(prompt_proposal_minibatch_decision(
            &challenger,
            &evaluation.observations,
            PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            PROMPT_EVOLUTION_MINIBATCH_RELATIVE_IMPROVEMENT,
        )?)
    } else {
        None
    };
    if proposal_gate
        .as_ref()
        .is_some_and(PromptProposalMinibatchDecision::is_rejected)
    {
        return Ok(PromptPairwiseEvaluationOutcome::no_work());
    }
    let minibatch_pending = proposal_gate
        .as_ref()
        .is_some_and(|decision| !decision.is_accepted());
    let split = if minibatch_pending
        || current_counts.0 < PROMPT_EVOLUTION_MIN_TRAIN_RUNS
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
            return Ok(PromptPairwiseEvaluationOutcome::no_work());
        }
        PromptEvaluationSplit::Train
    };
    let transfer_case = (effort == "pro")
        .then(|| {
            let (auto_profile, auto_profile_sha256) = auto_stable_profile.as_ref()?;
            let (current_gate, current_cohort) = evaluate_trusted_prompt_auto_transfer_gate(
                &scoped_model,
                &evaluation.observations,
                &current_profile.id,
                &auto_profile.id,
                auto_profile_sha256,
            );
            let (challenger_gate, challenger_cohort) = evaluate_trusted_prompt_auto_transfer_gate(
                &scoped_model,
                &evaluation.observations,
                &challenger.id,
                &auto_profile.id,
                auto_profile_sha256,
            );
            (!current_gate.eligible || !challenger_gate.eligible)
                .then(|| {
                    select_prompt_auto_transfer_case(
                        &dataset,
                        &evaluation.observations,
                        [&current_profile.id, &challenger.id],
                        [current_cohort.as_deref(), challenger_cohort.as_deref()],
                        &auto_profile.id,
                        auto_profile_sha256,
                        split,
                    )
                })
                .flatten()
        })
        .flatten();
    let selected_case = distillation
        .as_ref()
        .and_then(|distillation| {
            crate::prompt_distillation_runtime::select_prompt_distillation_case(
                &evaluation.observations,
                distillation,
                split,
            )
        })
        .or(transfer_case)
        .or_else(|| {
            distillation
                .is_none()
                .then(|| {
                    select_prompt_offline_case(
                        &dataset,
                        &evaluation.observations,
                        &current_profile.id,
                        &challenger.id,
                        split,
                    )
                })
                .flatten()
        });
    let Some(selected_case) = selected_case else {
        return Ok(PromptPairwiseEvaluationOutcome::no_work());
    };
    let auto_teacher = (effort == "pro")
        .then(|| {
            let (auto_profile, auto_profile_sha256) = auto_stable_profile.as_ref()?;
            selected_case.auto_teacher.as_ref().filter(|teacher| {
                teacher.profile_id == auto_profile.id
                    && teacher.profile_sha256 == *auto_profile_sha256
            })
        })
        .flatten();
    let excluded_reviewer_models = auto_teacher
        .map(|teacher| teacher.participant_models.iter().cloned().collect())
        .unwrap_or_default();
    let (evaluation_worker_models, reserved_evaluator_models) =
        prompt_evaluation_model_partition(config, worker_models, &excluded_reviewer_models);
    if reserved_evaluator_models.is_empty() {
        return Ok(PromptPairwiseEvaluationOutcome::no_work());
    }
    if distillation.is_none() {
        append_prompt_offline_dataset_snapshot(
            state,
            task_id,
            run_context,
            effort,
            &dataset,
            campaign_generation,
            Some(&selected_case),
        )?;
    }
    let mode = match split {
        PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
        PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
    };
    let objective = selected_case.objective.clone();
    let task_class = selected_case.task_class.clone();
    let evaluation_id = scoped_prompt_evaluation_id(
        project_id,
        &unique_id(match mode {
            PromptEvaluationMode::PairedShadow | PromptEvaluationMode::PairedExecution => {
                "prompt-pair"
            }
            PromptEvaluationMode::ReplayHoldout | PromptEvaluationMode::ReplayExecution => {
                "prompt-replay"
            }
            PromptEvaluationMode::Live => "prompt-live",
        }),
    );
    let mut candidate_lanes =
        PromptTreatmentControlGroup::new(control.as_ref(), 2, PROMPT_MATCHED_EVALUATION_LANES)?;
    let current_control = candidate_lanes.lane(0);
    let challenger_control = candidate_lanes.lane(1);
    let matched_treatment_budget = candidate_lanes.lane_budget();
    let execution_context = crate::prompt_learning_runtime::prompt_execution_context(
        config,
        effort,
        policy,
        &evaluation_worker_models,
        agent_budget,
        current_profile,
        &workspace_root,
        matched_treatment_budget,
        control.as_ref(),
    )?;
    let learning_cohort = if let Some(distillation) = distillation.as_ref() {
        PromptLearningCohortV1::new(distillation.dataset.clone(), execution_context)?
    } else {
        crate::prompt_learning_runtime::prompt_learning_cohort(
            &dataset,
            campaign_generation,
            execution_context,
        )?
    };
    let matched_identity = PromptMatchedEvaluationIdentityV1::new(
        evaluation_id.clone(),
        &learning_cohort,
        selected_case.id.clone(),
        split,
        mode,
    )?;
    let current_prompt_sha256 = prompt_genome_sha256(current_profile)?;
    let challenger_prompt_sha256 = prompt_genome_sha256(&challenger)?;
    let attempt_started = PromptEvaluationAttemptEventV1::started(
        matched_identity.clone(),
        &learning_cohort,
        [
            PromptTreatmentIdentityV1 {
                profile_id: current_profile.id.clone(),
                prompt_sha256: current_prompt_sha256.clone(),
            },
            PromptTreatmentIdentityV1 {
                profile_id: challenger.id.clone(),
                prompt_sha256: challenger_prompt_sha256.clone(),
            },
        ],
    )?;
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
    let mut attempt = crate::prompt_attempt_runtime::PromptEvaluationAttemptGuard::start(
        state,
        task_id,
        run_context,
        effort,
        &learning_cohort,
        attempt_started,
    )?;

    control.mark_progress("prompt_evaluation", "matched treatment lanes started");

    let (current_plan, challenger_plan) = std::thread::scope(|scope| {
        let current = scope.spawn(|| {
            evaluate_conductor_prompt_profile(
                config,
                &objective,
                effort,
                policy,
                &evaluation_worker_models,
                agent_budget,
                current_profile,
                &evaluation_id,
                &current_control,
            )
        });
        let challenger_handle = scope.spawn(|| {
            evaluate_conductor_prompt_profile(
                config,
                &objective,
                effort,
                policy,
                &evaluation_worker_models,
                agent_budget,
                &challenger,
                &evaluation_id,
                &challenger_control,
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
    if [current_control.as_ref(), challenger_control.as_ref()]
        .into_iter()
        .any(|lane| lane.stop_reason() == Some(agent_runtime::RunStopReason::UserCancelled))
    {
        attempt.finish(
            PromptEvaluationAttemptStatus::ForegroundPreempted,
            "foreground_preempted",
        )?;
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let (current, challenger) = std::thread::scope(|scope| {
        let current_handle = scope.spawn(|| {
            execute_prompt_workflow_candidate(
                config,
                &workspace_root,
                &objective,
                current_plan,
                &current_control,
            )
        });
        let challenger_handle = scope.spawn(|| {
            execute_prompt_workflow_candidate(
                config,
                &workspace_root,
                &objective,
                challenger_plan,
                &challenger_control,
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
    if [current_control.as_ref(), challenger_control.as_ref()]
        .into_iter()
        .any(|lane| lane.stop_reason() == Some(agent_runtime::RunStopReason::UserCancelled))
    {
        attempt.finish(
            PromptEvaluationAttemptStatus::ForegroundPreempted,
            "foreground_preempted",
        )?;
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    candidate_lanes.absorb()?;
    let current_execution_context = crate::prompt_learning_runtime::prompt_execution_context(
        config,
        effort,
        policy,
        &evaluation_worker_models,
        agent_budget,
        current_profile,
        &workspace_root,
        matched_treatment_budget,
        control.as_ref(),
    )?;
    if current_execution_context != learning_cohort.execution {
        attempt.finish(
            PromptEvaluationAttemptStatus::InfrastructureInvalid,
            "execution_context_changed",
        )?;
        return Err("prompt evaluation execution context changed during replay".to_string());
    }
    let candidate_a = &current;
    let candidate_b = &challenger;
    let treatment_failures = [candidate_a, candidate_b].map(|candidate| {
        candidate.plan.plan.is_none()
            || !candidate.execution.succeeded
            || !candidate.execution.quality_gate_met
    });
    let treatment_failed = treatment_failures.into_iter().any(|failed| failed);
    if treatment_failed {
        attempt.mark_treatment_failures(treatment_failures);
        attempt.finish(
            PromptEvaluationAttemptStatus::TreatmentFailure,
            "treatment_execution_failed",
        )?;
        if distillation.is_some() {
            return Ok(PromptPairwiseEvaluationOutcome::no_work());
        }
    }
    let participant_models = prompt_candidate_models([candidate_a, candidate_b]);
    let evaluator_models = reserved_evaluator_models
        .into_iter()
        .filter(|model| !model.trim().is_empty())
        .collect::<Vec<_>>();
    let reviewer_model = evaluator_models
        .first()
        .cloned()
        .unwrap_or_else(|| config.model_for_role(&ModelRole::Reviewer));
    let participant_models = participant_models.into_iter().collect::<Vec<_>>();
    let mut reviewer_lanes = match PromptTreatmentControlGroup::new(control.as_ref(), 2, 2) {
        Ok(lanes) => lanes,
        Err(error) => {
            attempt.finish(
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                "reviewer_budget_unavailable",
            )?;
            return Err(error);
        }
    };
    let forward_reviewer_control = reviewer_lanes.lane(0);
    let reverse_reviewer_control = reviewer_lanes.lane(1);
    let judge_result = prompt_evaluation_feedback::evaluate_prompt_candidate_pair_position_balanced(
        config,
        &reviewer_model,
        &objective,
        candidate_a,
        candidate_b,
        &evaluation_id,
        &forward_reviewer_control,
        &reverse_reviewer_control,
    );
    let reviewer_preempted = [
        forward_reviewer_control.as_ref(),
        reverse_reviewer_control.as_ref(),
    ]
    .into_iter()
    .any(|lane| lane.stop_reason() == Some(agent_runtime::RunStopReason::UserCancelled));
    if let Err(error) = reviewer_lanes.absorb() {
        attempt.finish(
            PromptEvaluationAttemptStatus::InfrastructureInvalid,
            "reviewer_accounting_failed",
        )?;
        return Err(error);
    }
    let judge = match judge_result {
        Ok(judge) => judge,
        Err(error) => {
            let (status, reason) = if error == MODEL_REQUEST_CANCELLED || reviewer_preempted {
                (
                    PromptEvaluationAttemptStatus::ForegroundPreempted,
                    "foreground_preempted",
                )
            } else {
                (
                    PromptEvaluationAttemptStatus::InfrastructureInvalid,
                    "reviewer_invalid",
                )
            };
            attempt.finish(status, reason)?;
            return Err(error);
        }
    };
    if reviewer_preempted {
        attempt.finish(
            PromptEvaluationAttemptStatus::ForegroundPreempted,
            "foreground_preempted",
        )?;
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    if control.should_stop() {
        let reason = control.stop_reason();
        let (status, reason_code, error) =
            if reason == Some(agent_runtime::RunStopReason::UserCancelled) {
                (
                    PromptEvaluationAttemptStatus::ForegroundPreempted,
                    "foreground_preempted",
                    MODEL_REQUEST_CANCELLED.to_string(),
                )
            } else {
                (
                    PromptEvaluationAttemptStatus::InfrastructureInvalid,
                    "parent_budget_exhausted",
                    "prompt evaluation parent budget expired before persistence".to_string(),
                )
            };
        attempt.finish(status, reason_code)?;
        return Err(error);
    }
    let final_execution_context = crate::prompt_learning_runtime::prompt_execution_context(
        config,
        effort,
        policy,
        &evaluation_worker_models,
        agent_budget,
        current_profile,
        &workspace_root,
        matched_treatment_budget,
        control.as_ref(),
    )?;
    if final_execution_context != learning_cohort.execution {
        attempt.finish(
            PromptEvaluationAttemptStatus::InfrastructureInvalid,
            "execution_context_changed",
        )?;
        return Err("prompt evaluation execution context changed before persistence".to_string());
    }
    let split = match mode {
        PromptEvaluationMode::ReplayHoldout | PromptEvaluationMode::ReplayExecution => {
            PromptEvaluationSplit::Holdout
        }
        PromptEvaluationMode::PairedShadow
        | PromptEvaluationMode::PairedExecution
        | PromptEvaluationMode::Live => PromptEvaluationSplit::Train,
    };
    let dataset_sha256 = learning_cohort.dataset.dataset_sha256.clone();
    let prompt_sha_a = current_prompt_sha256;
    let prompt_sha_b = challenger_prompt_sha256;
    let provenance_a = PromptEvaluationProvenance::blind_pairwise_swap(
        vec![reviewer_model.clone()],
        participant_models.clone(),
        dataset_sha256.clone(),
        prompt_sha_a.clone(),
        prompt_sha_b.clone(),
    )
    .with_matched_evaluation(matched_identity.clone());
    let provenance_a = if let Some(distillation) = distillation.as_ref() {
        provenance_a.with_pro_to_auto_distillation(distillation.provenance.clone())
    } else {
        provenance_a
    };
    let observation_a = prompt_evaluation_feedback::prompt_pairwise_observation(
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
        provenance_a,
    );
    let provenance_b = PromptEvaluationProvenance::blind_pairwise_swap(
        vec![reviewer_model.clone()],
        participant_models,
        dataset_sha256,
        prompt_sha_b,
        prompt_sha_a,
    )
    .with_matched_evaluation(matched_identity);
    let provenance_b = if let Some(distillation) = distillation.as_ref() {
        provenance_b.with_pro_to_auto_distillation(distillation.provenance.clone())
    } else {
        provenance_b
    };
    let observation_b = prompt_evaluation_feedback::prompt_pairwise_observation(
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
        provenance_b,
    );
    let lease = match control.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        agent_runtime::RunEpochLeaseOutcome::Stopped(
            agent_runtime::RunStopReason::UserCancelled,
        ) => {
            attempt.finish(
                PromptEvaluationAttemptStatus::ForegroundPreempted,
                "foreground_preempted",
            )?;
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        agent_runtime::RunEpochLeaseOutcome::Stopped(_)
        | agent_runtime::RunEpochLeaseOutcome::RestartAfterSteer
        | agent_runtime::RunEpochLeaseOutcome::TerminalCommitted => {
            attempt.finish(
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                "persistence_lease_unavailable",
            )?;
            return Err("prompt evaluation persistence lease is unavailable".to_string());
        }
    };
    let observation_commit = control.commit_execution_step_with(lease, || {
        if let Some(distillation) = distillation.as_ref() {
            crate::prompt_distillation_runtime::append_prompt_distillation_observations(
                state,
                task_id,
                run_context,
                mode,
                [&observation_a, &observation_b],
                [&candidate_a.plan.genome, &candidate_b.plan.genome],
                distillation,
            )
        } else {
            append_prompt_pairwise_observations(
                state,
                task_id,
                run_context,
                effort,
                mode,
                [&observation_a, &observation_b],
                [&candidate_a.plan.genome, &candidate_b.plan.genome],
            )
        }
    });
    match observation_commit {
        Ok(agent_runtime::RunExecutionStepCommit::Committed(())) => {}
        Ok(agent_runtime::RunExecutionStepCommit::Stopped(
            agent_runtime::RunStopReason::UserCancelled,
        )) => {
            attempt.finish(
                PromptEvaluationAttemptStatus::ForegroundPreempted,
                "foreground_preempted",
            )?;
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        Ok(agent_runtime::RunExecutionStepCommit::Stopped(_))
        | Ok(agent_runtime::RunExecutionStepCommit::RestartAfterSteer)
        | Ok(agent_runtime::RunExecutionStepCommit::TerminalCommitted) => {
            attempt.finish(
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                "observation_persistence_stopped",
            )?;
            return Err("prompt evaluation stopped before observation persistence".to_string());
        }
        Err(error) => {
            attempt.finish(
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                "observation_persistence_failed",
            )?;
            return Err(error);
        }
    }
    if treatment_failed {
        attempt.finish(
            PromptEvaluationAttemptStatus::TreatmentFailure,
            "treatment_execution_failed",
        )?;
    } else {
        attempt.finish(PromptEvaluationAttemptStatus::CompletedPair, "")?;
    }
    control.mark_progress("prompt_evaluation", "matched treatment pair persisted");
    if distillation.is_none() {
        crate::prompt_transfer_runtime::run_prompt_auto_transfer_evaluations(
            state,
            config,
            task_id,
            run_context,
            effort,
            policy,
            &evaluation_worker_models,
            agent_budget,
            &workspace_root,
            &dataset,
            auto_teacher,
            auto_stable_profile.as_ref(),
            &selected_case,
            split,
            mode,
            control,
            &evaluation_id,
            &objective,
            &task_class,
            &reviewer_model,
            [candidate_a, candidate_b],
        )?;
    }
    let reconciliation =
        crate::prompt_evolution_store_runtime::with_prompt_evolution_store(state, |store| {
            crate::prompt_evolution_runtime::reconcile_prompt_evolution_for_background(
                store,
                effort,
                project_id,
                run_context,
            )
        })?;
    if distillation.is_none() && reconciliation.deployment_recovery_error.is_none() {
        if let Some(parent) = reconciliation.evaluation.mutation_parent.as_ref() {
            if attempted_mutation_parent.as_deref() != Some(parent.id.as_str()) {
                let _ = generate_background_prompt_mutation(
                    state,
                    config,
                    task_id,
                    run_context,
                    effort,
                    parent,
                    &reconciliation.evaluation.mutation_trajectories,
                    control,
                )?;
            }
        }
    }
    Ok(PromptPairwiseEvaluationOutcome::progressed(
        reconciliation.deployment_recovery_error,
    ))
}

#[cfg(test)]
#[path = "prompt_pairwise_runtime_tests.rs"]
mod tests;
