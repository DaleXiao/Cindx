use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_adaptive_collaboration(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    collaboration_id: &str,
    prompt: &str,
    history: &[Message],
    models: &[String],
    agent_budget: usize,
) -> Result<String, String> {
    if models.is_empty() {
        return Err("adaptive collaboration has no configured worker models".to_string());
    }
    let workflow_started_at_ms = current_time_millis();
    let conductor_model = config.model_for_conductor();
    let role_hints = collaboration_role_hints(config, models);
    let effort = run_context
        .get("agent_effort")
        .cloned()
        .unwrap_or_else(|| "auto".to_string());
    let policy = run_context
        .get("collaboration_policy")
        .cloned()
        .unwrap_or_else(|| "best_of_n".to_string());
    let base_execution_contract = run_context
        .get("conductor_contract")
        .map(String::as_str)
        .map(ConductorExecutionContract::from_json)
        .transpose()?
        .unwrap_or_else(|| {
            ConductorExecutionContract::from_routing(
                &RoutingContext::from_prompt(prompt, Vec::new()),
                &effort,
                parse_policy(&policy).unwrap_or(OrchestrationPolicy::AutoRouter),
            )
        });
    let (resume_key, mut workflow_checkpoint) = load_workflow_checkpoint_for_run(
        state,
        task_id,
        run_context,
        prompt,
        &effort,
        &policy,
        models,
    )?;
    let resumed_from_workflow_id = workflow_checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.plan.workflow_id.clone());
    if let Some(checkpoint) = workflow_checkpoint.as_mut() {
        checkpoint.plan.workflow_id = collaboration_id.to_string();
        let additional_turns = checkpoint.plan.budget.max_model_turns_per_step;
        checkpoint.continue_with_budget(additional_turns, workflow_started_at_ms);
    }
    let resumed_from_checkpoint = workflow_checkpoint.is_some();
    let prior = if resumed_from_checkpoint {
        None
    } else {
        workflow_prior_for_run(state, run_context, models, agent_budget)?
    };
    let evolution = if !resumed_from_checkpoint && config.prompt_evolution_enabled {
        Some(prompt_evolution_evaluation_for_run(
            state,
            &effort,
            run_context,
        )?)
    } else {
        None
    };
    let selection_mode = if resumed_from_checkpoint {
        "checkpoint_resume".to_string()
    } else {
        evolution
            .as_ref()
            .map(|evaluation| evaluation.next_mode.clone())
            .unwrap_or_else(|| "baseline".to_string())
    };
    let mut prompt_genome = workflow_checkpoint
        .as_ref()
        .and_then(|checkpoint| {
            serde_json::from_str::<ConductorPromptGenome>(&checkpoint.prompt_genome_json).ok()
        })
        .or_else(|| {
            evolution
                .as_ref()
                .map(|evaluation| evaluation.next_profile.clone())
        })
        .unwrap_or_else(|| ConductorPromptGenome::seed_for_effort(&effort));
    if resumed_from_checkpoint {
        if let Some(profile) = workflow_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.plan.prompt_profile.clone())
        {
            prompt_genome.id = profile;
        }
    }
    let selected_prompt_genome = prompt_genome.clone();
    prompt_genome = prompt_genome.with_effort_capability_floor(&effort);
    let prompt_capability_floor_applied = prompt_genome != selected_prompt_genome;
    let execution_contract =
        base_execution_contract.with_prompt_commit_strategy(prompt_genome.commit_strategy);
    let prompt_genome_json = serde_json::to_string(&prompt_genome)
        .map_err(|error| format!("failed to serialize prompt genome: {error}"))?;
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile selected",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("prompt_profile".to_string(), prompt_genome.id.clone()),
                    ("prompt_effort".to_string(), effort.clone()),
                    (
                        "prompt_generation".to_string(),
                        prompt_genome.generation.to_string(),
                    ),
                    ("prompt_genome".to_string(), prompt_genome_json.clone()),
                    ("prompt_selection_mode".to_string(), selection_mode.clone()),
                    (
                        "prompt_capability_floor_applied".to_string(),
                        prompt_capability_floor_applied.to_string(),
                    ),
                    ("workflow_resume_key".to_string(), resume_key.clone()),
                    (
                        "prompt_evolution_status".to_string(),
                        if resumed_from_checkpoint {
                            "checkpoint_resume".to_string()
                        } else {
                            evolution
                                .as_ref()
                                .map(|evaluation| evaluation.status.clone())
                                .unwrap_or_else(|| "disabled".to_string())
                        },
                    ),
                    (
                        "prompt_champion".to_string(),
                        evolution
                            .as_ref()
                            .and_then(|evaluation| evaluation.champion_id.clone())
                            .unwrap_or_default(),
                    ),
                    (
                        "prompt_evolution_enabled".to_string(),
                        config.prompt_evolution_enabled.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    let shared_memory = collaboration_context_for_genome(history, prompt_genome.context_policy);
    let anchor_spec = direct_anchor_spec(&role_hints, prompt, &shared_memory);
    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
    if collaboration_steer_pending(cancellation.as_ref()) {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let mut direct_anchor_output = workflow_checkpoint.as_ref().and_then(|checkpoint| {
        checkpoint
            .anytime_outputs
            .get(DIRECT_ANCHOR_CANDIDATE_ID)
            .cloned()
    });
    let mut anchor_supervisor = None;
    let mut direct_anchor_attempted = false;
    if direct_anchor_output.is_none() {
        let anchor_role = adaptive_model_role(&anchor_spec.role);
        record_collaboration_stage_started(
            state,
            task_id,
            run_context,
            collaboration_id,
            &anchor_spec.stage,
            &anchor_role,
            &anchor_spec.model,
            &anchor_spec.request_id,
            &direct_anchor_metadata(&anchor_spec),
        )?;

        if effort == "fast" && !resumed_from_checkpoint {
            direct_anchor_attempted = true;
            let completion = complete_collaboration_worker_with_tools(
                app.clone(),
                config.clone(),
                task_id.clone(),
                workspace_root.to_path_buf(),
                run_context.clone(),
                collaboration_id.to_string(),
                anchor_spec.stage.clone(),
                anchor_role,
                anchor_spec.model.clone(),
                anchor_spec.prompt.clone(),
                false,
                anchor_spec.max_model_turns,
                anchor_spec.max_tool_calls,
                cancellation.clone(),
                None,
            );
            if collaboration_steer_pending(cancellation.as_ref()) {
                return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
            }
            direct_anchor_output = record_direct_anchor_completion(
                state,
                task_id,
                run_context,
                collaboration_id,
                &anchor_spec,
                &completion,
                prompt_genome.verification,
                cancellation.as_ref(),
            )?;
            if let Some(output) = direct_anchor_output.as_ref() {
                let mut controller = AnytimeController::new(
                    AnytimeControllerConfig::from_contract(&execution_contract),
                );
                controller.register(AnytimeCandidate::direct_anchor(DIRECT_ANCHOR_CANDIDATE_ID))?;
                controller.mark_running(DIRECT_ANCHOR_CANDIDATE_ID)?;
                controller.observe(
                    DIRECT_ANCHOR_CANDIDATE_ID,
                    direct_anchor_verdict(prompt_genome.verification, Some(output)),
                )?;
                if matches!(
                    controller.decision(u64::MAX, 0),
                    AnytimeDecision::Commit { .. }
                ) {
                    if let Ok(mut store) = state.store.lock() {
                        let _ = append_event(
                            &mut store,
                            task_id,
                            EventKind::TaskStatusChanged,
                            "Anytime direct anchor committed",
                            metadata_with_context(
                                [
                                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                                    ("effort".to_string(), effort.clone()),
                                    ("conductor_skipped".to_string(), "true".to_string()),
                                ]
                                .into_iter()
                                .collect(),
                                run_context,
                            ),
                        );
                    }
                    return Ok(output.clone());
                }
            }
        } else {
            let mut supervisor = model_job_supervisor::<CollaborationCompletion>();
            let app = app.clone();
            let config = config.clone();
            let task_id = task_id.clone();
            let workspace_root = workspace_root.to_path_buf();
            let run_context = run_context.clone();
            let collaboration_id = collaboration_id.to_string();
            let stage = anchor_spec.stage.clone();
            let model = anchor_spec.model.clone();
            let prompt = anchor_spec.prompt.clone();
            let max_model_turns = anchor_spec.max_model_turns;
            let max_tool_calls = anchor_spec.max_tool_calls;
            let cancellation = cancellation.clone();
            supervisor
                .submit(
                    DIRECT_ANCHOR_JOB_ID,
                    "direct-anchor",
                    Box::new(move |branch_cancellation| {
                        complete_collaboration_worker_with_tools(
                            app,
                            config,
                            task_id,
                            workspace_root,
                            run_context,
                            collaboration_id,
                            stage,
                            ModelRole::Executor,
                            model,
                            prompt,
                            false,
                            max_model_turns,
                            max_tool_calls,
                            cancellation,
                            Some(branch_cancellation),
                        )
                    }),
                )
                .map_err(|error| format!("direct anchor could not start: {error}"))?;
            anchor_supervisor = Some(supervisor);
        }
    }
    let (workflow_plan, conductor_attempts) = if let Some(checkpoint) = workflow_checkpoint.as_ref()
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration workflow resumed",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("workflow_resume_key".to_string(), resume_key.clone()),
                    (
                        "resumed_from_workflow_id".to_string(),
                        resumed_from_workflow_id.clone().unwrap_or_default(),
                    ),
                    (
                        "completed_steps".to_string(),
                        checkpoint.completed_step_count().to_string(),
                    ),
                    (
                        "workflow_steps".to_string(),
                        checkpoint.plan.steps.len().to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        (checkpoint.plan.clone(), 0)
    } else {
        let harness = ConductorHarness::new(ConductorRequest {
            workflow_id: collaboration_id.to_string(),
            objective: prompt.to_string(),
            recent_context: shared_memory.clone(),
            effort: effort.clone(),
            policy: policy.clone(),
            conductor_model: conductor_model.clone(),
            worker_models: models.to_vec(),
            role_hints: role_hints.clone(),
            budget: WorkflowBudget {
                max_steps: adaptive_workflow_step_budget(agent_budget),
                max_models: agent_budget.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS),
                max_model_turns_per_step: DEFAULT_COLLABORATION_WORKER_TURNS,
                max_tool_calls_per_step: MAX_COLLABORATION_WORKER_TOOL_CALLS,
                max_output_tokens_per_step: COLLABORATION_MAX_OUTPUT_TOKENS as usize,
            },
            execution_contract: execution_contract.clone(),
            prior_hint: prior.as_ref().map(WorkflowTopologyPrior::prompt_hint),
            prompt_evolution_enabled: config.prompt_evolution_enabled,
            prompt_genome: prompt_genome.clone(),
        });
        let conductor_response = run_collaboration_stage(
            state,
            config,
            task_id,
            run_context,
            collaboration_id,
            "conductor_plan",
            ModelRole::Planner,
            &conductor_model,
            harness.planning_prompt(),
        );
        match conductor_response {
            Ok(mut conductor_response) => {
                let mut conductor_attempts = 1usize;
                let workflow_plan = loop {
                    if collaboration_steer_pending(cancellation.as_ref()) {
                        cancel_anytime_background(anchor_supervisor.as_ref(), None);
                        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
                    }
                    match harness.parse_plan(&conductor_response) {
                        Ok(plan) => break plan,
                        Err(error) if conductor_attempts < CONDUCTOR_MAX_ATTEMPTS => {
                            if let Some(control) = cancellation.as_ref() {
                                if let Err(reason) =
                                    control.begin_repair_attempt("conductor_repair")
                                {
                                    if effort != "fast" {
                                        break deterministic_conductor_fallback(
                                            state,
                                            task_id,
                                            run_context,
                                            collaboration_id,
                                            &harness,
                                            conductor_attempts,
                                            &format!("repair budget exhausted: {}", reason.code()),
                                        )?;
                                    }
                                    if let Some(anchor) = await_direct_anchor_fallback(
                                        state,
                                        task_id,
                                        run_context,
                                        collaboration_id,
                                        &anchor_spec,
                                        prompt_genome.verification,
                                        cancellation.as_ref(),
                                        &mut anchor_supervisor,
                                        &mut direct_anchor_output,
                                        Duration::from_millis(500),
                                    )? {
                                        return Ok(anchor);
                                    }
                                    return Err(format!(
                                        "Conductor repair budget exhausted: {}",
                                        reason.code()
                                    ));
                                }
                            }
                            if let Ok(mut store) = state.store.lock() {
                                let _ = append_event(
                                    &mut store,
                                    task_id,
                                    EventKind::TaskStatusChanged,
                                    "Conductor workflow rejected",
                                    metadata_with_context(
                                        [
                                            (
                                                "collaboration_id".to_string(),
                                                collaboration_id.to_string(),
                                            ),
                                            ("attempt".to_string(), conductor_attempts.to_string()),
                                            (
                                                "validation_error".to_string(),
                                                truncate_for_collaboration(&error, 2_000),
                                            ),
                                        ]
                                        .into_iter()
                                        .collect(),
                                        run_context,
                                    ),
                                );
                            }
                            conductor_response = match run_collaboration_stage(
                                state,
                                config,
                                task_id,
                                run_context,
                                collaboration_id,
                                "conductor_repair",
                                ModelRole::Planner,
                                &conductor_model,
                                harness.repair_prompt(&conductor_response, &error),
                            ) {
                                Ok(response) => response,
                                Err(repair_error) => {
                                    if repair_error == COLLABORATION_STEER_INTERRUPTED
                                        || collaboration_steer_pending(cancellation.as_ref())
                                    {
                                        cancel_anytime_background(anchor_supervisor.as_ref(), None);
                                        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
                                    }
                                    if effort != "fast" {
                                        break deterministic_conductor_fallback(
                                            state,
                                            task_id,
                                            run_context,
                                            collaboration_id,
                                            &harness,
                                            conductor_attempts,
                                            &format!("repair stage failed: {repair_error}"),
                                        )?;
                                    }
                                    if let Some(anchor) = await_direct_anchor_fallback(
                                        state,
                                        task_id,
                                        run_context,
                                        collaboration_id,
                                        &anchor_spec,
                                        prompt_genome.verification,
                                        cancellation.as_ref(),
                                        &mut anchor_supervisor,
                                        &mut direct_anchor_output,
                                        Duration::from_millis(500),
                                    )? {
                                        return Ok(anchor);
                                    }
                                    return Err(format!(
                                        "Conductor repair failed after {conductor_attempts} attempt(s): {repair_error}"
                                    ));
                                }
                            };
                            conductor_attempts += 1;
                        }
                        Err(error) => {
                            if effort != "fast" {
                                break deterministic_conductor_fallback(
                                    state,
                                    task_id,
                                    run_context,
                                    collaboration_id,
                                    &harness,
                                    conductor_attempts,
                                    &format!("invalid workflow after repair: {error}"),
                                )?;
                            }
                            if let Some(anchor) = await_direct_anchor_fallback(
                                state,
                                task_id,
                                run_context,
                                collaboration_id,
                                &anchor_spec,
                                prompt_genome.verification,
                                cancellation.as_ref(),
                                &mut anchor_supervisor,
                                &mut direct_anchor_output,
                                Duration::from_millis(500),
                            )? {
                                return Ok(anchor);
                            }
                            return Err(format!(
                                "Conductor failed to produce a valid workflow after {conductor_attempts} attempts: {error}"
                            ));
                        }
                    }
                };
                (workflow_plan, conductor_attempts)
            }
            Err(error) => {
                if error == COLLABORATION_STEER_INTERRUPTED
                    || collaboration_steer_pending(cancellation.as_ref())
                {
                    cancel_anytime_background(anchor_supervisor.as_ref(), None);
                    return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
                }
                if effort != "fast" {
                    let plan = deterministic_conductor_fallback(
                        state,
                        task_id,
                        run_context,
                        collaboration_id,
                        &harness,
                        1,
                        &format!("planning stage failed: {error}"),
                    )?;
                    (plan, 1)
                } else {
                    if let Some(anchor) = await_direct_anchor_fallback(
                        state,
                        task_id,
                        run_context,
                        collaboration_id,
                        &anchor_spec,
                        prompt_genome.verification,
                        cancellation.as_ref(),
                        &mut anchor_supervisor,
                        &mut direct_anchor_output,
                        Duration::from_millis(500),
                    )? {
                        return Ok(anchor);
                    }
                    return Err(error);
                }
            }
        }
    };
    let workflow = workflow_plan.adaptive_workflow();
    let layers = adaptive_workflow_layers(&workflow)?;
    let role_coverage = workflow_role_coverage(&workflow, &role_hints);
    let layer_count = layers.len();
    let workflow_ir = workflow_plan.to_json()?;
    let mut workflow_checkpoint = workflow_checkpoint.take().unwrap_or_else(|| {
        WorkflowExecutionCheckpoint::new(
            resume_key.clone(),
            workflow_plan.clone(),
            workflow_started_at_ms,
        )
    });
    workflow_checkpoint.plan = workflow_plan.clone();
    workflow_checkpoint.prompt_genome_json = prompt_genome_json.clone();
    workflow_checkpoint.validate(models)?;
    let mut anytime_controller =
        initialize_anytime_controller(&execution_contract, &workflow, &workflow_checkpoint)?;
    let mut direct_anchor_verifier = None;
    let mut direct_anchor_verifier_attempted = false;
    if let Some(output) = direct_anchor_output.as_ref() {
        if anytime_controller
            .candidate(DIRECT_ANCHOR_CANDIDATE_ID)
            .is_some_and(|candidate| {
                matches!(
                    candidate.state,
                    AnytimeCandidateState::Pending | AnytimeCandidateState::Running
                )
            })
        {
            controller_mark_running_if_pending(
                &mut anytime_controller,
                DIRECT_ANCHOR_CANDIDATE_ID,
            )?;
            anytime_controller.observe(
                DIRECT_ANCHOR_CANDIDATE_ID,
                direct_anchor_verdict(prompt_genome.verification, Some(output)),
            )?;
        }
        workflow_checkpoint
            .anytime_outputs
            .insert(DIRECT_ANCHOR_CANDIDATE_ID.to_string(), output.clone());
    } else if anchor_supervisor.is_some() {
        controller_mark_running_if_pending(&mut anytime_controller, DIRECT_ANCHOR_CANDIDATE_ID)?;
    } else if direct_anchor_attempted {
        controller_mark_running_if_pending(&mut anytime_controller, DIRECT_ANCHOR_CANDIDATE_ID)?;
        anytime_controller.fail(DIRECT_ANCHOR_CANDIDATE_ID)?;
    }
    persist_anytime_controller(&mut workflow_checkpoint, &anytime_controller)?;
    {
        let workflow_summary = workflow
            .steps
            .iter()
            .map(|step| {
                format!(
                    "{}:{}:{}<-[{}]",
                    step.id,
                    step.role,
                    step.model,
                    step.access.join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        let unique_models = workflow
            .steps
            .iter()
            .map(|step| step.model.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration workflow planned",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "workflow_schema".to_string(),
                        WORKFLOW_IR_SCHEMA.to_string(),
                    ),
                    ("workflow_ir".to_string(), workflow_ir),
                    ("workflow_resume_key".to_string(), resume_key.clone()),
                    (
                        "workflow_checkpoint_schema".to_string(),
                        WORKFLOW_CHECKPOINT_SCHEMA.to_string(),
                    ),
                    ("resumed".to_string(), resumed_from_checkpoint.to_string()),
                    (
                        "prompt_profile".to_string(),
                        workflow_plan.prompt_profile.clone(),
                    ),
                    ("prompt_effort".to_string(), effort.clone()),
                    (
                        "prompt_generation".to_string(),
                        prompt_genome.generation.to_string(),
                    ),
                    ("prompt_genome".to_string(), prompt_genome_json.clone()),
                    ("prompt_selection_mode".to_string(), selection_mode.clone()),
                    (
                        "prompt_evolution_status".to_string(),
                        evolution
                            .as_ref()
                            .map(|evaluation| evaluation.status.clone())
                            .unwrap_or_else(|| "disabled".to_string()),
                    ),
                    ("conductor_version".to_string(), "agent_v2".to_string()),
                    ("conductor_model".to_string(), conductor_model.clone()),
                    (
                        "conductor_contract".to_string(),
                        execution_contract.to_json()?,
                    ),
                    (
                        "expected_collaboration_uplift_bps".to_string(),
                        execution_contract.expected_uplift_bps.to_string(),
                    ),
                    (
                        "conductor_attempts".to_string(),
                        conductor_attempts.to_string(),
                    ),
                    (
                        "conductor_source".to_string(),
                        if prior.is_some() {
                            "pareto_search_teacher_v2"
                        } else {
                            "model_cold_start"
                        }
                        .to_string(),
                    ),
                    (
                        "teacher_examples".to_string(),
                        prior
                            .as_ref()
                            .map(|prior| prior.examples)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    (
                        "workflow_steps".to_string(),
                        workflow.steps.len().to_string(),
                    ),
                    ("workflow_layers".to_string(), layers.len().to_string()),
                    ("worker_models".to_string(), unique_models.to_string()),
                    (
                        "role_aligned_steps".to_string(),
                        role_coverage.aligned_steps.to_string(),
                    ),
                    (
                        "role_total_steps".to_string(),
                        role_coverage.total_steps.to_string(),
                    ),
                    (
                        "independent_branch_models".to_string(),
                        role_coverage.independent_models.to_string(),
                    ),
                    (
                        "verifier_steps".to_string(),
                        role_coverage.verifier_steps.to_string(),
                    ),
                    (
                        "synthesizer_steps".to_string(),
                        role_coverage.synthesizer_steps.to_string(),
                    ),
                    (
                        "cross_reviewed".to_string(),
                        role_coverage.cross_reviewed.to_string(),
                    ),
                    (
                        "step_budget".to_string(),
                        adaptive_workflow_step_budget(agent_budget).to_string(),
                    ),
                    (
                        "workflow".to_string(),
                        truncate_for_collaboration(&workflow_summary, 4_000),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        if resumed_from_checkpoint {
            "Collaboration workflow checkpoint restored"
        } else {
            "Collaboration workflow checkpoint created"
        },
        if resumed_from_checkpoint {
            "resumed"
        } else {
            "planned"
        },
        None,
        &workflow_checkpoint,
    )?;
    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            &mut workflow_checkpoint,
            &anytime_controller,
            anchor_supervisor.as_ref(),
            direct_anchor_verifier.as_ref(),
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    advance_direct_anchor_background(
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        prompt,
        prompt_genome.verification,
        cancellation.as_ref(),
        &anchor_spec,
        &mut anchor_supervisor,
        &mut direct_anchor_output,
        &mut direct_anchor_verifier,
        &mut direct_anchor_verifier_attempted,
        &mut anytime_controller,
        &mut workflow_checkpoint,
    )?;
    let max_model_turns_per_step =
        effective_workflow_model_turn_budget(&workflow_plan, &workflow_checkpoint);
    let max_step_attempts =
        effective_workflow_step_attempt_budget(&prompt_genome, &workflow_checkpoint);
    let mut outputs = workflow_checkpoint.completed_outputs();
    let mut evidence_by_step = checkpoint_evidence_by_step(&workflow_checkpoint);
    let final_step_id = workflow
        .steps
        .last()
        .map(|step| step.id.clone())
        .ok_or_else(|| "adaptive workflow has no final step".to_string())?;

    let mut frontier_round = 0usize;
    loop {
        if collaboration_steer_pending(cancellation.as_ref()) {
            pause_anytime_for_steer(
                state,
                task_id,
                run_context,
                collaboration_id,
                &mut workflow_checkpoint,
                &anytime_controller,
                anchor_supervisor.as_ref(),
                direct_anchor_verifier.as_ref(),
            )?;
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        let ready_ids = anytime_controller
            .ready_candidates()
            .into_iter()
            .filter(|candidate| candidate.id != DIRECT_ANCHOR_CANDIDATE_ID)
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let layer = ready_ids
            .iter()
            .filter_map(|candidate_id| {
                workflow
                    .steps
                    .iter()
                    .position(|step| &step.id == candidate_id)
            })
            .filter(|step_index| {
                workflow_checkpoint
                    .steps
                    .get(&workflow.steps[*step_index].id)
                    .is_some_and(|step| {
                        !matches!(
                            step.status,
                            WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
                        )
                    })
            })
            .collect::<Vec<_>>();
        if layer.is_empty() {
            let all_workflow_steps_resolved = workflow_checkpoint.steps.values().all(|step| {
                matches!(
                    step.status,
                    WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
                )
            });
            if all_workflow_steps_resolved {
                break;
            }
            while (direct_anchor_output.is_none()
                && anchor_supervisor
                    .as_ref()
                    .is_some_and(|supervisor| supervisor.pending() > 0))
                || direct_anchor_verifier.is_some()
            {
                if collaboration_steer_pending(cancellation.as_ref()) {
                    pause_anytime_for_steer(
                        state,
                        task_id,
                        run_context,
                        collaboration_id,
                        &mut workflow_checkpoint,
                        &anytime_controller,
                        anchor_supervisor.as_ref(),
                        direct_anchor_verifier.as_ref(),
                    )?;
                    return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
                }
                advance_direct_anchor_background(
                    app,
                    state,
                    config,
                    task_id,
                    workspace_root,
                    run_context,
                    collaboration_id,
                    prompt,
                    prompt_genome.verification,
                    cancellation.as_ref(),
                    &anchor_spec,
                    &mut anchor_supervisor,
                    &mut direct_anchor_output,
                    &mut direct_anchor_verifier,
                    &mut direct_anchor_verifier_attempted,
                    &mut anytime_controller,
                    &mut workflow_checkpoint,
                )?;
                if direct_anchor_should_commit(&anytime_controller, cancellation.as_ref()) {
                    return direct_anchor_output.clone().ok_or_else(|| {
                        "anytime controller committed a missing direct anchor".to_string()
                    });
                }
                if cancellation.as_ref().is_some_and(agent_run_should_stop) {
                    return Err(MODEL_REQUEST_CANCELLED.to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Some((candidate_id, output, verdict)) =
                anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
            {
                if let Some(control) = cancellation.as_ref() {
                    control.record_best_known_result(
                        &format!("anytime_frontier_blocked:{candidate_id}"),
                        &output,
                        if verdict.verified {
                            ResultQuality::Verified
                        } else {
                            ResultQuality::Grounded
                        },
                        verdict.evidence_count,
                        verdict.verified,
                        false,
                    );
                }
                return Ok(output);
            }
            return Err(
                "anytime workflow frontier is blocked without runnable candidates".to_string(),
            );
        }
        let layer_index = frontier_round;
        frontier_round = frontier_round.saturating_add(1);
        if let Some(control) =
            active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?
        {
            control.mark_progress(
                "collaboration",
                &format!(
                    "Frontier wave {} · {}/{} steps restored",
                    layer_index + 1,
                    workflow_checkpoint.completed_step_count(),
                    workflow_plan.steps.len()
                ),
            );
        }
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                task_id,
                EventKind::TaskStatusChanged,
                format!("Anytime frontier wave {} started", layer_index + 1),
                metadata_with_context(
                    [
                        ("collaboration_id".to_string(), collaboration_id.to_string()),
                        ("layer".to_string(), (layer_index + 1).to_string()),
                        (
                            "planned_dependency_depth".to_string(),
                            layer_count.to_string(),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                    run_context,
                ),
            )
            .map_err(|error| error.to_string())?;
        }
        let specs = layer
            .into_iter()
            .map(|step_index| {
                let step = &workflow.steps[step_index];
                let worker_prompt =
                    adaptive_worker_prompt(&workflow, step_index, prompt, &shared_memory, &outputs)
                        .ok_or_else(|| {
                            format!("adaptive worker prompt is missing for {}", step.id)
                        })?;
                Ok(AdaptiveCollaborationSpec {
                    step_index,
                    step_id: step.id.clone(),
                    role: step.role.clone(),
                    stage: format!("worker_{}", step_index + 1),
                    model: workflow_checkpoint
                        .steps
                        .get(&step.id)
                        .map(|checkpoint| checkpoint.model.clone())
                        .unwrap_or_else(|| step.model.clone()),
                    subtask: step.subtask.clone(),
                    prompt: worker_prompt,
                    request_id: unique_id("collaboration-model"),
                    access: step.access.clone(),
                    tool_policy: workflow_plan.steps[step_index].tool_policy.clone(),
                    max_attempts: max_step_attempts,
                    max_model_turns: max_model_turns_per_step,
                    max_tool_calls: workflow_plan.budget.max_tool_calls_per_step,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        for spec in &specs {
            controller_mark_running_if_pending(&mut anytime_controller, &spec.step_id)?;
            persist_anytime_controller(&mut workflow_checkpoint, &anytime_controller)?;
            let started_new_attempt = ensure_adaptive_step_attempt_started(
                &mut workflow_checkpoint,
                &spec.step_id,
                &spec.model,
                spec.max_attempts,
                current_time_millis(),
            )?;
            append_workflow_checkpoint_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                if started_new_attempt {
                    "Collaboration workflow step started"
                } else {
                    "Collaboration workflow step resumed"
                },
                "running",
                Some(&spec.step_id),
                &workflow_checkpoint,
            )?;
            let metadata = adaptive_stage_metadata(spec);
            let role = adaptive_model_role(&spec.role);
            record_collaboration_stage_started(
                state,
                task_id,
                run_context,
                collaboration_id,
                &spec.stage,
                &role,
                &spec.model,
                &spec.request_id,
                &metadata,
            )?;
        }

        let cancellation =
            active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
        let independent_layer = specs.iter().all(|spec| spec.access.is_empty());
        let required_successes = if independent_layer
            && execution_contract.stop_policy != ConductorStopPolicy::Exhaustive
        {
            execution_contract.required_successes_for_layer(specs.len())
        } else {
            specs.len().max(1)
        };
        let jobs = specs
            .iter()
            .map(|spec| {
                let app = app.clone();
                let config = config.clone();
                let task_id = task_id.clone();
                let workspace_root = workspace_root.to_path_buf();
                let run_context = run_context.clone();
                let collaboration_id = collaboration_id.to_string();
                let stage = spec.stage.clone();
                let model = spec.model.clone();
                let role = adaptive_model_role(&spec.role);
                let prompt = spec.prompt.clone();
                let allow_tools = spec.tool_policy != WorkflowToolPolicy::None;
                let max_model_turns = spec.max_model_turns;
                let max_tool_calls = spec.max_tool_calls;
                let cancellation = cancellation.clone();
                Box::new(move |branch_cancellation| {
                    complete_collaboration_worker_with_tools(
                        app,
                        config,
                        task_id,
                        workspace_root,
                        run_context,
                        collaboration_id,
                        stage,
                        role,
                        model,
                        prompt,
                        allow_tools,
                        max_model_turns,
                        max_tool_calls,
                        cancellation,
                        Some(branch_cancellation),
                    )
                }) as CancellableParallelJob<CollaborationCompletion>
            })
            .collect::<Vec<_>>();
        let mut background_error = None;
        let mut anchor_commit_requested = false;
        let mut steer_interrupt_requested = false;
        let layer_execution = run_model_jobs_until_quorum_interruptible(
            "adaptive-worker",
            jobs,
            required_successes,
            Duration::from_millis(execution_contract.quorum_grace_ms()),
            Duration::from_millis(20),
            |completion| {
                completion
                    .content
                    .as_ref()
                    .is_some_and(|content| !content.trim().is_empty())
            },
            || {
                if collaboration_steer_pending(cancellation.as_ref()) {
                    steer_interrupt_requested = true;
                    return true;
                }
                if background_error.is_some() {
                    return true;
                }
                if let Err(error) = advance_direct_anchor_background(
                    app,
                    state,
                    config,
                    task_id,
                    workspace_root,
                    run_context,
                    collaboration_id,
                    prompt,
                    prompt_genome.verification,
                    cancellation.as_ref(),
                    &anchor_spec,
                    &mut anchor_supervisor,
                    &mut direct_anchor_output,
                    &mut direct_anchor_verifier,
                    &mut direct_anchor_verifier_attempted,
                    &mut anytime_controller,
                    &mut workflow_checkpoint,
                ) {
                    background_error = Some(error);
                    return true;
                }
                anchor_commit_requested =
                    direct_anchor_should_commit(&anytime_controller, cancellation.as_ref());
                anchor_commit_requested
            },
        );
        if steer_interrupt_requested || collaboration_steer_pending(cancellation.as_ref()) {
            pause_anytime_for_steer(
                state,
                task_id,
                run_context,
                collaboration_id,
                &mut workflow_checkpoint,
                &anytime_controller,
                anchor_supervisor.as_ref(),
                direct_anchor_verifier.as_ref(),
            )?;
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        if let Some(error) = background_error {
            if let Some((_candidate_id, output, _verdict)) =
                anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
            {
                return Ok(output);
            }
            return Err(error);
        }
        if layer_execution.interrupted && anchor_commit_requested {
            cancel_anytime_background(anchor_supervisor.as_ref(), direct_anchor_verifier.as_ref());
            append_workflow_checkpoint_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                "Anytime verified direct anchor committed",
                "completed",
                None,
                &workflow_checkpoint,
            )?;
            return direct_anchor_output
                .clone()
                .ok_or_else(|| "anytime controller committed a missing direct anchor".to_string());
        }
        let layer_execution = layer_execution.execution;
        if layer_execution.cancelled_stragglers > 0 {
            if let Ok(mut store) = state.store.lock() {
                let _ = append_event(
                    &mut store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Collaboration stragglers cancelled",
                    metadata_with_context(
                        [
                            ("collaboration_id".to_string(), collaboration_id.to_string()),
                            ("layer".to_string(), (layer_index + 1).to_string()),
                            (
                                "cancelled_stragglers".to_string(),
                                layer_execution.cancelled_stragglers.to_string(),
                            ),
                            (
                                "successful_branches".to_string(),
                                layer_execution.successful.to_string(),
                            ),
                            (
                                "required_branches".to_string(),
                                required_successes.to_string(),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        run_context,
                    ),
                );
            }
        }
        let completions = layer_execution
            .results
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|error| {
                    CollaborationCompletion::failed(format!(
                        "adaptive collaboration worker failed: {error}"
                    ))
                })
            })
            .collect::<Vec<_>>();

        let mut layer_failures = Vec::new();
        'completed_specs: for (spec, completion) in specs.iter().zip(completions) {
            if collaboration_steer_pending(cancellation.as_ref()) {
                pause_anytime_for_steer(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    &mut workflow_checkpoint,
                    &anytime_controller,
                    anchor_supervisor.as_ref(),
                    direct_anchor_verifier.as_ref(),
                )?;
                return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
            }
            let metadata = adaptive_stage_metadata(spec);
            let role = adaptive_model_role(&spec.role);
            record_collaboration_stage_finished(
                state,
                task_id,
                run_context,
                collaboration_id,
                &spec.stage,
                &role,
                &spec.model,
                &spec.request_id,
                &completion,
                &metadata,
            )?;
            if completion
                .content
                .as_ref()
                .is_none_or(|content| content.trim().is_empty())
                && cancellation.as_ref().is_some_and(agent_run_should_stop)
            {
                workflow_checkpoint.fail_step(
                    &spec.step_id,
                    MODEL_REQUEST_CANCELLED,
                    current_time_millis(),
                )?;
                append_workflow_checkpoint_event(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    "Collaboration workflow step paused",
                    "paused",
                    Some(&spec.step_id),
                    &workflow_checkpoint,
                )?;
                continue 'completed_specs;
            }
            let mut effective_completion = completion;
            let mut completed_model = spec.model.clone();
            let mut accumulated_evidence = Vec::new();
            let mut accumulated_latency_ms = 0u64;
            let mut accumulated_tokens = 0u64;
            loop {
                accumulated_latency_ms =
                    accumulated_latency_ms.saturating_add(effective_completion.latency_ms);
                accumulated_tokens = accumulated_tokens.saturating_add(
                    effective_completion
                        .usage
                        .get("total_tokens")
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or_default(),
                );
                accumulated_evidence.extend(effective_completion.evidence.iter().cloned());
                if effective_completion
                    .content
                    .as_ref()
                    .is_some_and(|content| !content.trim().is_empty())
                {
                    break;
                }

                let attempts = workflow_checkpoint
                    .steps
                    .get(&spec.step_id)
                    .map(|step| step.attempts)
                    .unwrap_or_default();
                if attempts >= spec.max_attempts {
                    let error = format!(
                        "step {} exhausted {} attempts; last failure: {}",
                        spec.step_id,
                        spec.max_attempts,
                        effective_completion
                            .error
                            .as_deref()
                            .unwrap_or("worker returned empty content")
                    );
                    workflow_checkpoint.fail_step(&spec.step_id, &error, current_time_millis())?;
                    append_workflow_checkpoint_event(
                        state,
                        task_id,
                        run_context,
                        collaboration_id,
                        "Collaboration workflow step failed",
                        "failed",
                        Some(&spec.step_id),
                        &workflow_checkpoint,
                    )?;
                    layer_failures.push(format!(
                        "step {} failed after recovery: {error}",
                        spec.step_id
                    ));
                    continue 'completed_specs;
                }

                let recovery_attempt = attempts.saturating_add(1);
                let replacement_model = match adaptive_recovery_model(
                    &spec.step_id,
                    &completed_model,
                    recovery_attempt,
                    models,
                    prompt_genome.retry_policy,
                    effective_completion.error.as_deref(),
                ) {
                    Ok(model) => model,
                    Err(error) => {
                        workflow_checkpoint.fail_step(
                            &spec.step_id,
                            &error,
                            current_time_millis(),
                        )?;
                        append_workflow_checkpoint_event(
                            state,
                            task_id,
                            run_context,
                            collaboration_id,
                            "Collaboration workflow step failed",
                            "failed",
                            Some(&spec.step_id),
                            &workflow_checkpoint,
                        )?;
                        layer_failures.push(format!(
                            "step {} failed after recovery: {error}",
                            spec.step_id
                        ));
                        continue 'completed_specs;
                    }
                };
                workflow_checkpoint.begin_step_with_attempt_limit(
                    &spec.step_id,
                    &replacement_model,
                    spec.max_attempts,
                    current_time_millis(),
                )?;
                append_workflow_checkpoint_event(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    "Collaboration workflow recovery started",
                    "running",
                    Some(&spec.step_id),
                    &workflow_checkpoint,
                )?;
                match recover_adaptive_worker(
                    app,
                    state,
                    config,
                    task_id,
                    workspace_root,
                    run_context,
                    collaboration_id,
                    prompt,
                    spec,
                    &effective_completion,
                    &completed_model,
                    &replacement_model,
                    recovery_attempt,
                    prompt_genome.retry_policy,
                    cancellation.clone(),
                ) {
                    Ok(recovered) => {
                        effective_completion = recovered;
                        completed_model = replacement_model;
                    }
                    Err(error) => {
                        if error == COLLABORATION_STEER_INTERRUPTED
                            || collaboration_steer_pending(cancellation.as_ref())
                        {
                            pause_anytime_for_steer(
                                state,
                                task_id,
                                run_context,
                                collaboration_id,
                                &mut workflow_checkpoint,
                                &anytime_controller,
                                anchor_supervisor.as_ref(),
                                direct_anchor_verifier.as_ref(),
                            )?;
                            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
                        }
                        workflow_checkpoint.fail_step(
                            &spec.step_id,
                            &error,
                            current_time_millis(),
                        )?;
                        append_workflow_checkpoint_event(
                            state,
                            task_id,
                            run_context,
                            collaboration_id,
                            "Collaboration workflow step failed",
                            "failed",
                            Some(&spec.step_id),
                            &workflow_checkpoint,
                        )?;
                        layer_failures.push(format!(
                            "step {} failed after recovery: {error}",
                            spec.step_id
                        ));
                        continue 'completed_specs;
                    }
                }
            }
            let content = effective_completion
                .content
                .as_ref()
                .filter(|content| !content.trim().is_empty())
                .cloned()
                .ok_or_else(|| {
                    format!("adaptive step {} completed without content", spec.step_id)
                })?;
            let shared_evidence = merge_collaboration_evidence(
                &spec.access,
                &evidence_by_step,
                &accumulated_evidence,
            );
            let step_output = collaboration_step_result(
                &spec.step_id,
                &completed_model,
                &content,
                &shared_evidence,
            );
            let evidence_json = serde_json::to_string(&shared_evidence)
                .map_err(|error| format!("workflow evidence serialization failed: {error}"))?;
            workflow_checkpoint.complete_step(
                &spec.step_id,
                &completed_model,
                step_output.clone(),
                evidence_json,
                current_time_millis(),
            )?;
            workflow_checkpoint.record_step_metrics(
                &spec.step_id,
                accumulated_latency_ms,
                accumulated_tokens,
            )?;
            workflow_checkpoint
                .anytime_outputs
                .insert(spec.step_id.clone(), step_output.clone());
            outputs.insert(spec.step_id.clone(), step_output);
            evidence_by_step.insert(spec.step_id.clone(), shared_evidence);
            append_workflow_checkpoint_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                "Collaboration workflow step checkpointed",
                "completed",
                Some(&spec.step_id),
                &workflow_checkpoint,
            )?;
        }
        if collaboration_steer_pending(cancellation.as_ref()) {
            pause_anytime_for_steer(
                state,
                task_id,
                run_context,
                collaboration_id,
                &mut workflow_checkpoint,
                &anytime_controller,
                anchor_supervisor.as_ref(),
                direct_anchor_verifier.as_ref(),
            )?;
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        advance_direct_anchor_background(
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            collaboration_id,
            prompt,
            prompt_genome.verification,
            cancellation.as_ref(),
            &anchor_spec,
            &mut anchor_supervisor,
            &mut direct_anchor_output,
            &mut direct_anchor_verifier,
            &mut direct_anchor_verifier_attempted,
            &mut anytime_controller,
            &mut workflow_checkpoint,
        )?;
        if collaboration_steer_pending(cancellation.as_ref()) {
            pause_anytime_for_steer(
                state,
                task_id,
                run_context,
                collaboration_id,
                &mut workflow_checkpoint,
                &anytime_controller,
                anchor_supervisor.as_ref(),
                direct_anchor_verifier.as_ref(),
            )?;
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        if direct_anchor_should_commit(&anytime_controller, cancellation.as_ref()) {
            if let Some(output) = direct_anchor_output.as_ref() {
                cancel_anytime_background(
                    anchor_supervisor.as_ref(),
                    direct_anchor_verifier.as_ref(),
                );
                append_workflow_checkpoint_event(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    "Anytime terminal reserve committed best-known result",
                    "completed",
                    None,
                    &workflow_checkpoint,
                )?;
                return Ok(output.clone());
            }
        }
        let successful_steps = specs
            .iter()
            .filter(|spec| {
                workflow_checkpoint
                    .steps
                    .get(&spec.step_id)
                    .is_some_and(|step| step.status == WorkflowStepStatus::Completed)
            })
            .count();
        if independent_layer && !layer_failures.is_empty() && successful_steps >= required_successes
        {
            for spec in &specs {
                let Some(step) = workflow_checkpoint.steps.get(&spec.step_id) else {
                    continue;
                };
                if step.status != WorkflowStepStatus::Failed {
                    continue;
                }
                let error = step
                    .error
                    .clone()
                    .unwrap_or_else(|| "worker returned no usable result".to_string());
                let degraded = adaptive_degraded_branch_output(&spec.step_id, &error);
                workflow_checkpoint.degrade_step(
                    &spec.step_id,
                    degraded.clone(),
                    error,
                    current_time_millis(),
                )?;
                workflow_checkpoint
                    .anytime_outputs
                    .insert(spec.step_id.clone(), degraded.clone());
                outputs.insert(spec.step_id.clone(), degraded);
                append_workflow_checkpoint_event(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    "Collaboration workflow branch degraded",
                    "degraded",
                    Some(&spec.step_id),
                    &workflow_checkpoint,
                )?;
            }
            if let Ok(mut store) = state.store.lock() {
                let _ = append_event(
                    &mut store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Collaboration quorum preserved",
                    metadata_with_context(
                        [
                            ("collaboration_id".to_string(), collaboration_id.to_string()),
                            (
                                "successful_branches".to_string(),
                                successful_steps.to_string(),
                            ),
                            (
                                "required_branches".to_string(),
                                required_successes.to_string(),
                            ),
                            (
                                "degraded_branches".to_string(),
                                layer_failures.len().to_string(),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        run_context,
                    ),
                );
            }
            layer_failures.clear();
        }
        for spec in &specs {
            let Some(step_checkpoint) = workflow_checkpoint.steps.get(&spec.step_id) else {
                continue;
            };
            if anytime_controller
                .candidate(&spec.step_id)
                .is_none_or(|candidate| candidate.state != AnytimeCandidateState::Running)
            {
                continue;
            }
            match step_checkpoint.status {
                WorkflowStepStatus::Completed if spec.step_id != final_step_id => {
                    anytime_controller.observe(
                        &spec.step_id,
                        AnytimeVerdict {
                            quality_bps: 6_500,
                            confidence_bps: 6_000,
                            constraint_coverage_bps: 6_500,
                            evidence_count: step_checkpoint.evidence_count,
                            safety_violations: 0,
                            deliverable: step_checkpoint
                                .output
                                .as_ref()
                                .is_some_and(|output| !output.trim().is_empty()),
                            verified: false,
                            anchor_uplift_bps: None,
                        },
                    )?;
                }
                WorkflowStepStatus::Degraded if spec.step_id != final_step_id => {
                    anytime_controller.observe(
                        &spec.step_id,
                        AnytimeVerdict {
                            quality_bps: 4_750,
                            confidence_bps: 4_500,
                            constraint_coverage_bps: 5_000,
                            evidence_count: step_checkpoint.evidence_count,
                            safety_violations: 0,
                            deliverable: step_checkpoint
                                .output
                                .as_ref()
                                .is_some_and(|output| !output.trim().is_empty()),
                            verified: false,
                            anchor_uplift_bps: None,
                        },
                    )?;
                }
                WorkflowStepStatus::Failed => anytime_controller.fail(&spec.step_id)?,
                WorkflowStepStatus::Pending
                | WorkflowStepStatus::Running
                | WorkflowStepStatus::Completed
                | WorkflowStepStatus::Degraded => {}
            }
        }
        persist_anytime_controller(&mut workflow_checkpoint, &anytime_controller)?;
        if let Some(error) = adaptive_layer_failure_error(&layer_failures) {
            let partial_handoff = adaptive_partial_work_handoff(prompt, &outputs, &layer_failures);
            if let Some(handoff) = partial_handoff.as_ref() {
                register_partial_handoff_candidate(
                    &mut anytime_controller,
                    &mut workflow_checkpoint,
                    handoff,
                    outputs.len(),
                    layer_failures.len(),
                    evidence_by_step.values().flatten().count(),
                )?;
                append_workflow_checkpoint_event(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    "Collaboration workflow checkpoint preserved",
                    "degraded",
                    None,
                    &workflow_checkpoint,
                )?;
            }
            if direct_anchor_output.is_none() {
                if let Some(completion) = anchor_supervisor
                    .as_mut()
                    .and_then(|supervisor| supervisor.recv_timeout(Duration::from_millis(500)))
                {
                    let completion = parallel_completion_or_failure(completion);
                    let _ = settle_direct_anchor_candidate(
                        state,
                        task_id,
                        run_context,
                        collaboration_id,
                        &anchor_spec,
                        &completion,
                        prompt_genome.verification,
                        cancellation.as_ref(),
                        &mut anytime_controller,
                        &mut workflow_checkpoint,
                    )?;
                }
            }
            if let Some((candidate_id, output, verdict)) =
                anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
            {
                append_workflow_checkpoint_event(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    "Anytime workflow failure committed best-known result",
                    "degraded",
                    None,
                    &workflow_checkpoint,
                )?;
                if let Some(control) = cancellation.as_ref() {
                    control.record_best_known_result(
                        &format!("anytime_failure:{candidate_id}"),
                        &output,
                        if verdict.verified {
                            ResultQuality::Verified
                        } else {
                            ResultQuality::Grounded
                        },
                        verdict.evidence_count,
                        verdict.verified,
                        false,
                    );
                }
                if let Ok(mut store) = state.store.lock() {
                    let _ = append_event(
                        &mut store,
                        task_id,
                        EventKind::TaskStatusChanged,
                        "Collaboration workflow failed",
                        metadata_with_context(
                            [
                                ("collaboration_id".to_string(), collaboration_id.to_string()),
                                (
                                    "workflow_schema".to_string(),
                                    WORKFLOW_IR_SCHEMA.to_string(),
                                ),
                                ("status".to_string(), "degraded".to_string()),
                                ("fallback_used".to_string(), "true".to_string()),
                                ("fallback_candidate".to_string(), candidate_id),
                                ("completed_steps".to_string(), outputs.len().to_string()),
                                ("failed_steps".to_string(), layer_failures.len().to_string()),
                                (
                                    "error".to_string(),
                                    truncate_for_collaboration(&error, 2_000),
                                ),
                                (
                                    "latency_ms".to_string(),
                                    current_time_millis()
                                        .saturating_sub(workflow_started_at_ms)
                                        .to_string(),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                            run_context,
                        ),
                    );
                }
                return Ok(output);
            }
            if let Some(handoff) = partial_handoff {
                return Ok(handoff);
            }
            return Err(error);
        }
        if cancellation.as_ref().is_some_and(agent_run_should_stop) {
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
    }

    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            &mut workflow_checkpoint,
            &anytime_controller,
            anchor_supervisor.as_ref(),
            direct_anchor_verifier.as_ref(),
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }

    if direct_anchor_output.is_none() {
        if let Some(completion) = anchor_supervisor
            .as_mut()
            .and_then(ParallelJobSupervisor::try_recv)
        {
            let completion = parallel_completion_or_failure(completion);
            direct_anchor_output = settle_direct_anchor_candidate(
                state,
                task_id,
                run_context,
                collaboration_id,
                &anchor_spec,
                &completion,
                prompt_genome.verification,
                cancellation.as_ref(),
                &mut anytime_controller,
                &mut workflow_checkpoint,
            )?;
        }
    }
    let final_step = workflow
        .steps
        .last()
        .ok_or_else(|| "adaptive workflow has no final step".to_string())?;
    let final_output = if let Some(output) = outputs.remove(&final_step.id) {
        output
    } else {
        if direct_anchor_output.is_none() {
            if let Some(completion) = anchor_supervisor
                .as_mut()
                .and_then(|supervisor| supervisor.recv_timeout(Duration::from_millis(500)))
            {
                let completion = parallel_completion_or_failure(completion);
                let _ = settle_direct_anchor_candidate(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    &anchor_spec,
                    &completion,
                    prompt_genome.verification,
                    cancellation.as_ref(),
                    &mut anytime_controller,
                    &mut workflow_checkpoint,
                )?;
            }
        }
        if let Some((_candidate_id, output, _verdict)) =
            anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
        {
            return Ok(output);
        }
        return Err("adaptive workflow final output is missing".to_string());
    };
    let evidence_count = evidence_by_step
        .values()
        .flatten()
        .map(|evidence| evidence.tool_call_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let quality_gate = quality_gate_adaptive_output(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        &final_output,
        prompt_genome.verification,
        evidence_count,
    );
    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            &mut workflow_checkpoint,
            &anytime_controller,
            anchor_supervisor.as_ref(),
            direct_anchor_verifier.as_ref(),
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let final_output = match adaptive_quality_handoff(&quality_gate) {
        Ok(output) => output,
        Err(error) => {
            controller_mark_running_if_pending(&mut anytime_controller, &final_step.id)?;
            if anytime_controller
                .candidate(&final_step.id)
                .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Running)
            {
                anytime_controller.observe(
                    &final_step.id,
                    AnytimeVerdict {
                        quality_bps: 0,
                        confidence_bps: 0,
                        constraint_coverage_bps: 0,
                        evidence_count,
                        safety_violations: quality_gate.safety_violations,
                        deliverable: false,
                        verified: false,
                        anchor_uplift_bps: None,
                    },
                )?;
            }
            persist_anytime_controller(&mut workflow_checkpoint, &anytime_controller)?;
            append_workflow_checkpoint_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                "Collaboration workflow blocked by quality gate",
                "paused",
                Some(&final_step.id),
                &workflow_checkpoint,
            )?;
            if let Some((candidate_id, output, verdict)) =
                anytime_best_known_output(&anytime_controller, &workflow_checkpoint)
            {
                if let Some(control) = cancellation.as_ref() {
                    control.record_best_known_result(
                        &format!("anytime_quality_rejection:{candidate_id}"),
                        &output,
                        if verdict.verified {
                            ResultQuality::Verified
                        } else {
                            ResultQuality::Grounded
                        },
                        verdict.evidence_count,
                        verdict.verified,
                        false,
                    );
                }
                return Ok(output);
            }
            return Err(error);
        }
    };
    if direct_anchor_output.is_none() {
        if let Some(completion) = anchor_supervisor
            .as_mut()
            .and_then(|supervisor| supervisor.recv_timeout(Duration::from_millis(750)))
        {
            let completion = parallel_completion_or_failure(completion);
            direct_anchor_output = settle_direct_anchor_candidate(
                state,
                task_id,
                run_context,
                collaboration_id,
                &anchor_spec,
                &completion,
                prompt_genome.verification,
                cancellation.as_ref(),
                &mut anytime_controller,
                &mut workflow_checkpoint,
            )?;
        }
    }
    finalize_adaptive_collaboration(AdaptiveCollaborationFinalization {
        app,
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        models,
        agent_budget,
        effort,
        policy,
        prompt_genome,
        workflow_started_at_ms,
        final_step_id: final_step.id.clone(),
        workflow_steps: workflow_plan.steps.len(),
        layer_count,
        role_coverage,
        evidence_count,
        quality_gate,
        final_output,
        direct_anchor_output: direct_anchor_output.as_deref(),
        cancellation: cancellation.as_ref(),
        anchor_supervisor: anchor_supervisor.as_ref(),
        direct_anchor_verifier: direct_anchor_verifier.as_ref(),
        anytime_controller: &mut anytime_controller,
        workflow_checkpoint: &mut workflow_checkpoint,
    })
}
