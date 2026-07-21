use super::*;

#[tauri::command]
pub(crate) async fn get_agent_state(
    app: tauri::AppHandle,
    session_id: Option<String>,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let run_context = project_session_metadata_for_session(&state, session_id.as_deref())?;
        let Some(session_id) = run_context.get("session_id").cloned() else {
            return Ok(empty_agent_state_for_session(""));
        };
        let store = open_app_read_store()?;
        store
            .with_read_snapshot(|snapshot| {
                let model = load_agent_session_read_model_snapshot(snapshot, &session_id)?;
                let history = snapshot.list_by_task_and_metadata_before_with_tool_metadata_limit(
                    &phase16_task_id(),
                    "session_id",
                    &session_id,
                    u64::MAX,
                    AGENT_HISTORY_INITIAL_PAGE_SIZE,
                    AGENT_HISTORY_MAX_TOOL_METADATA_BYTES,
                )?;
                agent_state_from_read_model(snapshot, &model, &session_id, &run_context, history)
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("agent state load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) async fn get_agent_state_revision(
    session_id: String,
) -> Result<AgentStateRevision, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = open_app_read_store()?;
        let revision = store
            .event_revision_by_metadata(&phase16_task_id(), "session_id", &session_id)
            .map_err(|error| error.to_string())?;
        Ok(AgentStateRevision {
            session_id,
            event_count: revision.event_count,
            latest_sequence: revision.latest_sequence,
            latest_timestamp_ms: revision.latest_timestamp_ms,
        })
    })
    .await
    .map_err(|error| format!("agent state revision load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) async fn get_agent_state_delta(
    app: tauri::AppHandle,
    session_id: String,
    after_sequence: u64,
) -> Result<AgentStateDelta, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
        let store = open_app_read_store()?;
        store
            .with_read_snapshot(|snapshot| {
                let model = load_agent_session_read_model_snapshot(snapshot, &session_id)?;
                let latest_sequence = model.revision;
                let reset = after_sequence == 0 || after_sequence > latest_sequence;
                let events = if reset {
                    snapshot.list_by_task_and_metadata_before_with_tool_metadata_limit(
                        &phase16_task_id(),
                        "session_id",
                        &session_id,
                        u64::MAX,
                        AGENT_HISTORY_INITIAL_PAGE_SIZE,
                        AGENT_HISTORY_MAX_TOOL_METADATA_BYTES,
                    )
                } else {
                    snapshot.list_by_task_and_metadata_after_with_tool_metadata_limit(
                        &phase16_task_id(),
                        "session_id",
                        &session_id,
                        after_sequence,
                        AGENT_HISTORY_MAX_TOOL_METADATA_BYTES,
                    )
                }?;
                let agent_state = agent_state_from_read_model(
                    snapshot,
                    &model,
                    &session_id,
                    &run_context,
                    events,
                )?;
                Ok(AgentStateDelta {
                    reset,
                    latest_sequence,
                    state: agent_state,
                })
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("agent state delta load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) async fn get_agent_history_page(
    session_id: String,
    before_sequence: u64,
    limit: Option<usize>,
) -> Result<AgentHistoryPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = open_app_read_store()?;
        let events = store
            .list_by_task_and_metadata_before_with_tool_metadata_limit(
                &phase16_task_id(),
                "session_id",
                &session_id,
                before_sequence,
                limit
                    .unwrap_or(AGENT_HISTORY_INITIAL_PAGE_SIZE)
                    .clamp(1, AGENT_HISTORY_MAX_PAGE_SIZE),
                AGENT_HISTORY_MAX_TOOL_METADATA_BYTES,
            )
            .map_err(|error| error.to_string())?;
        let oldest_sequence = events
            .first()
            .map(|event| event.sequence)
            .unwrap_or(before_sequence);
        let has_older_history = oldest_sequence > 0
            && store
                .has_task_metadata_event_before(
                    &phase16_task_id(),
                    "session_id",
                    &session_id,
                    oldest_sequence,
                )
                .map_err(|error| error.to_string())?;
        let audits = agent_session_audits(&store, &session_id, None, 0)
            .map_err(|error| error.to_string())?;
        Ok(AgentHistoryPage {
            session_id,
            oldest_sequence,
            has_older_history,
            timeline: events
                .iter()
                .filter(|event| !is_agent_queue_event(event))
                .cloned()
                .map(|event| timeline_entry(event, &audits))
                .collect(),
            messages: events.iter().filter_map(message_view_from_event).collect(),
        })
    })
    .await
    .map_err(|error| format!("agent history page load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) async fn get_agent_trace_state(
    app: tauri::AppHandle,
    session_id: Option<String>,
) -> Result<AgentTraceState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let run_context = project_session_metadata_for_session(&state, session_id.as_deref())?;
        let session_id = run_context.get("session_id").map(String::as_str);
        let store = open_app_read_store()?;
        store
            .with_read_snapshot(|snapshot| {
                let active_run_id = if let Some(session_id) = session_id {
                    load_agent_session_read_model_snapshot(snapshot, session_id)?.active_run_id
                } else {
                    None
                };
                let events = if let Some(run_id) = active_run_id {
                    snapshot.list_by_task_and_metadata(
                        &phase16_task_id(),
                        "agent_run_id",
                        &run_id,
                    )?
                } else {
                    agent_events_for_session(snapshot, &phase16_task_id(), session_id)?
                };
                agent_trace_state_from_events(snapshot, None, None, session_id, events)
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("agent trace load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) async fn get_agent_session_outputs(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<Vec<AgentOutputArtifactView>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
        let session_id = run_context
            .get("session_id")
            .map(String::as_str)
            .unwrap_or(session_id.as_str());
        let store = open_app_read_store()?;
        let events = agent_events_for_session(&store, &phase16_task_id(), Some(session_id))
            .map_err(|error| error.to_string())?;
        let root = run_context
            .get("project_root")
            .map(PathBuf::from)
            .unwrap_or(active_workspace_root(&state)?);
        let mut outputs = agent_output_artifacts_from_events(&events);
        for output in &mut outputs {
            let path = PathBuf::from(&output.path);
            let resolved = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            if resolved.is_dir() {
                output.kind = "directory".to_string();
            }
        }
        Ok(outputs)
    })
    .await
    .map_err(|error| format!("agent outputs load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) fn export_agent_trace_jsonl(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<AgentTraceState, String> {
    let run_context = project_session_metadata_for_session(&state, session_id.as_deref())?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let export_path =
        write_agent_trace_jsonl(&root, &store, session_id).map_err(|error| error.to_string())?;
    agent_trace_state_for_session(&store, Some(export_path), None, session_id)
        .map_err(|error| error.to_string())
}

pub(crate) fn begin_agent_run_control_for_effort<'a>(
    state: &'a tauri::State<'_, AppState>,
    session_id: &str,
    effort: &str,
    snapshot: Option<RunControlSnapshot>,
) -> Result<RegisteredRunControl<'a>, String> {
    cancel_background_prompt_evaluations(state)?;
    let control = Arc::new(
        snapshot
            .map(AgentRunControl::from_snapshot)
            .unwrap_or_else(|| AgentRunControl::new(effort)),
    );
    RegisteredRunControl::register(
        &state.agent_run_controls,
        session_id,
        control,
        "agent run control",
        "agent run is already active for this session",
    )
}

pub(crate) fn begin_agent_run_control_for_continuation<'a>(
    state: &'a tauri::State<'_, AppState>,
    session_id: &str,
    snapshot: RunControlSnapshot,
) -> Result<RegisteredRunControl<'a>, String> {
    cancel_background_prompt_evaluations(state)?;
    let control = Arc::new(
        AgentRunControl::from_snapshot_for_continuation(snapshot)
            .map_err(|reason| format!("agent run cannot continue after {}", reason.code()))?,
    );
    RegisteredRunControl::register(
        &state.agent_run_controls,
        session_id,
        control,
        "agent run control",
        "agent run is already active for this session",
    )
}

pub(crate) fn cancel_background_prompt_evaluations(
    state: &tauri::State<'_, AppState>,
) -> Result<(), String> {
    let controls = state
        .prompt_evaluation_controls
        .lock()
        .map_err(|error| format!("prompt evaluation control lock poisoned: {error}"))?;
    for control in controls.values() {
        control.request_cancel();
    }
    Ok(())
}

pub(crate) fn active_agent_run_control(
    state: &tauri::State<'_, AppState>,
    session_id: Option<&str>,
) -> Result<Option<Arc<AgentRunControl>>, String> {
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    Ok(state
        .agent_run_controls
        .lock()
        .map_err(|error| format!("agent run control lock poisoned: {error}"))?
        .get(session_id)
        .cloned())
}

pub(crate) fn request_agent_run_cancel(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<bool, String> {
    if let Some(control) = active_agent_run_control(state, Some(session_id))? {
        control.request_cancel();
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn agent_run_should_stop(control: &Arc<AgentRunControl>) -> bool {
    control.should_stop()
}

pub(crate) fn add_agent_run_budget_metadata(metadata: &mut Metadata, control: &AgentRunControl) {
    let budget = control.budget();
    metadata.insert(
        "run_budget_ms".to_string(),
        budget.max_duration.as_millis().to_string(),
    );
    metadata.insert(
        "run_model_call_budget".to_string(),
        budget.max_model_calls.to_string(),
    );
    metadata.insert(
        "run_initial_model_calls".to_string(),
        budget.initial_model_calls.to_string(),
    );
    metadata.insert(
        "run_tool_call_budget".to_string(),
        budget.max_tool_calls.to_string(),
    );
    metadata.insert(
        "run_initial_tool_calls".to_string(),
        budget.initial_tool_calls.to_string(),
    );
    metadata.insert(
        "run_model_timeout_ms".to_string(),
        budget.model_call_timeout.as_millis().to_string(),
    );
    metadata.insert(
        "run_tool_timeout_ms".to_string(),
        budget.tool_call_timeout.as_millis().to_string(),
    );
    metadata.insert(
        "run_no_progress_ms".to_string(),
        budget.no_progress_timeout.as_millis().to_string(),
    );
}

pub(crate) fn append_agent_progress_event(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    summary: &str,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        summary,
        run_context.clone(),
    )
    .map_err(|error| format!("failed to record agent progress `{summary}`: {error}"))?;
    Ok(())
}

pub(crate) fn cancelled_agent_state(
    state: &tauri::State<'_, AppState>,
    session_id: Option<&str>,
) -> Result<AgentState, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
}

pub(crate) fn append_agent_queue_event(
    store: &mut SqliteStore,
    run_context: &Metadata,
    action: &str,
    queue_id: &str,
    mode: &str,
    created_at_ms: u64,
    payload: Option<&QueuedAgentMessagePayload>,
) -> Result<(), String> {
    let mut metadata = [
        ("queue_action".to_string(), action.to_string()),
        ("queue_id".to_string(), queue_id.to_string()),
        ("queue_mode".to_string(), mode.to_string()),
        ("queue_created_at_ms".to_string(), created_at_ms.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(payload) = payload {
        metadata.insert(
            "queue_payload".to_string(),
            serde_json::to_string(payload)
                .map_err(|error| format!("failed to encode queued message: {error}"))?,
        );
    }
    let summary = match action {
        "enqueue" => "Agent message queued",
        "edit" => "Queued agent message edited",
        "steer" => "Agent steer requested",
        "delete" => "Queued agent message deleted",
        "start" => "Queued agent message started",
        "restore" => "Queued agent message restored",
        _ => "Agent queue updated",
    };
    append_event(
        store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        summary,
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn finish_agent_run_for_control_stop(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    control: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let Some(reason) = control.stop_reason() else {
        return Err("agent run stopped without a reason".to_string());
    };
    let session_id = run_context.get("session_id").map(String::as_str);
    if reason.is_user_cancelled() {
        clear_suspended_agent_run_for_context(state, run_context)?;
        return cancelled_agent_state(state, session_id);
    }

    let partial = control.partial_output();
    let answer = if partial.trim().is_empty() {
        format!(
            "Cindx paused this run at a safety checkpoint ({}). No verified partial result was available. Continue to resume with a fresh run budget.",
            reason.code()
        )
    } else {
        format!(
            "Cindx paused this run at a safety checkpoint ({}). Continue to resume from the saved execution state. Latest intermediate result:\n\n{}",
            reason.code(),
            partial.trim()
        )
    };
    let progress = control.progress();
    let mut metadata = metadata_with_context(
        [
            ("partial".to_string(), "true".to_string()),
            ("continuation_available".to_string(), "true".to_string()),
            ("stop_reason".to_string(), reason.code().to_string()),
            (
                "elapsed_ms".to_string(),
                progress.elapsed.as_millis().to_string(),
            ),
            ("model_calls".to_string(), progress.model_calls.to_string()),
            ("tool_calls".to_string(), progress.tool_calls.to_string()),
            (
                "model_call_limit".to_string(),
                progress.model_call_limit.to_string(),
            ),
            (
                "tool_call_limit".to_string(),
                progress.tool_call_limit.to_string(),
            ),
            ("checkpoints".to_string(), progress.checkpoints.to_string()),
            (
                "budget_extensions".to_string(),
                progress.budget_extensions.to_string(),
            ),
            ("last_stage".to_string(), progress.stage.clone()),
            ("last_detail".to_string(), progress.detail.clone()),
        ]
        .into_iter()
        .collect(),
        run_context,
    );
    metadata.insert("model".to_string(), "run-control".to_string());
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        &answer,
        metadata,
    )
    .map_err(|error| error.to_string())?;
    let events = agent_events_for_session(&store, &phase16_task_id(), session_id)
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let recovery_metadata = agent_recovery_metadata(
        &active_events,
        run_context,
        "paused",
        reason.code(),
        [
            ("completion".to_string(), "partial".to_string()),
            ("stop_reason".to_string(), reason.code().to_string()),
            (
                "elapsed_ms".to_string(),
                progress.elapsed.as_millis().to_string(),
            ),
            ("last_stage".to_string(), progress.stage),
            ("last_detail".to_string(), progress.detail),
        ]
        .into_iter()
        .collect(),
    )?;
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        recovery_metadata,
    )
    .map_err(|error| error.to_string())?;
    drop(store);
    emit_agent_stream_delta(
        app,
        "agent-budget-stop",
        session_id,
        &answer,
        false,
        true,
        None,
    );
    emit_agent_stream_delta(app, "agent-budget-stop", session_id, "", true, false, None);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
}

pub(crate) fn emit_agent_stream_delta(
    app: &tauri::AppHandle,
    request_id: &str,
    session_id: Option<&str>,
    delta: &str,
    done: bool,
    reset: bool,
    error: Option<String>,
) {
    let _ = app.emit(
        "model-stream-delta",
        ModelStreamDelta {
            task_id: PHASE16_TASK_ID.to_string(),
            request_id: request_id.to_string(),
            session_id: session_id.map(str::to_string),
            delta: delta.to_string(),
            done,
            reset,
            error,
        },
    );
}
