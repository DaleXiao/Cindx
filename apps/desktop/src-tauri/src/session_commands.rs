//! Session lifecycle IPC: create, rename, effort and model selection, title
//! generation, fork, archive, restore, delete, select, and activity
//! acknowledgement.
//!
//! Split out of `project_commands` so that module owns projects and the workspace
//! root while this one owns sessions. Both call `project_lifecycle_runtime` for the
//! published-delete cleanup, so neither command module depends on the other.

use super::*;
use crate::desktop_event_sink::DesktopEventSink;

pub(crate) fn acknowledged_event_sequence(
    latest_sequence: u64,
    through_sequence: Option<u64>,
) -> u64 {
    through_sequence
        .unwrap_or(latest_sequence)
        .min(latest_sequence)
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn create_session(
    app: tauri::AppHandle,
    input: CreateSessionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        create_session_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session creation failed to join: {error}"))?
}

fn create_session_blocking(
    state: tauri::State<'_, AppState>,
    input: CreateSessionInput,
) -> Result<ProjectSessionState, String> {
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return project_session_state_with_error(&state, "session name is empty");
    }
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let project_id = input
        .project_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| config.active_project_id.clone());
    if !config
        .projects
        .iter()
        .any(|project| project.id == project_id)
    {
        return Ok(project_session_state(
            &config,
            Some("project not found for session".to_string()),
        ));
    }
    let mut candidate = config.clone();
    let session_id = new_session_id();
    let now = current_time_millis();
    candidate.sessions.push(SessionRecord {
        id: session_id.clone(),
        project_id: project_id.clone(),
        title_state: SessionTitleState::parse(None, is_automatic_session_name(&name)),
        name,
        detail: "timeline + chat".to_string(),
        effort: default_agent_effort(),
        agent_model: String::new(),
        seen_event_sequence: 0,
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    candidate.active_project_id = project_id;
    candidate.active_session_id = session_id;
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;

    Ok(project_session_state(&config, None))
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn rename_session(
    app: tauri::AppHandle,
    input: RenameSessionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        rename_session_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session rename failed to join: {error}"))?
}

fn rename_session_blocking(
    state: tauri::State<'_, AppState>,
    input: RenameSessionInput,
) -> Result<ProjectSessionState, String> {
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return project_session_state_with_error(&state, "session name is empty");
    }
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let now = current_time_millis();
    let project_id = {
        let Some(session) = candidate
            .sessions
            .iter_mut()
            .find(|session| session.id == input.session_id)
        else {
            return Ok(project_session_state(
                &config,
                Some("session not found".to_string()),
            ));
        };
        session.name = name;
        session.title_state = SessionTitleState::Manual;
        session.updated_at_ms = now;
        session.project_id.clone()
    };
    if let Some(project) = candidate
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
    {
        project.updated_at_ms = now;
    }
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn set_session_effort(
    app: tauri::AppHandle,
    input: SessionEffortInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        set_session_effort_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session effort change failed to join: {error}"))?
}

fn set_session_effort_blocking(
    state: tauri::State<'_, AppState>,
    input: SessionEffortInput,
) -> Result<ProjectSessionState, String> {
    let effort = AgentPolicy::parse_ingress(&input.effort)
        .label()
        .to_string();
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    if !update_session_effort(&mut candidate, &input.session_id, &effort) {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    }
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

pub(crate) fn update_session_effort(
    config: &mut ProjectSessionConfig,
    session_id: &str,
    effort: &str,
) -> bool {
    let Some(session) = config
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id && session.archived_at_ms.is_none())
    else {
        return false;
    };
    session.effort = effort.to_string();
    true
}

pub(crate) fn update_session_model(
    config: &mut ProjectSessionConfig,
    session_id: &str,
    agent_model: &str,
) -> bool {
    let Some(session) = config
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id && session.archived_at_ms.is_none())
    else {
        return false;
    };
    session.agent_model = agent_model.to_string();
    true
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn set_session_model(
    app: tauri::AppHandle,
    input: SessionModelInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        set_session_model_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session model change failed to join: {error}"))?
}

fn set_session_model_blocking(
    state: tauri::State<'_, AppState>,
    input: SessionModelInput,
) -> Result<ProjectSessionState, String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    if !update_session_model(&mut candidate, &input.session_id, &input.agent_model) {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    }
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) async fn generate_session_title(
    app: tauri::AppHandle,
    input: GenerateSessionTitleInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let prompt = input.prompt.trim();
        let answer = input.answer.trim();
        if prompt.is_empty() || answer.is_empty() {
            return project_session_state_with_error(
                &state,
                "session title requires the first user and assistant messages",
            );
        }
        let title_turns = vec![SessionTitleTurn {
            prompt: prompt.to_string(),
            answer: answer.to_string(),
        }];
        let (expected_title, expected_title_state, expected_updated_at_ms) = {
            let mut config = state
                .project_session_config
                .lock()
                .map_err(|error| format!("project session config lock poisoned: {error}"))?;
            let mut candidate = config.clone();
            let (project_id, expected_title, expected_title_state, expected_updated_at_ms) = {
                let Some(session) = candidate.sessions.iter_mut().find(|session| {
                    session.id == input.session_id && session.archived_at_ms.is_none()
                }) else {
                    return Ok(project_session_state(
                        &config,
                        Some("session not found".to_string()),
                    ));
                };
                if !session_title_refinement_needed(
                    session.title_state,
                    &session.name,
                    &title_turns,
                ) {
                    return Ok(project_session_state(&config, None));
                }
                let now = current_time_millis().max(session.updated_at_ms.saturating_add(1));
                session.updated_at_ms = now;
                (
                    session.project_id.clone(),
                    session.name.clone(),
                    session.title_state,
                    now,
                )
            };
            if let Some(project) = candidate
                .projects
                .iter_mut()
                .find(|project| project.id == project_id)
            {
                project.updated_at_ms = expected_updated_at_ms;
            }
            commit_project_session_config(&mut config, candidate)
                .map_err(|error| error.to_string())?;
            (expected_title, expected_title_state, expected_updated_at_ms)
        };
        let provider_config = clone_provider_config(&state)?;
        if !provider_config.is_ready() {
            let config = state
                .project_session_config
                .lock()
                .map_err(|error| format!("project session config lock poisoned: {error}"))?;
            return Ok(project_session_state(&config, None));
        }
        let title = match semantic_session_title(&provider_config, &title_turns) {
            Ok(title) => title,
            Err(error) => {
                eprintln!(
                    "session title generation failed for {}: {error}",
                    input.session_id
                );
                let config = state
                    .project_session_config
                    .lock()
                    .map_err(|error| format!("project session config lock poisoned: {error}"))?;
                return Ok(project_session_state(&config, None));
            }
        };

        let now = current_time_millis();
        let mut config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let mut candidate = config.clone();
        let project_id =
            {
                let Some(session) = candidate.sessions.iter_mut().find(|session| {
                    session.id == input.session_id && session.archived_at_ms.is_none()
                }) else {
                    return Ok(project_session_state(&config, None));
                };
                if session.name != expected_title
                    || session.title_state != expected_title_state
                    || session.title_state == SessionTitleState::Manual
                    || session.updated_at_ms != expected_updated_at_ms
                {
                    return Ok(project_session_state(&config, None));
                }
                session.name = title;
                session.title_state = SessionTitleState::Automatic;
                session.updated_at_ms = now;
                session.project_id.clone()
            };
        if let Some(project) = candidate
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        {
            project.updated_at_ms = now;
        }
        commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
        let next_state = project_session_state(&config, None);
        drop(config);
        app.emit_session_title_updated(input.session_id);
        Ok(next_state)
    })
    .await
    .map_err(|error| format!("session title task failed: {error}"))?
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn fork_session(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        fork_session_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session fork failed to join: {error}"))?
}

fn fork_session_blocking(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let (source, project, fork) = {
        let config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let Some(source) = config
            .sessions
            .iter()
            .find(|session| session.id == input.session_id)
            .cloned()
        else {
            return Ok(project_session_state(
                &config,
                Some("session not found".to_string()),
            ));
        };
        let Some(project) = config
            .projects
            .iter()
            .find(|project| project.id == source.project_id)
            .cloned()
        else {
            return Ok(project_session_state(
                &config,
                Some("session project not found".to_string()),
            ));
        };
        let name = unique_fork_name(&config, &source);
        let id = new_session_id();
        let now = current_time_millis();
        let fork = SessionRecord {
            id: id.clone(),
            project_id: source.project_id.clone(),
            name,
            title_state: SessionTitleState::Manual,
            detail: format!("Fork of {}", source.name),
            effort: default_agent_effort(),
            agent_model: String::new(),
            seen_event_sequence: 0,
            created_at_ms: now,
            updated_at_ms: now,
            archived_at_ms: None,
        };
        (source, project, fork)
    };

    let project_root = PathBuf::from(&project.root);
    let source_attachment_dir = project_root
        .join(".cindx")
        .join("attachments")
        .join(slug_label(&source.id));
    let target_attachment_dir = project_root
        .join(".cindx")
        .join("attachments")
        .join(slug_label(&fork.id));
    let mut events = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        load_forkable_session_events(&store, &source.id).map_err(|error| error.to_string())?
    };
    let journal = ProjectLifecycleJournal::fork(
        project.id.clone(),
        project_root.clone(),
        source.id.clone(),
        fork.id.clone(),
    );
    persist_project_lifecycle_journal(&journal)?;
    let prepared = clone_fork_attachments_and_rewrite_events(
        &mut events,
        &source_attachment_dir,
        &target_attachment_dir,
    )
    .and_then(|()| {
        let fork_metadata = [
            ("session_id".to_string(), fork.id.clone()),
            ("session_name".to_string(), fork.name.clone()),
            ("project_id".to_string(), project.id.clone()),
            ("project_name".to_string(), project.name.clone()),
            ("project_root".to_string(), project.root.clone()),
            ("forked_from_session_id".to_string(), source.id.clone()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        persist_fork_events(&mut store, &events, &fork_metadata, &source.id)
            .map_err(|error| error.to_string())
    });
    if let Err(error) = prepared {
        return abort_unpublished_fork(&state, &journal, &project_root, &fork.id, error);
    }

    let published = {
        let _lifecycle = state
            .session_lifecycle_gate
            .lock()
            .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
        let mut config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let source_is_current = config
            .sessions
            .iter()
            .any(|session| session.id == source.id && session.project_id == project.id);
        let project_is_current = config
            .projects
            .iter()
            .any(|candidate| candidate.id == project.id && candidate.root == project.root);
        if !source_is_current || !project_is_current {
            false
        } else {
            let mut candidate = config.clone();
            candidate.sessions.push(fork.clone());
            candidate.active_project_id = source.project_id.clone();
            candidate.active_session_id = fork.id.clone();
            if let Err(error) = commit_project_session_config(&mut config, candidate) {
                drop(config);
                drop(_lifecycle);
                return abort_unpublished_fork(
                    &state,
                    &journal,
                    &project_root,
                    &fork.id,
                    error.to_string(),
                );
            }
            true
        }
    };
    if !published {
        return abort_unpublished_fork(
            &state,
            &journal,
            &project_root,
            &fork.id,
            "session changed while the fork was being prepared".to_string(),
        );
    }

    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut next_state = project_session_state(&config, None);
    if let Err(error) = complete_project_lifecycle_journal(&journal) {
        next_state.last_error = Some(format!(
            "Session forked, but its recovery journal could not be cleared: {error}"
        ));
    }
    Ok(next_state)
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn archive_session(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        archive_session_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session archive failed to join: {error}"))?
}

fn archive_session_blocking(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let latest_sequence = open_app_read_store().ok().and_then(|store| {
        load_agent_session_read_model_snapshot(&store, &input.session_id)
            .ok()
            .map(|snapshot| snapshot.revision)
    });
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let Some(index) = candidate
        .sessions
        .iter()
        .position(|session| session.id == input.session_id)
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    let project_id = candidate.sessions[index].project_id.clone();
    let now = current_time_millis();
    candidate.sessions[index].archived_at_ms = Some(now);
    candidate.sessions[index].updated_at_ms = now;
    if let Some(latest_sequence) = latest_sequence {
        candidate.sessions[index].seen_event_sequence = latest_sequence;
    }
    if candidate.active_session_id == input.session_id {
        candidate.active_session_id = ensure_open_session_for_project(&mut candidate, &project_id);
    }
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn restore_session(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        restore_session_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session restore failed to join: {error}"))?
}

fn restore_session_blocking(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let Some(session) = candidate
        .sessions
        .iter_mut()
        .find(|session| session.id == input.session_id)
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    session.archived_at_ms = None;
    session.updated_at_ms = current_time_millis();
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn delete_session(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        delete_session_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session deletion failed to join: {error}"))?
}

fn delete_session_blocking(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let session_id = input.session_id;
    let (project_id, project_root, journal, mut next_state) = {
        let _lifecycle = state
            .session_lifecycle_gate
            .lock()
            .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
        let mut config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let mut candidate = config.clone();
        let Some(index) = candidate
            .sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return Ok(project_session_state(
                &config,
                Some("session not found".to_string()),
            ));
        };
        let project_id = candidate.sessions[index].project_id.clone();
        let project_root = candidate
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| PathBuf::from(&project.root))
            .ok_or_else(|| "project not found for session".to_string())?;
        if let Some(reason) =
            session_deletion_block_reason(&state, std::slice::from_ref(&session_id))?
        {
            return Ok(project_session_state(&config, Some(reason)));
        }
        candidate.sessions.remove(index);
        if candidate.active_session_id == session_id {
            candidate.active_session_id =
                ensure_open_session_for_project(&mut candidate, &project_id);
        }
        let journal = ProjectLifecycleJournal::delete(
            project_id.clone(),
            project_root.clone(),
            vec![session_id.clone()],
            false,
        );
        persist_project_lifecycle_journal(&journal)?;
        if let Err(error) = commit_project_session_config(&mut config, candidate) {
            let _ = complete_project_lifecycle_journal(&journal);
            return Err(error.to_string());
        }
        (
            project_id,
            project_root,
            journal,
            project_session_state(&config, None),
        )
    };

    let runtime_cleanup_error =
        clear_session_runtime_state(&state, std::slice::from_ref(&session_id)).err();
    let durable_cleanup = cleanup_published_delete_with_managed_artifacts(
        &state,
        &project_id,
        &project_root,
        std::slice::from_ref(&session_id),
        false,
    );
    match durable_cleanup {
        Ok(Some(ledger)) => match state.provider_config.lock() {
            Ok(provider_config) => {
                schedule_project_memory_vector_refresh(
                    project_root,
                    provider_config.clone(),
                    ledger,
                );
                let journal_error = complete_project_lifecycle_journal(&journal).err();
                next_state.last_error = match (runtime_cleanup_error, journal_error) {
                    (Some(runtime_error), Some(journal_error)) => Some(format!(
                        "Session deleted, but runtime cleanup failed ({runtime_error}) and its recovery journal could not be cleared: {journal_error}"
                    )),
                    (Some(runtime_error), None) => Some(format!(
                        "Session deleted, but temporary runtime state could not be cleared: {runtime_error}"
                    )),
                    (None, Some(journal_error)) => Some(format!(
                        "Session deleted, but its recovery journal could not be cleared: {journal_error}"
                    )),
                    (None, None) => None,
                };
            }
            Err(error) => {
                next_state.last_error = Some(format!(
                    "Session deleted, but its memory refresh could not be scheduled: {error}"
                ));
            }
        },
        Ok(None) => {
            if let Err(error) = complete_project_lifecycle_journal(&journal) {
                next_state.last_error = Some(format!(
                    "Session deleted, but its recovery journal could not be cleared: {error}"
                ));
            }
        }
        Err(error) => {
            let error = runtime_cleanup_error
                .map(|runtime_error| format!("{runtime_error}; {error}"))
                .unwrap_or(error);
            next_state.last_error = Some(format!(
                "Session deleted, but some related data could not be cleaned up: {error}"
            ));
        }
    }
    Ok(next_state)
}

fn abort_unpublished_fork(
    state: &AppState,
    journal: &ProjectLifecycleJournal,
    project_root: &Path,
    session_id: &str,
    error: String,
) -> Result<ProjectSessionState, String> {
    let cleanup = state
        .store
        .lock()
        .map_err(|lock_error| format!("store lock poisoned: {lock_error}"))
        .and_then(|mut store| rollback_unpublished_fork(&mut store, project_root, session_id));
    if cleanup.is_ok() {
        let _ = complete_project_lifecycle_journal(journal);
    }
    project_session_state_with_error(
        state,
        cleanup
            .err()
            .map(|cleanup_error| {
                format!("{error}; incomplete fork cleanup will be retried: {cleanup_error}")
            })
            .unwrap_or(error),
    )
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn select_session(
    app: tauri::AppHandle,
    input: SelectSessionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        select_session_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session selection failed to join: {error}"))?
}

fn select_session_blocking(
    state: tauri::State<'_, AppState>,
    input: SelectSessionInput,
) -> Result<ProjectSessionState, String> {
    let mut workspace_config = state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let Some(session) = candidate
        .sessions
        .iter()
        .find(|session| session.id == input.session_id && session.archived_at_ms.is_none())
        .cloned()
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    let Some(project) = candidate
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .cloned()
    else {
        return Ok(project_session_state(
            &config,
            Some("session project not found".to_string()),
        ));
    };
    let root = validate_workspace_root(&project.root)?;
    candidate.active_project_id = project.id;
    candidate.active_session_id = session.id;
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    publish_workspace_config_cache(&mut workspace_config, WorkspaceConfig { root });

    Ok(project_session_state(&config, None))
}

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn acknowledge_session_activity(
    app: tauri::AppHandle,
    input: AcknowledgeSessionActivityInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        acknowledge_session_activity_blocking(state, input)
    })
    .await
    .map_err(|error| format!("session activity acknowledgement failed to join: {error}"))?
}

fn acknowledge_session_activity_blocking(
    state: tauri::State<'_, AppState>,
    input: AcknowledgeSessionActivityInput,
) -> Result<ProjectSessionState, String> {
    let store = open_app_read_store()?;
    let latest_sequence = load_agent_session_read_model_snapshot(&store, &input.session_id)
        .map_err(|error| error.to_string())?
        .revision;
    let acknowledged_sequence =
        acknowledged_event_sequence(latest_sequence, input.through_sequence);
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let Some(session) = candidate
        .sessions
        .iter_mut()
        .find(|session| session.id == input.session_id && session.archived_at_ms.is_none())
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    if acknowledged_sequence > session.seen_event_sequence {
        session.seen_event_sequence = acknowledged_sequence;
        commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    }
    project_session_state_from_store(&config, &store, None).map_err(|error| error.to_string())
}
