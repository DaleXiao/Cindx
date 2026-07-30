use super::*;
use crate::desktop_event_sink::DesktopEventSink;

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
        let agent = store
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
            .map_err(|error| error.to_string())?;
        match persist_completed_conversation_title(&state, &session_id, &agent.messages) {
            Ok(Some(refinement)) => {
                spawn_semantic_session_title_refinement(app.clone(), refinement);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("failed to prepare session title repair for {session_id}: {error}")
            }
        }
        Ok(agent)
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
        let mut outputs = cached_agent_output_artifacts(&state, &store, session_id)?;
        let root = run_context
            .get("project_root")
            .map(PathBuf::from)
            .unwrap_or(active_workspace_root(&state)?);
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
    let store = open_app_read_store()?;
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

pub(crate) fn begin_agent_run_control_at_steer_epoch<'a>(
    state: &'a tauri::State<'_, AppState>,
    session_id: &str,
    effort: &str,
    applied_epoch: u64,
) -> Result<RegisteredRunControl<'a>, String> {
    cancel_background_prompt_evaluations(state)?;
    RegisteredRunControl::register(
        &state.agent_run_controls,
        session_id,
        Arc::new(AgentRunControl::new_at_steer_epoch(effort, applied_epoch)),
        "agent run control",
        "agent run is already active for this session",
    )
}

pub(crate) fn begin_agent_run_control_from_persisted_resources<'a>(
    state: &'a tauri::State<'_, AppState>,
    session_id: &str,
    effort: &str,
    applied_epoch: u64,
    resources: RunResourceSnapshot,
    start_new_segment: bool,
) -> Result<RegisteredRunControl<'a>, String> {
    cancel_background_prompt_evaluations(state)?;
    let control = if start_new_segment {
        AgentRunControl::new_for_continuation_at_steer_epoch_with_resource_snapshot(
            effort,
            applied_epoch,
            resources,
        )
    } else {
        AgentRunControl::new_at_steer_epoch_with_resource_snapshot(effort, applied_epoch, resources)
    };
    RegisteredRunControl::register(
        &state.agent_run_controls,
        session_id,
        Arc::new(control),
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
        return Ok(control.request_cancel());
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
    metadata.insert(
        "run_total_token_budget".to_string(),
        budget.max_total_tokens.to_string(),
    );
    metadata.insert(
        "run_physical_model_attempt_budget".to_string(),
        budget.max_physical_model_attempts.to_string(),
    );
    metadata.insert(
        "run_terminal_token_reserve".to_string(),
        budget.terminal_token_reserve.to_string(),
    );
    metadata.insert(
        "run_terminal_physical_model_attempt_reserve".to_string(),
        budget.terminal_physical_model_attempt_reserve.to_string(),
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
    finish_agent_run_for_control_stop_with_task_state(app, state, run_context, control, None)
}

pub(crate) fn finish_agent_run_for_control_stop_with_task_state(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    control: &Arc<AgentRunControl>,
    task_state: Option<&AgentTaskStateSnapshot>,
) -> Result<AgentState, String> {
    let Some(reason) = control.stop_reason() else {
        return Err("agent run stopped without a reason".to_string());
    };
    let session_id = run_context.get("session_id").map(String::as_str);
    if reason.is_user_cancelled() {
        clear_suspended_agent_run_for_context(state, run_context)?;
        return cancelled_agent_state(state, session_id);
    }

    let best_known = control
        .best_known_result()
        .filter(|result| result.deliverable);
    let partial = control.partial_output();
    let answer = if let Some(result) = best_known.as_ref() {
        result.content.clone()
    } else if partial.trim().is_empty() {
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
    let resource_snapshot = control.resource_usage();
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
            (
                "material_checkpoints".to_string(),
                progress.checkpoints.to_string(),
            ),
            ("checkpoints".to_string(), progress.checkpoints.to_string()),
            (
                "observations".to_string(),
                progress.observations.to_string(),
            ),
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
    if let Some(result) = best_known.as_ref() {
        metadata.insert("degraded_delivery".to_string(), "true".to_string());
        metadata.insert("best_known_stage".to_string(), result.stage.clone());
        metadata.insert(
            "best_known_quality".to_string(),
            result.quality.as_str().to_string(),
        );
        metadata.insert(
            "best_known_evidence_count".to_string(),
            result.evidence_count.to_string(),
        );
        metadata.insert(
            "best_known_verified".to_string(),
            result.verified.to_string(),
        );
    }
    crate::model_resource_runtime::add_model_resource_snapshot_metadata(
        &mut metadata,
        &resource_snapshot,
    );
    metadata.insert("model".to_string(), "run-control".to_string());
    let recovery_metadata = [
        ("completion".to_string(), "partial".to_string()),
        ("stop_reason".to_string(), reason.code().to_string()),
        (
            "elapsed_ms".to_string(),
            progress.elapsed.as_millis().to_string(),
        ),
        (
            "material_checkpoints".to_string(),
            progress.checkpoints.to_string(),
        ),
        (
            "observations".to_string(),
            progress.observations.to_string(),
        ),
        (
            "budget_extensions".to_string(),
            progress.budget_extensions.to_string(),
        ),
        ("last_stage".to_string(), progress.stage),
        ("last_detail".to_string(), progress.detail),
    ]
    .into_iter()
    .collect();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    persist_paused_agent_run(
        &mut store,
        run_context,
        &answer,
        metadata,
        reason.code(),
        recovery_metadata,
        task_state,
        &resource_snapshot,
    )?;
    if let Err(error) = refresh_project_memory_after_run(&mut store, run_context) {
        eprintln!("project memory checkpoint unavailable: {error}");
    }
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

#[allow(clippy::too_many_arguments)]
fn persist_paused_agent_run(
    store: &mut SqliteStore,
    run_context: &Metadata,
    answer: &str,
    assistant_metadata: Metadata,
    reason: &str,
    recovery_metadata: Metadata,
    task_state: Option<&AgentTaskStateSnapshot>,
    resource_snapshot: &RunResourceSnapshot,
) -> Result<(), String> {
    persist_paused_agent_run_with(
        store,
        run_context,
        answer,
        assistant_metadata,
        reason,
        recovery_metadata,
        task_state,
        resource_snapshot,
        |store, session_id| {
            delete_persisted_agent_runtime_snapshot(store, session_id).map_err(StorageError::new)
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_paused_agent_run_with(
    store: &mut SqliteStore,
    run_context: &Metadata,
    answer: &str,
    assistant_metadata: Metadata,
    reason: &str,
    recovery_metadata: Metadata,
    task_state: Option<&AgentTaskStateSnapshot>,
    resource_snapshot: &RunResourceSnapshot,
    snapshot_handoff: impl FnOnce(&mut SqliteStore, Option<&str>) -> Result<(), StorageError>,
) -> Result<(), String> {
    let session_id = run_context.get("session_id").map(String::as_str);
    store
        .with_immediate_transaction(|store| {
            append_message_event_with_metadata(
                store,
                &phase16_task_id(),
                MessageRole::Assistant,
                answer,
                assistant_metadata,
            )?;
            let events = agent_events_for_session(store, &phase16_task_id(), session_id)?;
            let active_events = active_agent_events_for_session(&events, session_id);
            let recovery_metadata = agent_recovery_metadata_with_task_state(
                &active_events,
                run_context,
                "paused",
                reason,
                recovery_metadata,
                task_state,
                Some(resource_snapshot),
            )
            .map_err(StorageError::new)?;
            append_event(
                store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                "Agent task paused",
                recovery_metadata,
            )?;
            if task_state.is_some() {
                snapshot_handoff(store, session_id)?;
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
}

pub(crate) fn emit_agent_stream_delta(
    events: &impl DesktopEventSink,
    request_id: &str,
    session_id: Option<&str>,
    delta: &str,
    done: bool,
    reset: bool,
    error: Option<String>,
) {
    events.emit_model_stream_delta(ModelStreamDelta {
        task_id: PHASE16_TASK_ID.to_string(),
        request_id: request_id.to_string(),
        session_id: session_id.map(str::to_string),
        delta: delta.to_string(),
        done,
        reset,
        error,
    });
}

#[cfg(test)]
mod control_stop_persistence_tests {
    use super::*;

    fn run_context(session_id: &str) -> Metadata {
        [
            ("project_id".to_string(), "project-pause".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), format!("run-{session_id}")),
        ]
        .into_iter()
        .collect()
    }

    fn seed_running_session(store: &mut SqliteStore, run_context: &Metadata) {
        let session_id = run_context
            .get("session_id")
            .expect("session id should exist");
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            metadata_with_context(
                [("prompt".to_string(), "finish alpha".to_string())]
                    .into_iter()
                    .collect(),
                run_context,
            ),
        )
        .expect("run should start");
        append_message_event_with_metadata(
            store,
            &phase16_task_id(),
            MessageRole::User,
            "finish alpha",
            run_context.clone(),
        )
        .expect("user message should persist");
        for namespace in [
            AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
            AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
        ] {
            store
                .save_read_model(namespace, session_id, 2, "snapshot-before")
                .expect("snapshot should persist");
        }
    }

    fn assistant_metadata(run_context: &Metadata) -> Metadata {
        metadata_with_context(
            [("partial".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            run_context,
        )
    }

    fn recovery_metadata() -> Metadata {
        [
            ("completion".to_string(), "partial".to_string()),
            ("stop_reason".to_string(), "deadline_exceeded".to_string()),
        ]
        .into_iter()
        .collect()
    }

    fn checkpoint() -> AgentTaskStateSnapshot {
        AgentTaskStateSnapshot::capture(&start_agent_loop(
            phase16_task_id(),
            "finish alpha",
            AgentRuntimeConfig::default(),
        ))
    }

    #[test]
    fn paused_run_rolls_back_messages_status_and_snapshot_handoff_together() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = run_context("session-pause");
        seed_running_session(&mut store, &run_context);
        let checkpoint = checkpoint();
        let resources = RunResourceSnapshot::default();

        let error = persist_paused_agent_run_with(
            &mut store,
            &run_context,
            "verified partial answer",
            assistant_metadata(&run_context),
            "deadline_exceeded",
            recovery_metadata(),
            Some(&checkpoint),
            &resources,
            |store, session_id| {
                store.delete_read_model(
                    AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
                    session_id.expect("session should be present"),
                )?;
                Err(StorageError::new("injected snapshot handoff failure"))
            },
        )
        .expect_err("injected handoff failure should abort pause persistence");
        assert!(error.contains("injected snapshot handoff failure"));

        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-pause")
            .expect("events should load");
        assert_eq!(events.len(), 2, "failed pause leaked a partial state");
        for namespace in [
            AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
            AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
        ] {
            assert_eq!(
                store
                    .load_read_model(namespace, "session-pause")
                    .expect("snapshot should load")
                    .map(|model| model.payload),
                Some("snapshot-before".to_string())
            );
        }

        persist_paused_agent_run(
            &mut store,
            &run_context,
            "verified partial answer",
            assistant_metadata(&run_context),
            "deadline_exceeded",
            recovery_metadata(),
            Some(&checkpoint),
            &resources,
        )
        .expect("pause persistence retry should succeed");
        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-pause")
            .expect("events should reload");
        assert_eq!(events.len(), 4);
        assert_eq!(events[2].kind, EventKind::MessageAdded);
        assert_eq!(events[3].summary, "Agent task paused");
        assert!(events[3].metadata.contains_key("recovery_envelope"));
        for namespace in [
            AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
            AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
        ] {
            assert!(store
                .load_read_model(namespace, "session-pause")
                .expect("snapshot absence should load")
                .is_none());
        }
    }

    #[test]
    fn preparation_pause_without_task_state_keeps_recovery_snapshots() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = run_context("session-preparation-pause");
        seed_running_session(&mut store, &run_context);

        persist_paused_agent_run(
            &mut store,
            &run_context,
            "preparation paused",
            assistant_metadata(&run_context),
            "deadline_exceeded",
            recovery_metadata(),
            None,
            &RunResourceSnapshot::default(),
        )
        .expect("preparation pause should persist");

        let events = store
            .list_by_task_and_metadata(
                &phase16_task_id(),
                "session_id",
                "session-preparation-pause",
            )
            .expect("events should load");
        assert_eq!(events.len(), 4);
        assert_eq!(events[3].summary, "Agent task paused");
        for namespace in [
            AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
            AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
        ] {
            assert!(store
                .load_read_model(namespace, "session-preparation-pause")
                .expect("snapshot should load")
                .is_some());
        }
    }
}
