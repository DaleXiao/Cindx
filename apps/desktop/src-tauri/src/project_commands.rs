use super::*;
use crate::desktop_event_sink::DesktopEventSink;
use crate::managed_artifact_lifecycle::{
    apply_managed_artifact_retirement, plan_managed_artifact_retirement,
    retire_managed_browser_sessions,
};
use agent_memory::MemoryLedger;

fn cleanup_published_delete_with_managed_artifacts(
    state: &tauri::State<'_, AppState>,
    project_id: &str,
    project_root: &Path,
    session_ids: &[String],
    delete_project: bool,
) -> Result<Option<MemoryLedger>, String> {
    let initial_retirement = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        plan_managed_artifact_retirement(&store, project_root, session_ids)?
    };
    retire_managed_browser_sessions(&initial_retirement)?;

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let retirement = plan_managed_artifact_retirement(&store, project_root, session_ids)?;
    apply_managed_artifact_retirement(&retirement)?;
    cleanup_published_delete(
        &mut store,
        project_id,
        project_root,
        session_ids,
        delete_project,
    )
}

pub(crate) fn acknowledged_event_sequence(
    latest_sequence: u64,
    through_sequence: Option<u64>,
) -> u64 {
    through_sequence
        .unwrap_or(latest_sequence)
        .min(latest_sequence)
}

#[tauri::command]
pub(crate) fn create_project(
    state: tauri::State<'_, AppState>,
    input: CreateProjectInput,
) -> Result<ProjectSessionState, String> {
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return project_session_state_with_error(&state, "project name is empty");
    }
    let root = validate_workspace_root(&input.root)?;
    let mut workspace_config = state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let project_id = new_project_id(&name);
    let session_id = new_session_id();
    let now = current_time_millis();
    candidate.projects.push(ProjectRecord {
        id: project_id.clone(),
        name: name.clone(),
        root: root.display().to_string(),
        detail: "workspace project".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
    });
    candidate.sessions.push(SessionRecord {
        id: session_id.clone(),
        project_id: project_id.clone(),
        name: "New Session".to_string(),
        title_state: SessionTitleState::Pending,
        detail: "timeline + chat".to_string(),
        effort: default_agent_effort(),
        seen_event_sequence: 0,
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    candidate.active_project_id = project_id;
    candidate.active_session_id = session_id;
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    publish_workspace_config_cache(&mut workspace_config, WorkspaceConfig { root });

    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) fn create_session(
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

#[tauri::command]
pub(crate) fn rename_project(
    state: tauri::State<'_, AppState>,
    input: RenameProjectInput,
) -> Result<ProjectSessionState, String> {
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return project_session_state_with_error(&state, "project name is empty");
    }
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let mut candidate = config.clone();
    let Some(project) = candidate
        .projects
        .iter_mut()
        .find(|project| project.id == input.project_id)
    else {
        return Ok(project_session_state(
            &config,
            Some("project not found".to_string()),
        ));
    };
    project.name = name;
    project.updated_at_ms = current_time_millis();
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) fn delete_project(
    state: tauri::State<'_, AppState>,
    input: ProjectActionInput,
) -> Result<ProjectSessionState, String> {
    let (deleted_session_ids, project_root, journal, mut next_state) = {
        let _lifecycle = state
            .session_lifecycle_gate
            .lock()
            .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
        let mut workspace_config = state
            .workspace_config
            .lock()
            .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
        let mut config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let mut candidate = config.clone();
        let Some((project, deleted_session_ids)) =
            remove_project_from_config(&mut candidate, &input.project_id)
        else {
            return Ok(project_session_state(
                &config,
                Some("project not found".to_string()),
            ));
        };
        if let Some(reason) = session_deletion_block_reason(&state, &deleted_session_ids)? {
            return Ok(project_session_state(&config, Some(reason)));
        }

        let next_workspace_root = candidate
            .active_project()
            .map(|active_project| PathBuf::from(&active_project.root));
        let project_root = PathBuf::from(&project.root);
        let journal = ProjectLifecycleJournal::delete(
            input.project_id.clone(),
            project_root.clone(),
            deleted_session_ids.clone(),
            true,
        );
        persist_project_lifecycle_journal(&journal)?;
        if let Err(error) = commit_project_session_config(&mut config, candidate) {
            let _ = complete_project_lifecycle_journal(&journal);
            return Err(error.to_string());
        }
        if let Some(root) = next_workspace_root {
            publish_workspace_config_cache(&mut workspace_config, WorkspaceConfig { root });
        }
        (
            deleted_session_ids,
            project_root,
            journal,
            project_session_state(&config, None),
        )
    };

    let runtime_cleanup_error = clear_session_runtime_state(&state, &deleted_session_ids).err();
    let durable_cleanup = cleanup_published_delete_with_managed_artifacts(
        &state,
        &input.project_id,
        &project_root,
        &deleted_session_ids,
        true,
    )
    .map(|_| ());
    match durable_cleanup {
        Err(error) => {
            let error = runtime_cleanup_error
                .map(|runtime_error| format!("{runtime_error}; {error}"))
                .unwrap_or(error);
            next_state.last_error = Some(format!(
                "Project deleted, but some related data could not be cleaned up: {error}"
            ));
        }
        Ok(()) => {
            let journal_error = complete_project_lifecycle_journal(&journal).err();
            next_state.last_error = match (runtime_cleanup_error, journal_error) {
                (Some(runtime_error), Some(journal_error)) => Some(format!(
                    "Project deleted, but runtime cleanup failed ({runtime_error}) and its recovery journal could not be cleared: {journal_error}"
                )),
                (Some(runtime_error), None) => Some(format!(
                    "Project deleted, but temporary runtime state could not be cleared: {runtime_error}"
                )),
                (None, Some(journal_error)) => Some(format!(
                    "Project deleted, but its recovery journal could not be cleared: {journal_error}"
                )),
                (None, None) => None,
            };
        }
    }
    Ok(next_state)
}

#[tauri::command]
pub(crate) fn rename_session(
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

#[tauri::command]
pub(crate) fn set_session_effort(
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

#[tauri::command]
pub(crate) fn fork_session(
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

#[tauri::command]
pub(crate) fn archive_session(
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

#[tauri::command]
pub(crate) fn restore_session(
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

#[tauri::command]
pub(crate) fn delete_session(
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
    state: &tauri::State<'_, AppState>,
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

#[tauri::command]
pub(crate) fn select_project(
    state: tauri::State<'_, AppState>,
    input: SelectProjectInput,
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
    let Some(project) = candidate
        .projects
        .iter()
        .find(|project| project.id == input.project_id)
        .cloned()
    else {
        return Ok(project_session_state(
            &config,
            Some("project not found".to_string()),
        ));
    };
    let root = validate_workspace_root(&project.root)?;
    candidate.active_project_id = project.id.clone();
    if !candidate.sessions.iter().any(|session| {
        session.id == candidate.active_session_id
            && session.project_id == project.id
            && session.archived_at_ms.is_none()
            && !is_schedule_execution_session(session)
    }) {
        candidate.active_session_id = candidate
            .sessions
            .iter()
            .find(|session| {
                session.project_id == project.id
                    && session.archived_at_ms.is_none()
                    && !is_schedule_execution_session(session)
            })
            .map(|session| session.id.clone())
            .unwrap_or_else(|| {
                let session_id = new_session_id();
                let now = current_time_millis();
                candidate.sessions.push(SessionRecord {
                    id: session_id.clone(),
                    project_id: project.id.clone(),
                    name: format!("{} Session", project.name),
                    title_state: SessionTitleState::Pending,
                    detail: "timeline + chat".to_string(),
                    effort: default_agent_effort(),
                    seen_event_sequence: 0,
                    created_at_ms: now,
                    updated_at_ms: now,
                    archived_at_ms: None,
                });
                session_id
            });
    }
    commit_project_session_config(&mut config, candidate).map_err(|error| error.to_string())?;
    publish_workspace_config_cache(&mut workspace_config, WorkspaceConfig { root });

    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) fn select_session(
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

#[tauri::command]
pub(crate) fn acknowledge_session_activity(
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

#[tauri::command]
pub(crate) fn save_workspace_root(
    state: tauri::State<'_, AppState>,
    input: WorkspaceInput,
) -> Result<RuntimeStatus, String> {
    let root = validate_workspace_root(&input.path)?;
    let mut workspace_config = state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
    let mut project_session_config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let active_project_id = project_session_config.active_project_id.clone();
    if project_session_config
        .projects
        .iter()
        .any(|project| project.id == active_project_id)
    {
        let mut candidate = project_session_config.clone();
        if let Some(project) = candidate
            .projects
            .iter_mut()
            .find(|project| project.id == active_project_id)
        {
            project.root = root.display().to_string();
            project.updated_at_ms = current_time_millis();
        }
        commit_project_session_config(&mut project_session_config, candidate)
            .map_err(|error| error.to_string())?;
        publish_workspace_config_cache(&mut workspace_config, WorkspaceConfig { root });
    } else {
        commit_workspace_config(&mut workspace_config, WorkspaceConfig { root })
            .map_err(|error| error.to_string())?;
    }
    drop(project_session_config);
    drop(workspace_config);

    runtime_status(&state)
}

#[tauri::command]
pub(crate) fn pick_workspace_folder(
    initial_path: Option<String>,
) -> Result<Option<String>, String> {
    show_native_workspace_folder_picker(initial_path.as_deref())
}

#[cfg(target_os = "macos")]
pub(crate) fn show_native_workspace_folder_picker(
    initial_path: Option<&str>,
) -> Result<Option<String>, String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{NSString, NSURL};

    let Some(main_thread) = MainThreadMarker::new() else {
        return Err("workspace folder picker must run on the main thread".to_string());
    };
    let panel = NSOpenPanel::openPanel(main_thread);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);

    if let Some(path) = initial_path.filter(|path| Path::new(path).is_dir()) {
        let path = NSString::from_str(path);
        let directory_url = NSURL::fileURLWithPath_isDirectory(&path, true);
        panel.setDirectoryURL(Some(&directory_url));
    }

    if panel.runModal() != NSModalResponseOK {
        return Ok(None);
    }

    Ok(panel
        .URL()
        .and_then(|url| url.path())
        .map(|path| path.to_string()))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn show_native_workspace_folder_picker(
    _initial_path: Option<&str>,
) -> Result<Option<String>, String> {
    Err("native workspace folder selection is not available on this platform".to_string())
}

pub(crate) fn runtime_status(state: &tauri::State<'_, AppState>) -> Result<RuntimeStatus, String> {
    let root = active_workspace_root(state)?;
    let mut status = runtime_status_for_root(root.clone());
    status.provider_ready = clone_provider_config(state)?.is_ready();
    let registry = tool_registry_for_state(state, &root)?;
    status.registered_tools = registry
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .chain(
            ["rag.index", "rag.search", "rag.answer"]
                .into_iter()
                .map(str::to_string),
        )
        .collect();
    status.registered_tools.sort();
    status.registered_tools.dedup();
    Ok(status)
}

pub(crate) fn runtime_status_for_root(root: PathBuf) -> RuntimeStatus {
    RuntimeStatus {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        kernel_status: "kernel bridge online".to_string(),
        provider_ready: false,
        workspace_root: root.display().to_string(),
        orchestration_modes: vec![
            OrchestrationPolicy::Single.label().to_string(),
            OrchestrationPolicy::PlanExecuteReview.label().to_string(),
            OrchestrationPolicy::BestOfN { candidates: 3 }
                .label()
                .to_string(),
            OrchestrationPolicy::AutoRouter.label().to_string(),
        ],
        registered_tools: vec![
            "file.read".to_string(),
            "file.list".to_string(),
            "file.write".to_string(),
            "file.search".to_string(),
            "shell.run".to_string(),
            "rag.index".to_string(),
            "rag.search".to_string(),
            "rag.answer".to_string(),
            "web.search".to_string(),
            "browser.open".to_string(),
            "browser.extract_text".to_string(),
            "browser.capture".to_string(),
            "browser.click".to_string(),
            "browser.type".to_string(),
            "browser.scroll".to_string(),
            "browser.tabs".to_string(),
            "browser.select_tab".to_string(),
            "browser.close".to_string(),
            "computer.screenshot".to_string(),
            "computer.click".to_string(),
            "computer.type".to_string(),
            "computer.key".to_string(),
            "computer.scroll".to_string(),
        ],
        agent_run_budgets: AgentRunBudgetsView {
            fast: agent_run_budget_view("fast"),
            auto: agent_run_budget_view("auto"),
            pro: agent_run_budget_view("pro"),
        },
    }
}

fn agent_run_budget_view(effort: &str) -> AgentRunBudgetView {
    let budget = RunBudget::for_effort(effort);
    AgentRunBudgetView {
        max_duration_ms: u64::try_from(budget.max_duration.as_millis()).unwrap_or(u64::MAX),
        max_model_calls: budget.max_model_calls,
        max_tool_calls: budget.max_tool_calls,
    }
}
