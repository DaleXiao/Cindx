use super::task::run_agent_task_blocking;
use crate::*;

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
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
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
        effort: AgentPolicy::parse_ingress(&input.effort).label().to_string(),
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
    steer_committed: bool,
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
        steer_committed,
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
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
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
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
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
    queued_agent_message_action_receipt(
        &store,
        &input.session_id,
        &input.queue_id,
        None,
        false,
        false,
    )
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
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
    let run_context = project_session_metadata_for_session(state, Some(&input.session_id))?;
    let control = active_agent_run_control(state, Some(&input.session_id))?;
    let load_current = || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        require_queued_agent_message(&mut store, &input.session_id, &input.queue_id)
    };
    let (queued, steer_committed) = if let Some(control) = control {
        match control.commit_steer_request_with(input.queue_id.clone(), || {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            commit_queued_agent_steer(&mut store, &run_context, &input.session_id, &input.queue_id)
        })? {
            agent_runtime::RunSteerRequestCommit::Committed { value, .. } => (value, true),
            agent_runtime::RunSteerRequestCommit::Duplicate => (load_current()?, true),
            agent_runtime::RunSteerRequestCommit::CapacityReached
            | agent_runtime::RunSteerRequestCommit::Stopped(_)
            | agent_runtime::RunSteerRequestCommit::TerminalCommitted => (load_current()?, false),
        }
    } else {
        (load_current()?, false)
    };
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
        steer_committed,
    )
}

fn require_queued_agent_message(
    store: &mut SqliteStore,
    session_id: &str,
    queue_id: &str,
) -> Result<QueuedAgentMessageView, String> {
    let (queued, _) = queued_agent_message_from_read_model(store, session_id, queue_id)?;
    queued.ok_or_else(|| "queued message not found".to_string())
}

pub(crate) fn commit_queued_agent_steer(
    store: &mut SqliteStore,
    run_context: &Metadata,
    session_id: &str,
    queue_id: &str,
) -> Result<QueuedAgentMessageView, String> {
    let mut queued = require_queued_agent_message(store, session_id, queue_id)?;
    append_agent_queue_event(
        store,
        run_context,
        "steer",
        &queued.id,
        "steer",
        queued.created_at_ms,
        None,
    )?;
    queued.mode = "steer".to_string();
    Ok(queued)
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

pub(crate) fn begin_queue_dispatch(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<agent_harness::ExclusiveKeyLease>, String> {
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
    project_session_metadata_for_session(state, Some(session_id))?;
    state
        .queue_dispatching_sessions
        .try_acquire(session_id)
        .map_err(|error| error.to_string())
}

pub(crate) fn run_next_queued_agent_message_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<Option<AgentState>, String> {
    if !state
        .queue_dispatching_sessions
        .contains(&input.session_id)
        .map_err(|error| error.to_string())?
    {
        return Err("queue dispatch lease is required".to_string());
    }
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
            let restored = restore_queued_agent_message_before_run_start(
                &mut store,
                &run_context,
                &input.session_id,
                &queued,
            )?;
            if !restored {
                return Ok(Some(next));
            }
            agent_state_for_session(&store, None, Some(&input.session_id))
                .map(Some)
                .map_err(|state_error| state_error.to_string())
        }
        Err(error) => {
            let mut store = state
                .store
                .lock()
                .map_err(|lock_error| format!("store lock poisoned: {lock_error}"))?;
            restore_queued_agent_message_before_run_start(
                &mut store,
                &run_context,
                &input.session_id,
                &queued,
            )?;
            Err(error)
        }
    }
}

pub(crate) fn restore_queued_agent_message_before_run_start(
    store: &mut SqliteStore,
    run_context: &Metadata,
    session_id: &str,
    queued: &PendingQueuedAgentMessage,
) -> Result<bool, String> {
    let started = load_agent_session_read_model(store, session_id)
        .map_err(|error| error.to_string())?
        .latest_run_queue_id
        .as_deref()
        == Some(queued.view.id.as_str());
    if started {
        return Ok(false);
    }
    append_agent_queue_event(
        store,
        run_context,
        "restore",
        &queued.view.id,
        &queued.view.mode,
        queued.view.created_at_ms,
        Some(&queued.payload),
    )?;
    Ok(true)
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
