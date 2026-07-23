use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_collaboration_candidates(
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
    allow_tools: bool,
    prompt_profile: Option<&ConductorPromptGenome>,
) -> Result<String, String> {
    let recent_context = collaboration_recent_context(history);
    let conductor_directive = prompt_profile.map(ConductorPromptGenome::conductor_directive);
    let max_model_turns = prompt_profile
        .map(ConductorPromptGenome::effective_max_model_turns_per_step)
        .unwrap_or(DEFAULT_COLLABORATION_WORKER_TURNS);
    let max_tool_calls = prompt_profile
        .map(ConductorPromptGenome::effective_max_tool_calls_per_step)
        .unwrap_or(MAX_COLLABORATION_WORKER_TOOL_CALLS);
    let specs = models
        .iter()
        .enumerate()
        .map(|(index, model)| CollaborationCandidateSpec {
            stage: format!("candidate_{}", index + 1),
            model: model.clone(),
            prompt: build_collaboration_candidate_prompt(
                prompt,
                &recent_context,
                index,
                conductor_directive.as_deref(),
            ),
            request_id: unique_id("collaboration-model"),
        })
        .collect::<Vec<_>>();
    for spec in &specs {
        record_collaboration_stage_started(
            state,
            task_id,
            run_context,
            collaboration_id,
            &spec.stage,
            &ModelRole::Planner,
            &spec.model,
            &spec.request_id,
            &Metadata::new(),
        )?;
    }

    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
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
            let prompt = spec.prompt.clone();
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
                    ModelRole::Planner,
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
    let effort = run_context
        .get("agent_effort")
        .map(String::as_str)
        .unwrap_or("auto");
    let required_successes = collaboration_candidate_quorum(specs.len(), effort);
    let execution = run_model_jobs_until_quorum(
        "candidate-worker",
        jobs,
        required_successes,
        collaboration_candidate_quorum_grace(effort),
        |completion| {
            completion
                .content
                .as_ref()
                .is_some_and(|content| !content.trim().is_empty())
        },
    );
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration candidate quorum resolved",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "quorum_required".to_string(),
                        required_successes.to_string(),
                    ),
                    (
                        "quorum_successful".to_string(),
                        execution.successful.to_string(),
                    ),
                    (
                        "quorum_reached".to_string(),
                        execution.quorum_reached.to_string(),
                    ),
                    (
                        "cancelled_stragglers".to_string(),
                        execution.cancelled_stragglers.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        );
    }
    let completions = execution
        .results
        .into_iter()
        .map(|result| {
            result.unwrap_or_else(|error| {
                CollaborationCompletion::failed(format!(
                    "collaboration candidate worker failed: {error}"
                ))
            })
        })
        .collect::<Vec<_>>();
    if cancellation.as_ref().is_some_and(agent_run_should_stop) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }

    let mut candidates = Vec::new();
    for (spec, completion) in specs.iter().zip(&completions) {
        record_collaboration_stage_finished(
            state,
            task_id,
            run_context,
            collaboration_id,
            &spec.stage,
            &ModelRole::Planner,
            &spec.model,
            &spec.request_id,
            completion,
            &Metadata::new(),
        )?;
        if let Some(content) = completion
            .content
            .as_ref()
            .filter(|content| !content.trim().is_empty())
        {
            candidates.push((
                spec.model.clone(),
                collaboration_step_result(&spec.stage, &spec.model, content, &completion.evidence),
            ));
        }
    }

    if candidates.is_empty() {
        return Err("all collaboration candidates were unavailable".to_string());
    }
    if candidates.len() == 1 {
        return Ok(candidates.remove(0).1);
    }
    run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        "arbiter",
        ModelRole::Reviewer,
        &config.model_for_role(&ModelRole::Reviewer),
        build_collaboration_arbiter_prompt(prompt, &candidates, conductor_directive.as_deref()),
    )
}

pub(crate) fn collaboration_candidate_quorum(candidate_count: usize, effort: &str) -> usize {
    match candidate_count {
        0 => 0,
        1 => 1,
        _ if effort == "fast" => 1,
        count if effort == "pro" => count,
        count => count.saturating_sub(1).max(2).min(count),
    }
}

pub(crate) fn collaboration_candidate_quorum_grace(effort: &str) -> Duration {
    match effort {
        "fast" => Duration::from_millis(100),
        "pro" => Duration::ZERO,
        _ => Duration::from_millis(400),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_collaboration(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    policy: &OrchestrationPolicy,
    prompt: &str,
    history: &[Message],
) -> Result<Option<AgentCollaboration>, String> {
    let OrchestrationPolicy::BestOfN { candidates } = policy else {
        return Ok(None);
    };
    let id = unique_id("collab");
    let agent_budget = collaboration_agent_budget(*candidates);
    let models = collaboration_candidate_models(config, agent_budget);
    let fallback_models = collaboration_fallback_models(&models, agent_budget);
    let bounded = run_context.get("collaboration_profile").map(String::as_str) == Some("bounded");
    let effort = run_context
        .get("agent_effort")
        .cloned()
        .unwrap_or_else(|| "auto".to_string());
    let policy_label = run_context
        .get("collaboration_policy")
        .cloned()
        .unwrap_or_else(|| policy.label().to_string());
    let bounded_evolution = if bounded && config.prompt_evolution_enabled && effort != "fast" {
        prompt_evolution_evaluation_for_run(state, &effort, run_context).ok()
    } else {
        None
    };
    let bounded_profile = bounded_evolution
        .as_ref()
        .map(|evaluation| evaluation.next_profile.clone());
    if let (Some(evaluation), Some(profile)) =
        (bounded_evolution.as_ref(), bounded_profile.as_ref())
    {
        let prompt_genome = serde_json::to_string(profile)
            .map_err(|error| format!("failed to serialize prompt genome: {error}"))?;
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
                    ("collaboration_id".to_string(), id.clone()),
                    ("prompt_profile".to_string(), profile.id.clone()),
                    ("prompt_effort".to_string(), effort.clone()),
                    (
                        "prompt_generation".to_string(),
                        profile.generation.to_string(),
                    ),
                    ("prompt_genome".to_string(), prompt_genome),
                    (
                        "prompt_selection_mode".to_string(),
                        evaluation.next_mode.clone(),
                    ),
                    (
                        "prompt_evolution_status".to_string(),
                        evaluation.status.clone(),
                    ),
                    (
                        "prompt_champion".to_string(),
                        evaluation.champion_id.clone().unwrap_or_default(),
                    ),
                    ("prompt_evolution_enabled".to_string(), "true".to_string()),
                    ("collaboration_profile".to_string(), "bounded".to_string()),
                    (
                        "prompt_objective".to_string(),
                        truncate_for_collaboration(prompt, 6_000),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    let guidance_result = if bounded {
        run_collaboration_candidates(
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            &id,
            prompt,
            history,
            &fallback_models,
            false,
            bounded_profile.as_ref(),
        )
    } else {
        run_adaptive_collaboration(
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            &id,
            prompt,
            history,
            &models,
            agent_budget,
        )
        .or_else(|error| {
            if error.starts_with(WORKFLOW_RESUMABLE_ERROR_PREFIX) {
                return Err(error);
            }
            let control =
                active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
            if control
                .as_ref()
                .is_some_and(|control| control.should_stop())
            {
                return Err(error);
            }
            let fallback_allowed = control
                .as_ref()
                .is_none_or(|control| control.progress().remaining.as_secs() >= 90);
            if let Ok(mut store) = state.store.lock() {
                let _ = append_event(
                    &mut store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Collaboration workflow failed",
                    metadata_with_context(
                        [
                            ("collaboration_id".to_string(), id.clone()),
                            (
                                "workflow_schema".to_string(),
                                WORKFLOW_IR_SCHEMA.to_string(),
                            ),
                            ("status".to_string(), "failed".to_string()),
                            ("fallback_used".to_string(), fallback_allowed.to_string()),
                            (
                                "error".to_string(),
                                truncate_for_collaboration(&error, 2_000),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        run_context,
                    ),
                );
            }
            if !fallback_allowed {
                return Err(format!(
                    "{error}; adaptive fallback skipped because less than 90 seconds remain"
                ));
            }
            run_collaboration_candidates(
                app,
                state,
                config,
                task_id,
                workspace_root,
                run_context,
                &id,
                prompt,
                history,
                &fallback_models,
                true,
                None,
            )
        })
    };
    let guidance = guidance_result?;
    if bounded {
        if let Some(profile) = bounded_profile {
            schedule_prompt_pairwise_evaluation(
                app.clone(),
                config.clone(),
                task_id.clone(),
                run_context.clone(),
                effort,
                policy_label,
                models.clone(),
                agent_budget,
                profile,
            );
        }
    }
    Ok(Some(AgentCollaboration {
        id,
        policy: policy.label().to_string(),
        guidance,
        candidate_models: models,
    }))
}
