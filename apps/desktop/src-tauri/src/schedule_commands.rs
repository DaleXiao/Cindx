use super::*;

/// Opens a read-only store connection and projects every session's snapshot; the
/// UI polls it on navigation and after each run.
///
/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole read (P2-05).
#[tauri::command]
pub(crate) async fn get_project_session_state(
    app: tauri::AppHandle,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        get_project_session_state_blocking(state)
    })
    .await
    .map_err(|error| format!("project session state failed to join: {error}"))?
}

fn get_project_session_state_blocking(
    state: tauri::State<'_, AppState>,
) -> Result<ProjectSessionState, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    let store = open_app_read_store()?;
    project_session_state_from_store(&config, &store, None).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleQueueProgress {
    pub(crate) status: String,
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) error: Option<String>,
}

/// Reconciles the schedule runs and projects the panel state, so it takes the
/// store lock and may write the schedule configuration. That reconcile already
/// runs concurrently with the background schedule runner, so moving it off the
/// invoke thread adds no new concurrency (P2-05).
#[tauri::command]
pub(crate) async fn get_schedule_state(app: tauri::AppHandle) -> Result<ScheduleStateView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        get_schedule_state_blocking(state)
    })
    .await
    .map_err(|error| format!("schedule state failed to join: {error}"))?
}

fn get_schedule_state_blocking(
    state: tauri::State<'_, AppState>,
) -> Result<ScheduleStateView, String> {
    if let Err(error) = reconcile_schedule_runs(&state) {
        set_schedule_last_error(&state, Some(error));
    }
    schedule_state_view(&state)
}

/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole operation (P2-05).
#[tauri::command]
pub(crate) async fn upsert_schedule(
    app: tauri::AppHandle,
    input: UpsertScheduleInput,
) -> Result<ScheduleStateView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        upsert_schedule_blocking(state, input)
    })
    .await
    .map_err(|error| format!("schedule upsert failed to join: {error}"))?
}

fn upsert_schedule_blocking(
    state: tauri::State<'_, AppState>,
    input: UpsertScheduleInput,
) -> Result<ScheduleStateView, String> {
    let name = input.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("schedule name is empty".to_string());
    }
    if name.chars().count() > SCHEDULE_MAX_NAME_CHARS {
        return Err(format!(
            "schedule name exceeds {SCHEDULE_MAX_NAME_CHARS} characters"
        ));
    }
    let prompt = input.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("schedule prompt is empty".to_string());
    }
    if prompt.chars().count() > SCHEDULE_MAX_PROMPT_CHARS {
        return Err(format!(
            "schedule prompt exceeds {SCHEDULE_MAX_PROMPT_CHARS} characters"
        ));
    }
    let cadence = ScheduleCadence::parse(&input.cadence)
        .ok_or_else(|| "schedule cadence is invalid".to_string())?;
    let timezone = input.timezone.trim().to_string();
    schedule::parse_timezone(&timezone)?;
    let anchor_at_ms = timestamp_ms_from_local(&input.anchor_local, &timezone)?;
    let project_id = input
        .project_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let session_id = input
        .session_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    validate_schedule_target(&state, project_id.as_deref(), session_id.as_deref())?;
    let weekly_days = normalized_weekly_days(anchor_at_ms, &timezone, &input.weekly_days)?;
    let ends_at_ms = input
        .ends_local
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| timestamp_ms_from_local(value, &timezone))
        .transpose()?;
    if ends_at_ms.is_some_and(|ends_at_ms| ends_at_ms < anchor_at_ms) {
        return Err("schedule end must be after its start".to_string());
    }
    let now = current_time_millis();
    let next_run_at_ms = if input.enabled {
        initial_next_run_at_ms(
            anchor_at_ms,
            cadence,
            &timezone,
            &weekly_days,
            ends_at_ms,
            input.catch_up,
            now,
        )?
    } else {
        None
    };
    if input.enabled && next_run_at_ms.is_none() {
        return Err("schedule needs a future occurrence before it can be enabled".to_string());
    }

    let mut config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
    let existing_index = match input.id.as_deref() {
        Some(id) => Some(
            config
                .schedules
                .iter()
                .position(|schedule| schedule.id == id)
                .ok_or_else(|| "schedule not found".to_string())?,
        ),
        None => None,
    };
    if existing_index
        .and_then(|index| config.schedules[index].active_run())
        .is_some()
    {
        return Err("cancel the current scheduled run before editing".to_string());
    }
    let id = existing_index
        .map(|index| config.schedules[index].id.clone())
        .unwrap_or_else(|| {
            unique_config_id(
                "schedule",
                &name,
                &config
                    .schedules
                    .iter()
                    .map(|schedule| schedule.id.clone())
                    .collect::<Vec<_>>(),
            )
        });
    let (created_at_ms, runs, existing_execution_session_id) = existing_index
        .map(|index| {
            (
                config.schedules[index].created_at_ms,
                config.schedules[index].runs.clone(),
                Some(config.schedules[index].execution_session_id.clone()),
            )
        })
        .unwrap_or((now, Vec::new(), None));
    let effort = AgentPolicy::parse_ingress(&input.effort)
        .label()
        .to_string();
    let execution_session_id = ensure_schedule_execution_session(
        &state,
        &id,
        &name,
        &effort,
        project_id.as_deref(),
        session_id.as_deref(),
        existing_execution_session_id.as_deref(),
    )?;
    let record = ScheduleRecord {
        id,
        name,
        project_id,
        session_id,
        execution_session_id,
        prompt,
        effort,
        timezone,
        cadence,
        anchor_at_ms,
        weekly_days,
        ends_at_ms,
        catch_up: input.catch_up,
        enabled: input.enabled,
        next_run_at_ms,
        created_at_ms,
        updated_at_ms: now,
        runs,
    };
    if let Some(index) = existing_index {
        config.schedules[index] = record;
    } else {
        config.schedules.push(record);
    }
    save_schedule_config(&config)?;
    drop(config);
    set_schedule_last_error(&state, None);
    schedule_state_view(&state)
}

/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole operation (P2-05).
#[tauri::command]
pub(crate) async fn set_schedule_enabled(
    app: tauri::AppHandle,
    input: SetScheduleEnabledInput,
) -> Result<ScheduleStateView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        set_schedule_enabled_blocking(state, input)
    })
    .await
    .map_err(|error| format!("schedule enable change failed to join: {error}"))?
}

fn set_schedule_enabled_blocking(
    state: tauri::State<'_, AppState>,
    input: SetScheduleEnabledInput,
) -> Result<ScheduleStateView, String> {
    let _ = reconcile_schedule_runs(&state);
    let now = current_time_millis();
    let mut config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
    let schedule_index = config
        .schedules
        .iter()
        .position(|schedule| schedule.id == input.schedule_id)
        .ok_or_else(|| "schedule not found".to_string())?;
    let schedule = &config.schedules[schedule_index];
    let execution_session_id = ensure_schedule_execution_session(
        &state,
        &schedule.id,
        &schedule.name,
        &schedule.effort,
        schedule.project_id.as_deref(),
        schedule.session_id.as_deref(),
        Some(schedule.execution_session_id.as_str()),
    )?;
    let next_run_at_ms = if input.enabled {
        initial_next_run_at_ms(
            schedule.anchor_at_ms,
            schedule.cadence,
            &schedule.timezone,
            &schedule.weekly_days,
            schedule.ends_at_ms,
            schedule.catch_up,
            now,
        )?
    } else {
        None
    };
    if input.enabled && next_run_at_ms.is_none() {
        return Err("schedule needs a future occurrence before it can be enabled".to_string());
    }
    let schedule = &mut config.schedules[schedule_index];
    schedule.execution_session_id = execution_session_id;
    schedule.enabled = input.enabled;
    schedule.next_run_at_ms = next_run_at_ms;
    schedule.updated_at_ms = now;
    save_schedule_config(&config)?;
    drop(config);
    set_schedule_last_error(&state, None);
    schedule_state_view(&state)
}

/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole operation (P2-05).
#[tauri::command]
pub(crate) async fn delete_schedule(
    app: tauri::AppHandle,
    input: ScheduleActionInput,
) -> Result<ScheduleStateView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        delete_schedule_blocking(state, input)
    })
    .await
    .map_err(|error| format!("schedule deletion failed to join: {error}"))?
}

fn delete_schedule_blocking(
    state: tauri::State<'_, AppState>,
    input: ScheduleActionInput,
) -> Result<ScheduleStateView, String> {
    let _ = reconcile_schedule_runs(&state);
    let mut config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
    let Some(index) = config
        .schedules
        .iter()
        .position(|schedule| schedule.id == input.schedule_id)
    else {
        return Err("schedule not found".to_string());
    };
    if config.schedules[index].active_run().is_some() {
        return Err("cancel the current scheduled run before deleting".to_string());
    }
    config.schedules.remove(index);
    save_schedule_config(&config)?;
    drop(config);
    set_schedule_last_error(&state, None);
    schedule_state_view(&state)
}

/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole operation (P2-05).
#[tauri::command]
pub(crate) async fn run_schedule_now(
    app: tauri::AppHandle,
    input: ScheduleActionInput,
) -> Result<ScheduleStateView, String> {
    tauri::async_runtime::spawn_blocking(move || run_schedule_now_blocking(app, input))
        .await
        .map_err(|error| format!("immediate schedule run failed to join: {error}"))?
}

fn run_schedule_now_blocking(
    app: tauri::AppHandle,
    input: ScheduleActionInput,
) -> Result<ScheduleStateView, String> {
    let state = app.state::<AppState>();
    let session_id = trigger_schedule_run(
        &state,
        &input.schedule_id,
        "manual",
        current_time_millis(),
        false,
    )?;
    spawn_schedule_dispatch(app.clone(), session_id);
    schedule_state_view(&state)
}

/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole operation (P2-05).
#[tauri::command]
pub(crate) async fn cancel_schedule_run(
    app: tauri::AppHandle,
    input: ScheduleActionInput,
) -> Result<ScheduleStateView, String> {
    tauri::async_runtime::spawn_blocking(move || cancel_schedule_run_blocking(app, input))
        .await
        .map_err(|error| format!("schedule run cancellation failed to join: {error}"))?
}

fn cancel_schedule_run_blocking(
    app: tauri::AppHandle,
    input: ScheduleActionInput,
) -> Result<ScheduleStateView, String> {
    let state = app.state::<AppState>();
    reconcile_schedule_runs(&state)?;
    let (session_id, queue_id, status) = {
        let config = state
            .schedule_config
            .lock()
            .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
        let schedule = config
            .schedules
            .iter()
            .find(|schedule| schedule.id == input.schedule_id)
            .ok_or_else(|| "schedule not found".to_string())?;
        let run = schedule
            .active_run()
            .ok_or_else(|| "schedule has no active run".to_string())?;
        (
            schedule.execution_session_id.clone(),
            run.queue_id
                .clone()
                .ok_or_else(|| "scheduled run is still preparing".to_string())?,
            run.status.clone(),
        )
    };

    if status == "queued" {
        delete_queue_message_by_id(&state, &session_id, &queue_id)?;
    } else {
        let events = {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            agent_events_for_session(&store, &phase16_task_id(), Some(&session_id))
                .map_err(|error| error.to_string())?
        };
        if latest_unfinished_agent_queue_id(&events).as_deref() != Some(queue_id.as_str()) {
            return Err("the scheduled run is no longer the active agent run".to_string());
        }
        // Call the shared blocking path, not the command: `cancel_agent_task` is
        // async, and this body is already on a blocking task.
        cancel_agent_task_blocking(&app, state.clone(), &session_id)?;
    }

    let now = current_time_millis();
    let mut config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
    if let Some(run) = config
        .schedules
        .iter_mut()
        .find(|schedule| schedule.id == input.schedule_id)
        .and_then(|schedule| {
            schedule
                .runs
                .iter_mut()
                .rev()
                .find(|run| run.queue_id.as_deref() == Some(queue_id.as_str()))
        })
    {
        run.status = "cancelled".to_string();
        run.finished_at_ms = Some(now);
        run.error = None;
    }
    save_schedule_config(&config)?;
    drop(config);
    schedule_state_view(&state)
}

pub(crate) fn validate_schedule_target(
    state: &tauri::State<'_, AppState>,
    project_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<(), String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    if let Some(project_id) = project_id {
        if !config
            .projects
            .iter()
            .any(|project| project.id == project_id)
        {
            return Err("schedule project not found".to_string());
        }
    }
    if let Some(session_id) = session_id {
        let project_id =
            project_id.ok_or_else(|| "choose a project before linking a task".to_string())?;
        let Some(session) = config
            .sessions
            .iter()
            .find(|session| session.id == session_id)
        else {
            return Err("schedule session not found".to_string());
        };
        if session.project_id != project_id {
            return Err("schedule session does not belong to the selected project".to_string());
        }
        if session.archived_at_ms.is_some() {
            return Err("schedule session is archived".to_string());
        }
    }
    Ok(())
}

pub(crate) fn ensure_schedule_execution_session(
    state: &tauri::State<'_, AppState>,
    _schedule_id: &str,
    schedule_name: &str,
    effort: &str,
    project_id: Option<&str>,
    session_id: Option<&str>,
    existing_execution_session_id: Option<&str>,
) -> Result<String, String> {
    validate_schedule_target(state, project_id, session_id)?;
    if let Some(session_id) = session_id {
        return Ok(session_id.to_string());
    }

    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let execution_project_id = project_id
        .map(str::to_string)
        .or_else(|| {
            candidate
                .projects
                .iter()
                .any(|project| project.id == candidate.active_project_id)
                .then(|| candidate.active_project_id.clone())
        })
        .or_else(|| candidate.projects.first().map(|project| project.id.clone()))
        .ok_or_else(|| "create a project before enabling a standalone schedule".to_string())?;

    if let Some(session) = existing_execution_session_id.and_then(|session_id| {
        candidate.sessions.iter_mut().find(|session| {
            session.id == session_id
                && session.archived_at_ms.is_none()
                && is_schedule_execution_session(session)
        })
    }) {
        let next_name = format!("{} · Schedule", schedule_name.trim());
        let next_effort = AgentPolicy::parse_ingress(effort).label().to_string();
        let execution_session_id = session.id.clone();
        let mut changed = false;
        if session.name != next_name
            || session.effort != next_effort
            || session.project_id != execution_project_id
        {
            session.name = next_name;
            session.effort = next_effort;
            session.project_id = execution_project_id.clone();
            session.updated_at_ms = current_time_millis();
            changed = true;
        }
        if changed {
            commit_project_session_config(&mut config, candidate)
                .map_err(|error| error.to_string())?;
        }
        return Ok(execution_session_id);
    }

    let execution_session_id = new_session_id();
    let now = current_time_millis();
    candidate.sessions.push(SessionRecord {
        id: execution_session_id.clone(),
        project_id: execution_project_id,
        name: format!("{} · Schedule", schedule_name.trim()),
        title_state: SessionTitleState::Manual,
        detail: SCHEDULE_EXECUTION_SESSION_DETAIL.to_string(),
        effort: AgentPolicy::parse_ingress(effort).label().to_string(),
        agent_model: String::new(),
        seen_event_sequence: 0,
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    Ok(execution_session_id)
}

pub(crate) fn schedule_state_view(
    state: &tauri::State<'_, AppState>,
) -> Result<ScheduleStateView, String> {
    let config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?
        .clone();
    let project_sessions = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    let mut schedules = config
        .schedules
        .into_iter()
        .map(|schedule| {
            let project_name = schedule.project_id.as_deref().map_or_else(
                || "No project".to_string(),
                |project_id| {
                    project_sessions
                        .projects
                        .iter()
                        .find(|project| project.id == project_id)
                        .map(|project| project.name.clone())
                        .unwrap_or_else(|| "Missing project".to_string())
                },
            );
            let session_name = schedule.session_id.as_deref().map_or_else(
                || "Standalone".to_string(),
                |session_id| {
                    project_sessions
                        .sessions
                        .iter()
                        .find(|session| session.id == session_id)
                        .map(|session| session.name.clone())
                        .unwrap_or_else(|| "Missing task".to_string())
                },
            );
            ScheduleView {
                id: schedule.id,
                name: schedule.name,
                project_id: schedule.project_id,
                project_name,
                session_id: schedule.session_id,
                session_name,
                prompt: schedule.prompt,
                effort: schedule.effort,
                timezone: schedule.timezone,
                cadence: schedule.cadence.label().to_string(),
                anchor_at_ms: schedule.anchor_at_ms,
                weekly_days: schedule.weekly_days,
                ends_at_ms: schedule.ends_at_ms,
                catch_up: schedule.catch_up,
                enabled: schedule.enabled,
                next_run_at_ms: schedule.next_run_at_ms,
                created_at_ms: schedule.created_at_ms,
                updated_at_ms: schedule.updated_at_ms,
                runs: schedule.runs,
            }
        })
        .collect::<Vec<_>>();
    schedules.sort_by(|left, right| {
        right
            .enabled
            .cmp(&left.enabled)
            .then_with(|| {
                left.next_run_at_ms
                    .unwrap_or(u64::MAX)
                    .cmp(&right.next_run_at_ms.unwrap_or(u64::MAX))
            })
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
    });
    let last_error = state
        .schedule_last_error
        .lock()
        .map_err(|error| format!("schedule error lock poisoned: {error}"))?
        .clone();
    Ok(ScheduleStateView {
        schedules,
        last_error,
    })
}

pub(crate) fn set_schedule_last_error(state: &tauri::State<'_, AppState>, error: Option<String>) {
    if let Ok(mut last_error) = state.schedule_last_error.lock() {
        *last_error = error;
    }
}

pub(crate) fn save_schedule_config(config: &ScheduleConfig) -> Result<(), String> {
    schedule::save(&schedule_config_path(), config)
        .map_err(|error| format!("failed to save schedules: {error}"))
}

pub(crate) fn schedule_queue_progress(
    events: &[Event],
    queue_id: &str,
) -> Option<ScheduleQueueProgress> {
    let mut progress = None;
    for event in events {
        if event.metadata.get("queue_id").map(String::as_str) != Some(queue_id) {
            continue;
        }
        if let Some(action) = event.metadata.get("queue_action").map(String::as_str) {
            match action {
                "enqueue" | "restore" => {
                    progress = Some(ScheduleQueueProgress {
                        status: "queued".to_string(),
                        started_at_ms: None,
                        finished_at_ms: None,
                        error: None,
                    });
                }
                "start" => {
                    let started_at_ms = progress
                        .as_ref()
                        .and_then(|progress| progress.started_at_ms)
                        .or(Some(event.timestamp_ms));
                    progress = Some(ScheduleQueueProgress {
                        status: "running".to_string(),
                        started_at_ms,
                        finished_at_ms: None,
                        error: None,
                    });
                }
                "delete" => {
                    progress = Some(ScheduleQueueProgress {
                        status: "cancelled".to_string(),
                        started_at_ms: progress
                            .as_ref()
                            .and_then(|progress| progress.started_at_ms),
                        finished_at_ms: Some(event.timestamp_ms),
                        error: None,
                    });
                }
                _ => {}
            }
        }
        let next_status = AgentRunEvent::from_event(event).map(AgentRunEvent::status);
        if let Some(status) = next_status {
            let terminal = status == AgentRunStatus::Paused || status.is_terminal();
            let started_at_ms = progress
                .as_ref()
                .and_then(|progress| progress.started_at_ms)
                .or((!terminal).then_some(event.timestamp_ms));
            progress = Some(ScheduleQueueProgress {
                status: status.label().to_string(),
                started_at_ms,
                finished_at_ms: terminal.then_some(event.timestamp_ms),
                error: if status == AgentRunStatus::Failed {
                    event
                        .metadata
                        .get("error")
                        .cloned()
                        .or_else(|| Some("Agent task failed".to_string()))
                } else {
                    None
                },
            });
        }
    }
    progress
}

pub(crate) fn latest_unfinished_agent_queue_id(events: &[Event]) -> Option<String> {
    let mut active = None;
    for event in events {
        if is_agent_run_start_event(event) {
            active = event.metadata.get("queue_id").cloned();
            continue;
        }
        if matches!(
            event.summary.as_str(),
            "Agent task paused"
                | "Agent task completed"
                | "Agent task failed"
                | "Agent task cancelled"
        ) && event.metadata.get("queue_id") == active.as_ref()
        {
            active = None;
        }
    }
    active
}

pub(crate) fn reconcile_schedule_runs(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let active_runs = {
        let config = state
            .schedule_config
            .lock()
            .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
        config
            .schedules
            .iter()
            .filter_map(|schedule| {
                schedule.active_run().map(|run| {
                    (
                        schedule.id.clone(),
                        schedule.execution_session_id.clone(),
                        run.id.clone(),
                        run.queue_id.clone(),
                        run.queued_at_ms,
                    )
                })
            })
            .collect::<Vec<_>>()
    };
    if active_runs.is_empty() {
        return Ok(());
    }

    let mut events_by_session = BTreeMap::new();
    {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        for (_, session_id, _, _, _) in &active_runs {
            if events_by_session.contains_key(session_id) {
                continue;
            }
            events_by_session.insert(
                session_id.clone(),
                agent_events_for_session(&store, &phase16_task_id(), Some(session_id))
                    .map_err(|error| error.to_string())?,
            );
        }
    }

    let now = current_time_millis();
    let mut changed = false;
    let mut config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
    for (schedule_id, session_id, run_id, queue_id, queued_at_ms) in active_runs {
        let Some(schedule) = config
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == schedule_id)
        else {
            continue;
        };
        let Some(run) = schedule.runs.iter_mut().find(|run| run.id == run_id) else {
            continue;
        };
        let progress = queue_id.as_deref().and_then(|queue_id| {
            events_by_session
                .get(&session_id)
                .and_then(|events| schedule_queue_progress(events, queue_id))
        });
        if let Some(progress) = progress {
            if run.status != progress.status
                || run.started_at_ms != progress.started_at_ms
                || run.finished_at_ms != progress.finished_at_ms
                || run.error != progress.error
            {
                run.status = progress.status;
                run.started_at_ms = progress.started_at_ms;
                run.finished_at_ms = progress.finished_at_ms;
                run.error = progress.error;
                schedule.updated_at_ms = now;
                changed = true;
            }
        } else if run.status == "preparing"
            && queued_at_ms.is_some_and(|queued_at_ms| now.saturating_sub(queued_at_ms) > 60_000)
        {
            run.status = "failed".to_string();
            run.finished_at_ms = Some(now);
            run.error = Some("Scheduled queue entry was not created".to_string());
            schedule.updated_at_ms = now;
            changed = true;
        }
    }
    if changed {
        save_schedule_config(&config)?;
    }
    Ok(())
}

pub(crate) fn trigger_schedule_run(
    state: &tauri::State<'_, AppState>,
    schedule_id: &str,
    source: &str,
    scheduled_for_ms: u64,
    advance_schedule: bool,
) -> Result<String, String> {
    let now = current_time_millis();
    let mut config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
    let schedule = config
        .schedules
        .iter_mut()
        .find(|schedule| schedule.id == schedule_id)
        .ok_or_else(|| "schedule not found".to_string())?;
    if schedule.active_run().is_some() {
        return Err("schedule already has an active run".to_string());
    }
    let session_id = ensure_schedule_execution_session(
        state,
        &schedule.id,
        &schedule.name,
        &schedule.effort,
        schedule.project_id.as_deref(),
        schedule.session_id.as_deref(),
        Some(schedule.execution_session_id.as_str()),
    )?;
    schedule.execution_session_id = session_id.clone();
    let queue_input = QueueAgentMessageInput {
        session_id: session_id.clone(),
        prompt: schedule.prompt.clone(),
        current_time: normalized_current_time_context(""),
        queue_id: None,
        effort: schedule.effort.clone(),
        attachments: Vec::new(),
        // Scheduled runs are dispatched unattended; the plan-then-confirm gate
        // is an interactive Composer feature and never applies here.
        plan_mode: false,
    };
    let run_id = unique_id("schedule-run");
    let queue_result = enqueue_agent_message_inner(state, queue_input);
    let (queue_id, run_status, run_error) = match queue_result {
        Ok((_, queue_id)) => (Some(queue_id), "queued".to_string(), None),
        Err(error) => (None, "failed".to_string(), Some(error)),
    };
    schedule.push_run(ScheduleRunRecord {
        id: run_id,
        queue_id: queue_id.clone(),
        source: source.to_string(),
        scheduled_for_ms,
        queued_at_ms: queue_id.as_ref().map(|_| now),
        dispatch_attempts: 0,
        last_dispatch_at_ms: None,
        started_at_ms: None,
        finished_at_ms: queue_id.is_none().then_some(now),
        status: run_status,
        error: run_error.clone(),
    });
    if advance_schedule {
        if schedule.cadence.recurring() {
            schedule.next_run_at_ms = next_occurrence_after_ms(
                schedule.anchor_at_ms,
                schedule.cadence,
                &schedule.timezone,
                &schedule.weekly_days,
                schedule.ends_at_ms,
                now,
            )?;
            if schedule.next_run_at_ms.is_none() {
                schedule.enabled = false;
            }
        } else {
            schedule.enabled = false;
            schedule.next_run_at_ms = None;
        }
    }
    schedule.updated_at_ms = now;
    save_schedule_config(&config)?;
    if let Some(error) = run_error {
        set_schedule_last_error(state, Some(error.clone()));
        return Err(error);
    }
    set_schedule_last_error(state, None);
    Ok(session_id)
}

pub(crate) fn delete_queue_message_by_id(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
    queue_id: &str,
) -> Result<(), String> {
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
    let run_context = project_session_metadata_for_session(state, Some(session_id))?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let events = agent_events_for_session(&store, &phase16_task_id(), Some(session_id))
        .map_err(|error| error.to_string())?;
    let Some(queued) = pending_queued_agent_messages(&events, session_id)
        .into_iter()
        .find(|message| message.view.id == queue_id)
    else {
        return Ok(());
    };
    append_agent_queue_event(
        &mut store,
        &run_context,
        "delete",
        &queued.view.id,
        &queued.view.mode,
        queued.view.created_at_ms,
        None,
    )
}

pub(crate) fn poll_schedules(app: &tauri::AppHandle) -> Result<Vec<String>, String> {
    let state = app.state::<AppState>();
    reconcile_schedule_runs(&state)?;
    let now = current_time_millis();
    let mut due = Vec::new();
    let mut changed = false;
    {
        let mut config = state
            .schedule_config
            .lock()
            .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
        for schedule in &mut config.schedules {
            if !schedule.enabled || schedule.active_run().is_some() {
                continue;
            }
            if schedule
                .ends_at_ms
                .is_some_and(|ends_at_ms| now > ends_at_ms)
            {
                schedule.enabled = false;
                schedule.next_run_at_ms = None;
                schedule.updated_at_ms = now;
                changed = true;
                continue;
            }
            let Some(next_run_at_ms) = schedule.next_run_at_ms else {
                continue;
            };
            if next_run_at_ms > now {
                continue;
            }
            if !schedule.catch_up && now.saturating_sub(next_run_at_ms) > SCHEDULE_MISSED_GRACE_MS {
                schedule.push_run(ScheduleRunRecord {
                    id: unique_id("schedule-run"),
                    queue_id: None,
                    source: "scheduled".to_string(),
                    scheduled_for_ms: next_run_at_ms,
                    queued_at_ms: None,
                    dispatch_attempts: 0,
                    last_dispatch_at_ms: None,
                    started_at_ms: None,
                    finished_at_ms: Some(now),
                    status: "skipped".to_string(),
                    error: Some("Missed while Cindx was not running".to_string()),
                });
                if schedule.cadence.recurring() {
                    schedule.next_run_at_ms = next_occurrence_after_ms(
                        schedule.anchor_at_ms,
                        schedule.cadence,
                        &schedule.timezone,
                        &schedule.weekly_days,
                        schedule.ends_at_ms,
                        now,
                    )?;
                    if schedule.next_run_at_ms.is_none() {
                        schedule.enabled = false;
                    }
                } else {
                    schedule.enabled = false;
                    schedule.next_run_at_ms = None;
                }
                schedule.updated_at_ms = now;
                changed = true;
            } else {
                due.push((schedule.id.clone(), next_run_at_ms));
            }
        }
        if changed {
            save_schedule_config(&config)?;
        }
    }

    let mut sessions = BTreeSet::new();
    for (schedule_id, scheduled_for_ms) in due {
        match trigger_schedule_run(&state, &schedule_id, "scheduled", scheduled_for_ms, true) {
            Ok(session_id) => {
                sessions.insert(session_id);
            }
            Err(error) => set_schedule_last_error(&state, Some(error)),
        }
    }
    let config = state
        .schedule_config
        .lock()
        .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
    for schedule in &config.schedules {
        if schedule
            .active_run()
            .is_some_and(|run| run.status == "queued")
        {
            sessions.insert(schedule.execution_session_id.clone());
        }
    }
    Ok(sessions.into_iter().collect())
}

pub(crate) fn spawn_schedule_dispatch(app: tauri::AppHandle, session_id: String) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = dispatch_scheduled_session(&app, &session_id) {
            let state = app.state::<AppState>();
            set_schedule_last_error(&state, Some(error));
        }
    });
}

pub(crate) fn dispatch_scheduled_session(
    app: &tauri::AppHandle,
    session_id: &str,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    reconcile_schedule_runs(&state)?;
    let now = current_time_millis();
    let first_queue_id = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = agent_events_for_session(&store, &phase16_task_id(), Some(session_id))
            .map_err(|error| error.to_string())?;
        let agent = agent_state_for_session(&store, None, Some(session_id))
            .map_err(|error| error.to_string())?;
        if matches!(agent.status.as_str(), "running" | "waiting_for_permission")
            || !agent.pending_approvals.is_empty()
        {
            return Ok(());
        }
        let Some(queue_id) = pending_queued_agent_messages(&events, session_id)
            .first()
            .map(|queued| queued.view.id.clone())
        else {
            return Ok(());
        };
        queue_id
    };
    let (schedule_id, queue_id) = {
        let config = state
            .schedule_config
            .lock()
            .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
        let Some((schedule, run)) = config.schedules.iter().find_map(|schedule| {
            (schedule.execution_session_id == session_id).then(|| {
                schedule
                    .active_run()
                    .filter(|run| {
                        run.status == "queued"
                            && run.queue_id.as_deref() == Some(first_queue_id.as_str())
                    })
                    .map(|run| (schedule, run))
            })?
        }) else {
            return Ok(());
        };
        if run.dispatch_attempts >= SCHEDULE_MAX_DISPATCH_ATTEMPTS
            || run
                .last_dispatch_at_ms
                .is_some_and(|last| now.saturating_sub(last) < SCHEDULE_DISPATCH_RETRY_MS)
        {
            return Ok(());
        }
        (
            schedule.id.clone(),
            run.queue_id
                .clone()
                .ok_or_else(|| "scheduled queue id is missing".to_string())?,
        )
    };

    let Some(dispatch_lease) = begin_queue_dispatch(&state, session_id)? else {
        return Ok(());
    };
    let result = (|| {
        let mut config = state
            .schedule_config
            .lock()
            .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
        let run = config
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == schedule_id)
            .and_then(|schedule| {
                schedule
                    .runs
                    .iter_mut()
                    .rev()
                    .find(|run| run.queue_id.as_deref() == Some(queue_id.as_str()))
            })
            .ok_or_else(|| "scheduled run not found".to_string())?;
        run.dispatch_attempts = run.dispatch_attempts.saturating_add(1);
        run.last_dispatch_at_ms = Some(now);
        save_schedule_config(&config)?;
        drop(config);
        run_next_queued_agent_message_blocking_inner(
            app,
            state.clone(),
            SessionActionInput {
                session_id: session_id.to_string(),
            },
        )
    })();
    drop(dispatch_lease);
    reconcile_schedule_runs(&state)?;

    let exhausted = {
        let config = state
            .schedule_config
            .lock()
            .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
        config
            .schedules
            .iter()
            .find(|schedule| schedule.id == schedule_id)
            .and_then(|schedule| {
                schedule
                    .runs
                    .iter()
                    .rev()
                    .find(|run| run.queue_id.as_deref() == Some(queue_id.as_str()))
            })
            .is_some_and(|run| {
                run.status == "queued" && run.dispatch_attempts >= SCHEDULE_MAX_DISPATCH_ATTEMPTS
            })
    };
    if exhausted {
        delete_queue_message_by_id(&state, session_id, &queue_id)?;
        let mut config = state
            .schedule_config
            .lock()
            .map_err(|error| format!("schedule config lock poisoned: {error}"))?;
        if let Some(run) = config
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == schedule_id)
            .and_then(|schedule| {
                schedule
                    .runs
                    .iter_mut()
                    .rev()
                    .find(|run| run.queue_id.as_deref() == Some(queue_id.as_str()))
            })
        {
            run.status = "failed".to_string();
            run.finished_at_ms = Some(current_time_millis());
            run.error = Some("Agent could not start after three attempts".to_string());
        }
        save_schedule_config(&config)?;
    }
    result.map(|_| ())
}

pub(crate) fn start_schedule_runner(app: tauri::AppHandle) {
    let _ = std::thread::Builder::new()
        .name("cindx-schedule-runner".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            loop {
                match poll_schedules(&app) {
                    Ok(session_ids) => {
                        for session_id in session_ids {
                            spawn_schedule_dispatch(app.clone(), session_id);
                        }
                    }
                    Err(error) => {
                        let state = app.state::<AppState>();
                        set_schedule_last_error(&state, Some(error.clone()));
                        append_startup_log(&format!("schedule runner failed: {error}"));
                    }
                }
                std::thread::sleep(SCHEDULE_POLL_INTERVAL);
            }
        });
}
