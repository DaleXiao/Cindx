use super::queue::*;
use crate::*;

#[tauri::command]
pub(crate) async fn run_agent_task(
    app: tauri::AppHandle,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        run_agent_task_blocking(&app, state, input)
    })
    .await
    .map_err(|error| format!("agent task failed to join: {error}"))?
}

pub(crate) fn run_agent_task_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    let session_id = input.session_id.clone();
    let effort = AgentEffort::parse(&input.effort);
    let run_control_lease =
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), None)?;
    let cancellation = run_control_lease.control();
    let result = run_agent_task_blocking_inner(app, state.clone(), input, &cancellation);
    if let Ok(agent) = result.as_ref() {
        if agent.status == "completed" {
            if let Ok(Some(refinement)) =
                persist_completed_conversation_title(&state, &session_id, &agent.messages)
            {
                spawn_semantic_session_title_refinement(app.clone(), refinement);
            }
        }
    }
    result
}

pub(crate) fn run_agent_task_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: AgentTaskInput,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let effort = AgentEffort::parse(&input.effort);
    let user_prompt = input.prompt.trim().to_string();
    let queue_id = input.queue_id.clone();
    let session_id = input.session_id;
    clear_suspended_agent_run(&state, &session_id)?;
    let mut run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let mut config = clone_provider_config(&state)?;
    config.agent_system_prompt = personalized_agent_instructions(
        &load_personalization_config(),
        &config.agent_system_prompt,
    );
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }

    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let attachments = validate_agent_attachments(&root, input.attachments)?;
    if user_prompt.is_empty() && attachments.is_empty() {
        return agent_state_with_error_in_context(&state, &run_context, "agent prompt is empty");
    }
    let display_prompt = if user_prompt.is_empty() {
        format!(
            "Review attached {}",
            attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        user_prompt
    };
    let prompt = prompt_with_attachments(&display_prompt, &attachments);
    run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    run_context.insert("agent_run_id".to_string(), unique_id("agent-run"));
    if let Some(queue_id) = queue_id {
        run_context.insert("queue_id".to_string(), queue_id);
    }
    run_context.insert(
        "current_time".to_string(),
        normalized_current_time_context(&input.current_time),
    );
    let requested_policy = effort.requested_policy();
    let mut routing_context =
        RoutingContext::from_prompt(&prompt, model_candidates_for_config(&config));
    if requested_policy != OrchestrationPolicy::AutoRouter {
        routing_context.user_policy_override = Some(requested_policy.clone());
    }
    let (routing_decision, router_examples) = route_with_local_telemetry(&state, &routing_context)?;
    let collaboration_policy = if requested_policy == OrchestrationPolicy::AutoRouter {
        routing_decision.policy.clone()
    } else {
        requested_policy.clone()
    };
    let conductor_contract = ConductorExecutionContract::from_routing(
        &routing_context,
        effort.label(),
        collaboration_policy.clone(),
    );
    run_context.insert(
        "task_class".to_string(),
        routing_context.task_class.label().to_string(),
    );
    run_context.insert(
        "routing_signature".to_string(),
        routing_context.learning_signature(),
    );
    run_context.insert("agent_effort".to_string(), effort.label().to_string());
    add_image_generation_run_context(&mut run_context, &config, &prompt);
    add_agent_run_budget_metadata(&mut run_context, cancellation);
    run_context.insert(
        "requested_policy".to_string(),
        requested_policy.label().to_string(),
    );
    run_context.insert(
        "collaboration_policy".to_string(),
        collaboration_policy.label().to_string(),
    );
    run_context.insert(
        "collaboration_profile".to_string(),
        collaboration_profile(effort, &routing_context).to_string(),
    );
    run_context.insert(
        "conductor_contract".to_string(),
        conductor_contract.to_json()?,
    );
    run_context.insert(
        "expected_collaboration_uplift_bps".to_string(),
        conductor_contract.expected_uplift_bps.to_string(),
    );
    run_context.insert(
        "agent_model".to_string(),
        agent_model_for_effort(&config, effort, &collaboration_policy, &routing_decision),
    );
    run_context.insert("router_model".to_string(), routing_decision.model.clone());
    run_context.insert("router_examples".to_string(), router_examples.to_string());
    run_context.insert(
        "router_source".to_string(),
        routing_decision
            .metadata
            .get("router")
            .cloned()
            .unwrap_or_else(|| "rule_based_v2".to_string()),
    );
    let session_id = run_context.get("session_id").map(String::as_str);
    let task_id = phase16_task_id();
    let (mut history, artifact_manifest) = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = agent_events_for_session(&store, &task_id, session_id)
            .map_err(|error| error.to_string())?;
        let session_events = session_id
            .map(|session_id| agent_session_events(&events, session_id))
            .unwrap_or_default();
        let history = session_events
            .iter()
            .filter_map(message_from_event)
            .collect::<Vec<_>>();
        let artifact_manifest = artifact_manifest_message(&session_events);
        let mut start_metadata = run_context.clone();
        start_metadata.insert("prompt".to_string(), display_prompt.clone());
        start_metadata.insert(
            "requested_policy".to_string(),
            requested_policy.label().to_string(),
        );
        start_metadata.insert(
            "collaboration_policy".to_string(),
            collaboration_policy.label().to_string(),
        );
        start_metadata.insert(
            "router_explanation".to_string(),
            routing_decision.explanation.clone(),
        );
        start_metadata.insert(
            "context_window_tokens".to_string(),
            config.context_window_tokens.to_string(),
        );
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task started",
            start_metadata,
        )
        .map_err(|error| error.to_string())?;
        append_router_decision_event(
            &mut store,
            &task_id,
            &run_context,
            &routing_context,
            &routing_decision,
            router_examples,
        )
        .map_err(|error| error.to_string())?;
        let mut message_metadata = run_context.clone();
        add_attachment_metadata(&mut message_metadata, &attachments);
        append_message_event_with_metadata(
            &mut store,
            &task_id,
            MessageRole::User,
            &display_prompt,
            message_metadata,
        )
        .map_err(|error| error.to_string())?;
        (history, artifact_manifest)
    };

    history = prepare_session_history_context(
        &state,
        &root,
        &run_context,
        history,
        config.context_window_tokens,
    )
    .map_err(|error| format!("context preparation failed: {error}"))?;
    let prepared_knowledge = match prepare_run_knowledge_contexts(
        &state,
        &task_id,
        &run_context,
        &root,
        &config,
        &prompt,
        &routing_context,
        &routing_decision.retrieval_mode,
        cancellation,
    ) {
        Ok(prepared) => prepared,
        Err(error) if error == MODEL_REQUEST_CANCELLED => {
            return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
        }
        Err(error) => return Err(error),
    };
    append_prepared_memory_context(&mut run_context, &mut history, prepared_knowledge.memory);
    if let Some(artifact_manifest) = artifact_manifest {
        history.push(artifact_manifest);
    }

    append_skill_context_for_run(&root, &prompt, &mut history)?;
    if let Some(workspace_context) = prepared_knowledge.workspace {
        history.push(workspace_context);
    }

    if agent_run_should_stop(cancellation) {
        return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
    }

    cancellation.mark_progress("orchestration", "Preparing execution strategy");
    append_agent_progress_event(
        &state,
        &task_id,
        &run_context,
        "Preparing execution strategy",
    )?;
    append_single_model_policy_guidance(&mut history, &collaboration_policy);
    let collaboration = match prepare_agent_collaboration_or_degrade(
        app,
        &state,
        &config,
        &task_id,
        &root,
        &run_context,
        &collaboration_policy,
        &prompt,
        &history,
    ) {
        Ok(collaboration) => collaboration,
        Err(_) if agent_run_should_stop(cancellation) => {
            return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
        }
        Err(error) => {
            return agent_state_with_error_in_context(
                &state,
                &run_context,
                format!("Collaboration failed: {error}"),
            );
        }
    };
    if let Some(collaboration) = collaboration.as_ref() {
        append_agent_collaboration_context(&mut history, collaboration);
    }

    if agent_run_should_stop(cancellation) {
        return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
    }

    cancellation.mark_progress("executor", "Starting execution");
    append_agent_progress_event(&state, &task_id, &run_context, "Starting execution")?;
    let runtime_config = cancellation.runtime_config();
    let mut runtime = if history.is_empty() {
        start_agent_loop(task_id, prompt.clone(), runtime_config.clone())
    } else {
        start_agent_loop_with_history(task_id, prompt.clone(), history, runtime_config)
    };
    if let Some(message) = runtime
        .messages
        .iter_mut()
        .rev()
        .find(|message| matches!(message.role, MessageRole::User))
    {
        add_attachment_metadata(&mut message.metadata, &attachments);
    }
    continue_agent_loop(
        app,
        &state,
        &config,
        &root,
        runtime,
        prompt,
        run_context,
        collaboration.as_ref(),
        cancellation,
    )
}

#[tauri::command]
pub(crate) fn cancel_agent_task(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    let state = app.state::<AppState>();
    let active_run_cancelled = request_agent_run_cancel(&state, &input.session_id)?;
    clear_suspended_agent_run(&state, &input.session_id)?;
    let run_context = project_session_metadata_for_session(&state, Some(&input.session_id))?;
    let session_id = run_context.get("session_id").map(String::as_str);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let current =
        agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?;
    if !current.can_cancel && !active_run_cancelled {
        return Ok(current);
    }

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        metadata_with_context(
            [("reason".to_string(), "user_cancelled".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    if let Err(error) = refresh_project_memory_after_run(&mut store, &run_context) {
        eprintln!("project memory checkpoint unavailable: {error}");
    }

    emit_agent_stream_delta(
        &app,
        "agent-cancelled",
        Some(&input.session_id),
        "",
        true,
        true,
        None,
    );

    agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn retry_agent_task(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        retry_agent_task_blocking(&app, state, input)
    })
    .await
    .map_err(|error| format!("agent retry failed to join: {error}"))?
}

pub(crate) fn retry_agent_task_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    let session_id = input.session_id.clone();
    let effort = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", &session_id)
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, Some(&session_id));
        agent_effort_from_active_events(&active_events)
    };
    let suspended_snapshot = suspended_agent_run_control_snapshot(&state, &session_id)?;
    let run_control_lease = if let Some(snapshot) = suspended_snapshot {
        begin_agent_run_control_for_continuation(&state, &session_id, snapshot)?
    } else {
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), None)?
    };
    let cancellation = run_control_lease.control();
    let suspended = take_suspended_agent_run(&state, &session_id)?;
    if let Some(suspended) = suspended {
        resume_suspended_agent_run(app, &state, suspended, &cancellation)
    } else {
        retry_agent_task_blocking_inner(app, state.clone(), input, &cancellation)
    }
}

pub(crate) fn resume_suspended_agent_run(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    suspended: SuspendedAgentRun,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let SuspendedAgentRun {
        mut runtime,
        prompt,
        mut run_context,
        workspace_root,
        collaboration,
        run_control: _,
    } = suspended;
    let config = clone_provider_config(state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let recovery = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        claim_agent_recovery_envelope(&mut store, &run_context, &["paused"], "user_continued")?
    };
    if let Some(recovery) = recovery {
        run_context.insert(
            "recovery_resume_key".to_string(),
            recovery.resume_key.clone(),
        );
        run_context.insert(
            "source_agent_run_id".to_string(),
            recovery.source_run_id.clone(),
        );
        run_context.insert(
            "recovery_attempts".to_string(),
            recovery.attempts.to_string(),
        );
        run_context.insert("continuation".to_string(), "true".to_string());
        if let Some(queue_id) = recovery.queue_id {
            run_context.insert("queue_id".to_string(), queue_id);
        }
    }
    run_context.insert("agent_run_id".to_string(), unique_id("agent-run"));
    run_context.insert(
        "current_time".to_string(),
        normalized_current_time_context(""),
    );
    add_agent_run_budget_metadata(&mut run_context, cancellation);
    cancellation.extend_runtime_budget(&mut runtime);
    cancellation.mark_progress("continuation", "Resuming saved execution state");
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            metadata_with_context(
                [
                    ("prompt".to_string(), prompt.clone()),
                    ("continuation".to_string(), "true".to_string()),
                    (
                        "context_window_tokens".to_string(),
                        config.context_window_tokens.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                &run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    continue_agent_loop(
        app,
        state,
        &config,
        &workspace_root,
        runtime,
        prompt,
        run_context,
        collaboration.as_ref(),
        cancellation,
    )
}

pub(crate) fn retry_agent_task_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    clear_suspended_agent_run(&state, &input.session_id)?;
    let mut run_context = project_session_metadata_for_session(&state, Some(&input.session_id))?;
    run_context.insert(
        "current_time".to_string(),
        normalized_current_time_context(""),
    );
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let session_id = run_context.get("session_id").cloned();
    let task_id = phase16_task_id();
    let (prompt, effort, recovery) = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task(&task_id)
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, session_id.as_deref());
        let prompt = latest_agent_prompt_from_active_events(&active_events)
            .ok_or_else(|| "No previous agent prompt to retry".to_string())?;
        let effort = agent_effort_from_active_events(&active_events);
        let recovery =
            claim_agent_recovery_envelope(&mut store, &run_context, &["paused"], "user_continued")?;
        (prompt, effort, recovery)
    };
    if let Some(recovery) = recovery.as_ref() {
        run_context.insert(
            "recovery_resume_key".to_string(),
            recovery.resume_key.clone(),
        );
        run_context.insert(
            "recovery_attempts".to_string(),
            recovery.attempts.to_string(),
        );
        run_context.insert(
            "source_agent_run_id".to_string(),
            recovery.source_run_id.clone(),
        );
        run_context.insert("continuation".to_string(), "true".to_string());
        if let Some(queue_id) = recovery.queue_id.as_ref() {
            run_context.insert("queue_id".to_string(), queue_id.clone());
        }
    }
    run_context.insert("agent_run_id".to_string(), unique_id("agent-run"));
    let requested_policy = effort.requested_policy();
    let mut routing_context =
        RoutingContext::from_prompt(&prompt, model_candidates_for_config(&config));
    if requested_policy != OrchestrationPolicy::AutoRouter {
        routing_context.user_policy_override = Some(requested_policy.clone());
    }
    let (routing_decision, router_examples) = route_with_local_telemetry(&state, &routing_context)?;
    let collaboration_policy = if requested_policy == OrchestrationPolicy::AutoRouter {
        routing_decision.policy.clone()
    } else {
        requested_policy.clone()
    };
    let conductor_contract = ConductorExecutionContract::from_routing(
        &routing_context,
        effort.label(),
        collaboration_policy.clone(),
    );
    run_context.insert(
        "task_class".to_string(),
        routing_context.task_class.label().to_string(),
    );
    run_context.insert(
        "routing_signature".to_string(),
        routing_context.learning_signature(),
    );
    run_context.insert("agent_effort".to_string(), effort.label().to_string());
    add_image_generation_run_context(&mut run_context, &config, &prompt);
    add_agent_run_budget_metadata(&mut run_context, cancellation);
    run_context.insert(
        "requested_policy".to_string(),
        requested_policy.label().to_string(),
    );
    run_context.insert(
        "collaboration_policy".to_string(),
        collaboration_policy.label().to_string(),
    );
    run_context.insert(
        "collaboration_profile".to_string(),
        collaboration_profile(effort, &routing_context).to_string(),
    );
    run_context.insert(
        "conductor_contract".to_string(),
        conductor_contract.to_json()?,
    );
    run_context.insert(
        "expected_collaboration_uplift_bps".to_string(),
        conductor_contract.expected_uplift_bps.to_string(),
    );
    run_context.insert(
        "agent_model".to_string(),
        agent_model_for_effort(&config, effort, &collaboration_policy, &routing_decision),
    );
    run_context.insert("router_model".to_string(), routing_decision.model.clone());
    run_context.insert("router_examples".to_string(), router_examples.to_string());
    run_context.insert(
        "router_source".to_string(),
        routing_decision
            .metadata
            .get("router")
            .cloned()
            .unwrap_or_else(|| "rule_based_v2".to_string()),
    );
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let mut start_metadata = run_context.clone();
        start_metadata.insert("prompt".to_string(), prompt.clone());
        start_metadata.insert(
            "context_window_tokens".to_string(),
            config.context_window_tokens.to_string(),
        );
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            start_metadata,
        )
        .map_err(|error| error.to_string())?;
        append_router_decision_event(
            &mut store,
            &task_id,
            &run_context,
            &routing_context,
            &routing_decision,
            router_examples,
        )
        .map_err(|error| error.to_string())?;
        let mut continuation_metadata = run_context.clone();
        continuation_metadata.insert("continuation_replay".to_string(), "true".to_string());
        append_message_event_with_metadata(
            &mut store,
            &task_id,
            MessageRole::User,
            &prompt,
            continuation_metadata,
        )
        .map_err(|error| error.to_string())?;
    }

    let (mut history, artifact_manifest, restored_task_state) = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task(&task_id)
            .map_err(|error| error.to_string())?;
        let session_events = session_id
            .as_deref()
            .map(|session_id| agent_session_events(&events, session_id))
            .unwrap_or_default();
        let mut messages = recovery_safe_transcript(&session_events);
        if messages
            .last()
            .map(|message| matches!(message.role, MessageRole::User) && message.content == prompt)
            .unwrap_or(false)
        {
            messages.pop();
        }
        let restored_task_state = recovery
            .as_ref()
            .and_then(|recovery| recovery.task_state.as_ref())
            .and_then(
                |snapshot| match snapshot.restore(prompt.clone(), messages.clone()) {
                    Ok(runtime) => Some(runtime),
                    Err(error) => {
                        let _ = append_event(
                            &mut store,
                            &task_id,
                            EventKind::TaskStatusChanged,
                            "Agent task checkpoint fallback",
                            metadata_with_context(
                                [
                                    (
                                        "recovery_code".to_string(),
                                        "task_state_lineage_mismatch".to_string(),
                                    ),
                                    ("recovery_detail".to_string(), error.to_string()),
                                ]
                                .into_iter()
                                .collect(),
                                &run_context,
                            ),
                        );
                        None
                    }
                },
            );
        (
            messages,
            artifact_manifest_message(&session_events),
            restored_task_state,
        )
    };
    history = prepare_session_history_context(
        &state,
        &root,
        &run_context,
        history,
        config.context_window_tokens,
    )?;
    let prepared_knowledge = match prepare_run_knowledge_contexts(
        &state,
        &task_id,
        &run_context,
        &root,
        &config,
        &prompt,
        &routing_context,
        &routing_decision.retrieval_mode,
        cancellation,
    ) {
        Ok(prepared) => prepared,
        Err(error) if error == MODEL_REQUEST_CANCELLED => {
            return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
        }
        Err(error) => return Err(error),
    };
    append_prepared_memory_context(&mut run_context, &mut history, prepared_knowledge.memory);
    if let Some(artifact_manifest) = artifact_manifest {
        history.push(artifact_manifest);
    }
    append_skill_context_for_run(&root, &prompt, &mut history)?;
    if let Some(workspace_context) = prepared_knowledge.workspace {
        history.push(workspace_context);
    }
    cancellation.mark_progress("orchestration", "Preparing execution strategy");
    append_agent_progress_event(
        &state,
        &task_id,
        &run_context,
        "Preparing execution strategy",
    )?;
    append_single_model_policy_guidance(&mut history, &collaboration_policy);
    let collaboration = match prepare_agent_collaboration_or_degrade(
        app,
        &state,
        &config,
        &task_id,
        &root,
        &run_context,
        &collaboration_policy,
        &prompt,
        &history,
    ) {
        Ok(collaboration) => collaboration,
        Err(_) if agent_run_should_stop(cancellation) => {
            return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
        }
        Err(error) => {
            return agent_state_with_error_in_context(
                &state,
                &run_context,
                format!("Collaboration failed: {error}"),
            );
        }
    };
    if let Some(collaboration) = collaboration.as_ref() {
        append_agent_collaboration_context(&mut history, collaboration);
    }
    if agent_run_should_stop(cancellation) {
        return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
    }
    cancellation.mark_progress("executor", "Starting execution");
    append_agent_progress_event(&state, &task_id, &run_context, "Starting execution")?;
    let runtime_config = cancellation.runtime_config();
    let runtime = if let Some(mut runtime) = restored_task_state {
        runtime.messages = history;
        runtime.messages.push(Message {
            role: MessageRole::User,
            content: prompt.clone(),
            metadata: Metadata::new(),
        });
        cancellation.extend_runtime_budget(&mut runtime);
        runtime
    } else if history.is_empty() {
        start_agent_loop(task_id, prompt.clone(), runtime_config.clone())
    } else {
        start_agent_loop_with_history(task_id, prompt.clone(), history, runtime_config)
    };
    continue_agent_loop(
        app,
        &state,
        &config,
        &root,
        runtime,
        prompt,
        run_context,
        collaboration.as_ref(),
        cancellation,
    )
}
