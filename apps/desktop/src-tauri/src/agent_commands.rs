use super::*;

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

#[tauri::command]
pub(crate) async fn queue_agent_message(
    app: tauri::AppHandle,
    input: QueueAgentMessageInput,
) -> Result<QueuedAgentMessageReceipt, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        enqueue_agent_message_inner(&state, input).map(|(receipt, _)| receipt)
    })
    .await
    .map_err(|error| format!("queued message failed to join: {error}"))?
}

pub(crate) fn enqueue_agent_message_inner(
    state: &tauri::State<'_, AppState>,
    input: QueueAgentMessageInput,
) -> Result<(QueuedAgentMessageReceipt, String), String> {
    let run_context = project_session_metadata_for_session(state, Some(&input.session_id))?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(state)?);
    let attachments = validate_agent_attachments(&root, input.attachments)?;
    let prompt = input.prompt.trim().to_string();
    if prompt.is_empty() && attachments.is_empty() {
        let _ = agent_state_with_error_in_context(state, &run_context, "agent prompt is empty")?;
        return Err("agent prompt is empty".to_string());
    }
    let display_prompt = if prompt.is_empty() {
        format!(
            "Review attached {}",
            attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        prompt
    };
    let payload = QueuedAgentMessagePayload {
        prompt: display_prompt,
        attachments,
        effort: AgentEffort::parse(&input.effort).label().to_string(),
        current_time: normalized_current_time_context(&input.current_time),
    };
    let queue_id = queued_agent_message_id(input.queue_id.as_deref());
    let created_at_ms = current_time_millis();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_agent_queue_event(
        &mut store,
        &run_context,
        "enqueue",
        &queue_id,
        "queue",
        created_at_ms,
        Some(&payload),
    )?;
    let revision = store
        .event_revision_by_metadata(&phase16_task_id(), "session_id", &input.session_id)
        .map_err(|error| error.to_string())?;
    let message = QueuedAgentMessageView {
        id: queue_id.clone(),
        session_id: input.session_id,
        prompt: payload.prompt,
        attachments: payload.attachments,
        effort: payload.effort,
        mode: "queue".to_string(),
        created_at_ms,
        updated_at_ms: revision.latest_timestamp_ms,
    };
    Ok((
        QueuedAgentMessageReceipt {
            message,
            event_count: revision.event_count,
            latest_sequence: revision.latest_sequence,
            latest_timestamp_ms: revision.latest_timestamp_ms,
        },
        queue_id,
    ))
}

pub(crate) fn queued_agent_message_id(candidate: Option<&str>) -> String {
    queue_service::queued_agent_message_id(candidate, || unique_id("agent-queue"))
}

pub(crate) fn queued_agent_message_from_read_model(
    store: &mut SqliteStore,
    session_id: &str,
    queue_id: &str,
) -> Result<(Option<QueuedAgentMessageView>, bool), String> {
    let model =
        load_agent_session_read_model(store, session_id).map_err(|error| error.to_string())?;
    let can_cancel = model.state.can_cancel;
    let message = model
        .state
        .queued_messages
        .into_iter()
        .find(|message| message.id == queue_id);
    Ok((message, can_cancel))
}

pub(crate) fn next_queued_agent_message_from_read_model(
    store: &mut SqliteStore,
    session_id: &str,
) -> Result<Option<PendingQueuedAgentMessage>, String> {
    let model =
        load_agent_session_read_model(store, session_id).map_err(|error| error.to_string())?;
    let Some(view) = model.state.queued_messages.first().cloned() else {
        return Ok(None);
    };
    let payload = model
        .queued_payloads
        .get(&view.id)
        .cloned()
        .ok_or_else(|| format!("queued message payload is unavailable for `{}`", view.id))?;
    Ok(Some(PendingQueuedAgentMessage {
        view,
        payload,
        priority_sequence: 0,
    }))
}

pub(crate) fn queued_agent_message_action_receipt(
    store: &SqliteStore,
    session_id: &str,
    queue_id: &str,
    mut message: Option<QueuedAgentMessageView>,
    cancelled_active_run: bool,
) -> Result<QueuedAgentMessageActionReceipt, String> {
    let revision = store
        .event_revision_by_metadata(&phase16_task_id(), "session_id", session_id)
        .map_err(|error| error.to_string())?;
    if let Some(message) = message.as_mut() {
        message.updated_at_ms = revision.latest_timestamp_ms;
    }
    Ok(QueuedAgentMessageActionReceipt {
        queue_id: queue_id.to_string(),
        message,
        event_count: revision.event_count,
        latest_sequence: revision.latest_sequence,
        latest_timestamp_ms: revision.latest_timestamp_ms,
        cancelled_active_run,
    })
}

#[tauri::command]
pub(crate) async fn edit_queued_agent_message(
    app: tauri::AppHandle,
    input: EditQueuedAgentMessageInput,
) -> Result<QueuedAgentMessageActionReceipt, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        edit_queued_agent_message_blocking(&state, input)
    })
    .await
    .map_err(|error| format!("queued message edit failed to join: {error}"))?
}

pub(crate) fn edit_queued_agent_message_blocking(
    state: &tauri::State<'_, AppState>,
    input: EditQueuedAgentMessageInput,
) -> Result<QueuedAgentMessageActionReceipt, String> {
    let run_context = project_session_metadata_for_session(state, Some(&input.session_id))?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let (queued, _) =
        queued_agent_message_from_read_model(&mut store, &input.session_id, &input.queue_id)?;
    let Some(mut queued) = queued else {
        return Err("queued message not found".to_string());
    };
    let prompt = input.prompt.trim();
    if prompt.is_empty() && queued.attachments.is_empty() {
        return Err("queued message is empty".to_string());
    }
    let edited_prompt = if prompt.is_empty() {
        format!(
            "Review attached {}",
            queued
                .attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        prompt.to_string()
    };
    let payload = QueuedAgentMessagePayload {
        prompt: edited_prompt.clone(),
        attachments: queued.attachments.clone(),
        effort: queued.effort.clone(),
        current_time: normalized_current_time_context(""),
    };
    append_agent_queue_event(
        &mut store,
        &run_context,
        "edit",
        &queued.id,
        &queued.mode,
        queued.created_at_ms,
        Some(&payload),
    )?;
    queued.prompt = edited_prompt;
    queued_agent_message_action_receipt(
        &store,
        &input.session_id,
        &input.queue_id,
        Some(queued),
        false,
    )
}

#[tauri::command]
pub(crate) async fn delete_queued_agent_message(
    app: tauri::AppHandle,
    input: QueuedAgentMessageActionInput,
) -> Result<QueuedAgentMessageActionReceipt, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        delete_queued_agent_message_blocking(&state, input)
    })
    .await
    .map_err(|error| format!("queued message delete failed to join: {error}"))?
}

pub(crate) fn delete_queued_agent_message_blocking(
    state: &tauri::State<'_, AppState>,
    input: QueuedAgentMessageActionInput,
) -> Result<QueuedAgentMessageActionReceipt, String> {
    let run_context = project_session_metadata_for_session(state, Some(&input.session_id))?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let (queued, _) =
        queued_agent_message_from_read_model(&mut store, &input.session_id, &input.queue_id)?;
    let Some(queued) = queued else {
        return queued_agent_message_action_receipt(
            &store,
            &input.session_id,
            &input.queue_id,
            None,
            false,
        );
    };
    append_agent_queue_event(
        &mut store,
        &run_context,
        "delete",
        &queued.id,
        &queued.mode,
        queued.created_at_ms,
        None,
    )?;
    queued_agent_message_action_receipt(&store, &input.session_id, &input.queue_id, None, false)
}

#[tauri::command]
pub(crate) async fn steer_queued_agent_message(
    app: tauri::AppHandle,
    input: QueuedAgentMessageActionInput,
) -> Result<QueuedAgentMessageActionReceipt, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        steer_queued_agent_message_blocking(&app, &state, input)
    })
    .await
    .map_err(|error| format!("queued message steer failed to join: {error}"))?
}

pub(crate) fn steer_queued_agent_message_blocking(
    _app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    input: QueuedAgentMessageActionInput,
) -> Result<QueuedAgentMessageActionReceipt, String> {
    let run_context = project_session_metadata_for_session(state, Some(&input.session_id))?;
    let mut queued = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let (queued, can_cancel) =
            queued_agent_message_from_read_model(&mut store, &input.session_id, &input.queue_id)?;
        let _ = can_cancel;
        queued.ok_or_else(|| "queued message not found".to_string())?
    };
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_agent_queue_event(
        &mut store,
        &run_context,
        "steer",
        &queued.id,
        "steer",
        queued.created_at_ms,
        None,
    )?;
    drop(store);
    if let Some(control) = active_agent_run_control(state, Some(&input.session_id))? {
        let _ = control.request_steer(queued.id.clone());
    }
    queued.mode = "steer".to_string();
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    queued_agent_message_action_receipt(
        &store,
        &input.session_id,
        &input.queue_id,
        Some(queued),
        false,
    )
}

#[tauri::command]
pub(crate) async fn run_next_queued_agent_message(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<Option<AgentState>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        run_next_queued_agent_message_blocking(&app, state, input)
    })
    .await
    .map_err(|error| format!("queued agent task failed to join: {error}"))?
}

pub(crate) fn run_next_queued_agent_message_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<Option<AgentState>, String> {
    let Some(_dispatch_lease) = begin_queue_dispatch(&state, &input.session_id)? else {
        return Ok(None);
    };
    run_next_queued_agent_message_blocking_inner(app, state.clone(), input)
}

pub(crate) fn begin_queue_dispatch<'a>(
    state: &'a tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<ExclusiveKeyLease<'a>>, String> {
    ExclusiveKeyLease::try_acquire(
        &state.queue_dispatching_sessions,
        session_id,
        "queue dispatch",
    )
}

pub(crate) fn run_next_queued_agent_message_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<Option<AgentState>, String> {
    let run_context = project_session_metadata_for_session(&state, Some(&input.session_id))?;
    let queued = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let Some(queued) =
            next_queued_agent_message_from_read_model(&mut store, &input.session_id)?
        else {
            return Ok(None);
        };
        append_agent_queue_event(
            &mut store,
            &run_context,
            "start",
            &queued.view.id,
            &queued.view.mode,
            queued.view.created_at_ms,
            None,
        )?;
        queued
    };
    let task_input = AgentTaskInput {
        prompt: queued.payload.prompt.clone(),
        session_id: input.session_id.clone(),
        current_time: normalized_current_time_context(""),
        queue_id: Some(queued.view.id.clone()),
        effort: queued.payload.effort.clone(),
        attachments: queued.payload.attachments.clone(),
    };
    match run_agent_task_blocking(app, state.clone(), task_input) {
        Ok(next) => {
            let mut store = state
                .store
                .lock()
                .map_err(|lock_error| format!("store lock poisoned: {lock_error}"))?;
            let started = load_agent_session_read_model(&mut store, &input.session_id)
                .map_err(|event_error| event_error.to_string())?
                .latest_run_queue_id
                .as_deref()
                == Some(queued.view.id.as_str());
            if started {
                return Ok(Some(next));
            }
            append_agent_queue_event(
                &mut store,
                &run_context,
                "restore",
                &queued.view.id,
                &queued.view.mode,
                queued.view.created_at_ms,
                Some(&queued.payload),
            )?;
            agent_state_for_session(&store, None, Some(&input.session_id))
                .map(Some)
                .map_err(|state_error| state_error.to_string())
        }
        Err(error) => {
            let mut store = state
                .store
                .lock()
                .map_err(|lock_error| format!("store lock poisoned: {lock_error}"))?;
            append_agent_queue_event(
                &mut store,
                &run_context,
                "restore",
                &queued.view.id,
                &queued.view.mode,
                queued.view.created_at_ms,
                Some(&queued.payload),
            )?;
            Err(error)
        }
    }
}

pub(crate) fn validate_agent_attachments(
    workspace_root: &Path,
    attachments: Vec<AgentAttachmentView>,
) -> Result<Vec<AgentAttachmentView>, String> {
    if attachments.len() > MAX_ATTACHMENT_FILES {
        return Err(format!(
            "a message can include at most {MAX_ATTACHMENT_FILES} attachments"
        ));
    }
    let mut validated = Vec::with_capacity(attachments.len());
    let mut total_bytes = 0u64;
    for attachment in attachments {
        if validated
            .iter()
            .any(|existing: &AgentAttachmentView| existing.path == attachment.path)
        {
            continue;
        }
        let path = validated_attachment_path(workspace_root, &attachment.path)?;
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("failed to inspect attachment: {error}"))?;
        if metadata.len() > MAX_ATTACHMENT_BYTES as u64 {
            return Err(format!(
                "{} exceeds the 20 MB attachment limit",
                attachment.name
            ));
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_ATTACHMENT_TOTAL_BYTES as u64 {
            return Err("attachments exceed the 50 MB message limit".to_string());
        }
        validated.push(AgentAttachmentView {
            id: attachment.id,
            name: safe_attachment_name(&attachment.name),
            path: path.display().to_string(),
            mime_type: normalized_attachment_mime(&attachment.mime_type, &path),
            size_bytes: metadata.len(),
        });
    }
    Ok(validated)
}

pub(crate) fn prompt_with_attachments(prompt: &str, attachments: &[AgentAttachmentView]) -> String {
    if attachments.is_empty() {
        return prompt.to_string();
    }
    let manifest = attachments
        .iter()
        .map(|attachment| {
            format!(
                "- {} ({}, {} bytes): {}",
                attachment.name, attachment.mime_type, attachment.size_bytes, attachment.path
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{prompt}\n\nAttached files have been staged inside the active workspace:\n{manifest}\nUse file tools when inspection is needed. Attached images are also supplied as visual references to compatible models."
    )
}

pub(crate) fn add_attachment_metadata(
    metadata: &mut Metadata,
    attachments: &[AgentAttachmentView],
) {
    if attachments.is_empty() {
        return;
    }
    metadata.insert(
        "attachment_paths".to_string(),
        attachments
            .iter()
            .map(|attachment| attachment.path.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    metadata.insert(
        "attachment_names".to_string(),
        attachments
            .iter()
            .map(|attachment| attachment.name.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    metadata.insert(
        "attachment_ids".to_string(),
        attachments
            .iter()
            .map(|attachment| attachment.id.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    metadata.insert(
        "attachment_mime_types".to_string(),
        attachments
            .iter()
            .map(|attachment| attachment.mime_type.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    metadata.insert(
        "attachment_sizes".to_string(),
        attachments
            .iter()
            .map(|attachment| attachment.size_bytes.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let image_paths = attachments
        .iter()
        .filter(|attachment| attachment.mime_type.starts_with("image/"))
        .map(|attachment| attachment.path.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if !image_paths.is_empty() {
        metadata.insert("image_paths".to_string(), image_paths);
    }
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
    let collaboration = match prepare_agent_collaboration(
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
        if !collaboration.guidance.is_empty() {
            history.push(Message {
                role: MessageRole::System,
                content: format!(
                    "Multi-model team guidance for the next user request:\n{}",
                    collaboration.guidance
                ),
                metadata: [
                    ("internal".to_string(), "true".to_string()),
                    ("collaboration_id".to_string(), collaboration.id.clone()),
                    ("collaboration_stage".to_string(), "guidance".to_string()),
                ]
                .into_iter()
                .collect(),
            });
        }
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
    let result = if let Some(suspended) = suspended {
        resume_suspended_agent_run(app, &state, suspended, &cancellation)
    } else {
        retry_agent_task_blocking_inner(app, state.clone(), input, &cancellation)
    };
    result
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
        run_context.insert("recovery_resume_key".to_string(), recovery.resume_key);
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

    let (mut history, artifact_manifest) = {
        let store = state
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
        (messages, artifact_manifest_message(&session_events))
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
    let collaboration = match prepare_agent_collaboration(
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
        if !collaboration.guidance.is_empty() {
            history.push(Message {
                role: MessageRole::System,
                content: format!(
                    "Multi-model team guidance for the next user request:\n{}",
                    collaboration.guidance
                ),
                metadata: [
                    ("internal".to_string(), "true".to_string()),
                    ("collaboration_id".to_string(), collaboration.id.clone()),
                    ("collaboration_stage".to_string(), "guidance".to_string()),
                ]
                .into_iter()
                .collect(),
            });
        }
    }
    if agent_run_should_stop(cancellation) {
        return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
    }
    cancellation.mark_progress("executor", "Starting execution");
    append_agent_progress_event(&state, &task_id, &run_context, "Starting execution")?;
    let runtime_config = cancellation.runtime_config();
    let runtime = if history.is_empty() {
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

#[tauri::command]
pub(crate) async fn resolve_agent_permission(
    app: tauri::AppHandle,
    request_id: String,
    decision: String,
    session_id: String,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        resolve_agent_permission_blocking(&app, state, request_id, decision, session_id)
    })
    .await
    .map_err(|error| format!("agent permission resume failed to join: {error}"))?
}

pub(crate) fn resolve_agent_permission_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
) -> Result<AgentState, String> {
    let snapshot = suspended_agent_run_control_snapshot(&state, &session_id)?;
    let effort = if snapshot.is_some() {
        AgentEffort::Auto
    } else {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .get_permission_request(&PermissionRequestId(request_id.clone()))
            .map_err(|error| error.to_string())?
            .and_then(|request| request.metadata.get("agent_effort").cloned())
            .map(|effort| AgentEffort::parse(&effort))
            .unwrap_or(AgentEffort::Auto)
    };
    let run_control_lease =
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), snapshot)?;
    let cancellation = run_control_lease.control();
    let result = resolve_agent_permission_blocking_inner(
        app,
        state.clone(),
        request_id,
        decision,
        session_id.clone(),
        &cancellation,
    );
    result
}

pub(crate) fn resolve_agent_permission_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let mut run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;
    if matches!(&decision, PermissionDecision::AllowForSession)
        && matches!(&request.risk, PermissionRisk::Destructive)
    {
        return agent_state_for_session(
            &store,
            Some("destructive permissions can only be allowed once".to_string()),
            Some(&session_id),
        )
        .map_err(|error| error.to_string());
    }
    for key in [
        "agent_run_id",
        "agent_effort",
        "agent_model",
        "requested_policy",
        "collaboration_policy",
        "current_time",
        "task_class",
        "collaboration_profile",
        "queue_id",
        "recovery_resume_key",
        "recovery_attempts",
        "image_generation_required",
        "configured_image_model",
        "configured_image_endpoint",
    ] {
        if let Some(value) = request.metadata.get(key) {
            run_context.insert(key.to_string(), value.clone());
        }
    }
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();

    if request.task_id != phase16_task_id() {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the agent loop".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }
    let current =
        agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?;
    if !current
        .pending_approvals
        .iter()
        .any(|approval| approval.request_id == request_id.0)
    {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the active session".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }
    drop(store);

    let mut resolved_observations = vec![resolve_agent_permission_request(
        &state,
        &request,
        &decision,
        "local-user",
        &root,
        &run_context,
    )?];

    if matches!(&decision, PermissionDecision::AllowForSession) {
        let pending = {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            pending_agent_permissions_for_run(
                &store,
                &phase16_task_id(),
                session_id,
                run_context.get("agent_run_id").map(String::as_str),
            )
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|pending| !matches!(&pending.risk, PermissionRisk::Destructive))
            .collect::<Vec<_>>()
        };
        for pending_request in pending {
            resolved_observations.push(resolve_agent_permission_request(
                &state,
                &pending_request,
                &PermissionDecision::AllowForSession,
                "session-grant",
                &root,
                &run_context,
            )?);
        }
    }

    append_observations_to_suspended_run(
        &state,
        session_id.unwrap_or_default(),
        &resolved_observations,
    )?;

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let pending = pending_agent_permissions_for_run(
        &store,
        &phase16_task_id(),
        session_id,
        run_context.get("agent_run_id").map(String::as_str),
    )
    .map_err(|error| error.to_string())?;
    if !pending.is_empty() {
        return agent_state_for_session(&store, None, session_id)
            .map_err(|error| error.to_string());
    }

    if let Some(recovery) = claim_agent_recovery_envelope(
        &mut store,
        &run_context,
        &["blocked"],
        "permission_resolved",
    )? {
        run_context.insert("recovery_resume_key".to_string(), recovery.resume_key);
        run_context.insert(
            "recovery_attempts".to_string(),
            recovery.attempts.to_string(),
        );
        run_context.insert("continuation".to_string(), "true".to_string());
        if let Some(queue_id) = recovery.queue_id {
            run_context.insert("queue_id".to_string(), queue_id);
        }
    }

    append_event(
        &mut store,
        &request.task_id,
        EventKind::TaskStatusChanged,
        "Agent task resumed after permission",
        metadata_with_context(
            [
                ("permission_id".to_string(), request_id.0),
                (
                    "decision".to_string(),
                    permission_decision_label(&decision).to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    let events = store
        .list_by_task(&phase16_task_id())
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let prompt = latest_agent_prompt_from_active_events(&active_events)
        .unwrap_or_else(|| "Continue the agent task.".to_string());
    let transcript = agent_transcript_from_active_events(&active_events);
    drop(store);

    if let Some(session_id) = session_id {
        if let Some(mut suspended) = take_suspended_agent_run(&state, session_id)? {
            cancellation.extend_runtime_budget(&mut suspended.runtime);
            for (key, value) in &run_context {
                suspended.run_context.insert(key.clone(), value.clone());
            }
            return continue_agent_loop(
                app,
                &state,
                &config,
                &suspended.workspace_root,
                suspended.runtime,
                suspended.prompt,
                suspended.run_context,
                suspended.collaboration.as_ref(),
                cancellation,
            );
        }
    }

    let mut runtime = resume_agent_loop_from_messages(
        phase16_task_id(),
        prompt.clone(),
        transcript,
        cancellation.runtime_config(),
    );
    cancellation.extend_runtime_budget(&mut runtime);
    continue_agent_loop(
        app,
        &state,
        &config,
        &root,
        runtime,
        prompt,
        run_context,
        None,
        cancellation,
    )
}

pub(crate) fn resolve_agent_permission_request(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    decision: &PermissionDecision,
    resolved_by: &str,
    root: &Path,
    run_context: &Metadata,
) -> Result<ResolvedToolObservation, String> {
    let request_id = request.id.clone();
    let tool_call_id = request
        .metadata
        .get("tool_call_id")
        .cloned()
        .unwrap_or_else(|| unique_id("agent-tool"));
    let tool_name = request
        .metadata
        .get("tool_name")
        .cloned()
        .unwrap_or_else(|| request.action.clone());
    let tool_input = request
        .metadata
        .get("tool_input")
        .cloned()
        .unwrap_or_default();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: decision.clone(),
            resolved_at_ms: current_time_millis(),
            resolved_by: resolved_by.to_string(),
        })
        .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(decision)),
        metadata_with_context(
            [
                ("permission_id".to_string(), request_id.0),
                (
                    "decision".to_string(),
                    permission_decision_label(decision).to_string(),
                ),
                ("tool".to_string(), request.action.clone()),
                ("resolved_by".to_string(), resolved_by.to_string()),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;

    let (observation, status, image_paths) = if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(tool_call_id.clone()),
            task_id: request.task_id.clone(),
            tool_name: tool_name.clone(),
            input_json: tool_input.clone(),
            proposed_by_model: "agent-loop".to_string(),
            metadata: Metadata::new(),
        };
        drop(store);
        let registry = tool_registry_for_state(state, root)?;
        let result =
            execute_agent_tool_invocation(state, &registry, invocation, root, run_context)?;
        let observation = observation_from_agent_tool_result(&tool_name, &result);
        let image_paths = tool_result_image_paths(&result);
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_tool_message_event(
            &mut store,
            &request.task_id,
            &tool_call_id,
            &tool_name,
            tool_outcome_label(&result.status),
            &observation,
            Some(run_context),
        )
        .map_err(|error| error.to_string())?;
        if !image_paths.is_empty() {
            append_visual_reference_event(
                &mut store,
                &request.task_id,
                &tool_name,
                &image_paths,
                run_context,
            )?;
        }
        (observation, result.status, image_paths)
    } else {
        let observation = observation_from_tool_result(
            &request.action,
            "denied",
            "The user denied this tool call.",
        );
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Agent tool denied",
            metadata_with_context(
                [
                    ("tool_call_id".to_string(), tool_call_id.clone()),
                    ("tool".to_string(), request.action.clone()),
                    ("status".to_string(), "denied".to_string()),
                    ("output".to_string(), observation.clone()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        append_tool_message_event(
            &mut store,
            &request.task_id,
            &tool_call_id,
            &request.action,
            "denied",
            &observation,
            Some(run_context),
        )
        .map_err(|error| error.to_string())?;
        (observation, ToolOutcomeStatus::Denied, Vec::new())
    };
    Ok(ResolvedToolObservation {
        call_id: agent_core::ToolCallId(tool_call_id),
        tool_name,
        input_json: tool_input,
        status,
        observation,
        image_paths,
    })
}
