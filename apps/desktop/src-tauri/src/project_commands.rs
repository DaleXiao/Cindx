use super::*;

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
    let project_id = unique_config_id(
        "project",
        &name,
        &config
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect::<Vec<_>>(),
    );
    let session_id = new_session_id();
    let now = current_time_millis();
    config.projects.push(ProjectRecord {
        id: project_id.clone(),
        name: name.clone(),
        root: root.display().to_string(),
        detail: "workspace project".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
    });
    config.sessions.push(SessionRecord {
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
    config.active_project_id = project_id;
    config.active_session_id = session_id;
    workspace_config.root = root;
    save_workspace_config_to_disk(&workspace_config).map_err(|error| error.to_string())?;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

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
    let session_id = new_session_id();
    let now = current_time_millis();
    config.sessions.push(SessionRecord {
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
    config.active_project_id = project_id;
    config.active_session_id = session_id;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

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
    let Some(project) = config
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
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) fn delete_project(
    state: tauri::State<'_, AppState>,
    input: ProjectActionInput,
) -> Result<ProjectSessionState, String> {
    let (deleted_session_ids, attachment_dirs, context_files, project_root, mut next_state) = {
        let mut workspace_config = state
            .workspace_config
            .lock()
            .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
        let mut config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let Some((project, deleted_session_ids)) =
            remove_project_from_config(&mut config, &input.project_id)
        else {
            return Ok(project_session_state(
                &config,
                Some("project not found".to_string()),
            ));
        };
        let attachment_dirs = deleted_session_ids
            .iter()
            .map(|session_id| {
                PathBuf::from(&project.root)
                    .join(".cindx")
                    .join("attachments")
                    .join(slug_label(session_id))
            })
            .collect::<Vec<_>>();
        let context_files = deleted_session_ids
            .iter()
            .map(|session_id| {
                context_checkpoint_path_for_session(
                    Path::new(&project.root),
                    Some(session_id.as_str()),
                )
            })
            .collect::<Vec<_>>();

        if let Some(active_project) = config.active_project() {
            workspace_config.root = PathBuf::from(&active_project.root);
            save_workspace_config_to_disk(&workspace_config).map_err(|error| error.to_string())?;
        }

        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
        (
            deleted_session_ids,
            attachment_dirs,
            context_files,
            PathBuf::from(&project.root),
            project_session_state(&config, None),
        )
    };

    let mut cleanup_errors = Vec::new();
    if let Err(error) = clear_session_runtime_state(&state, &deleted_session_ids) {
        cleanup_errors.push(error);
    }
    if let Err(error) = delete_session_history(&state, &deleted_session_ids) {
        cleanup_errors.push(error);
    }
    if let Err(error) = delete_project_memory(&state, &project_root, &input.project_id) {
        cleanup_errors.push(error);
    }
    if let Err(error) = remove_staged_attachment_dirs(&attachment_dirs) {
        cleanup_errors.push(error);
    }
    if let Err(error) = remove_session_context_files(&context_files) {
        cleanup_errors.push(error);
    }
    if !cleanup_errors.is_empty() {
        next_state.last_error = Some(format!(
            "Project deleted, but some related data could not be cleaned up: {}",
            cleanup_errors.join("; ")
        ));
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
    let now = current_time_millis();
    let project_id = {
        let Some(session) = config
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
    if let Some(project) = config
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
    {
        project.updated_at_ms = now;
    }
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) fn set_session_effort(
    state: tauri::State<'_, AppState>,
    input: SessionEffortInput,
) -> Result<ProjectSessionState, String> {
    let effort = AgentEffort::parse(&input.effort).label().to_string();
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    if !update_session_effort(&mut config, &input.session_id, &effort) {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    }
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
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
            let (project_id, expected_title, expected_title_state, expected_updated_at_ms) = {
                let Some(session) = config.sessions.iter_mut().find(|session| {
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
            if let Some(project) = config
                .projects
                .iter_mut()
                .find(|project| project.id == project_id)
            {
                project.updated_at_ms = expected_updated_at_ms;
            }
            save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
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
        let project_id =
            {
                let Some(session) = config.sessions.iter_mut().find(|session| {
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
        if let Some(project) = config
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        {
            project.updated_at_ms = now;
        }
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
        let next_state = project_session_state(&config, None);
        drop(config);
        let _ = app.emit("session-title-updated", input.session_id);
        Ok(next_state)
    })
    .await
    .map_err(|error| format!("session title task failed: {error}"))?
}

#[tauri::command]
pub(crate) fn stage_agent_attachments(
    state: tauri::State<'_, AppState>,
    input: StageAgentAttachmentsInput,
) -> Result<Vec<AgentAttachmentView>, String> {
    if input.files.is_empty() {
        return Ok(Vec::new());
    }
    if input.files.len() > MAX_ATTACHMENT_FILES {
        return Err(format!(
            "a message can include at most {MAX_ATTACHMENT_FILES} attachments"
        ));
    }

    let root = project_root_for_session(&state, &input.session_id)?;
    let attachment_root = root
        .join(".cindx")
        .join("attachments")
        .join(slug_label(&input.session_id));
    fs::create_dir_all(&attachment_root)
        .map_err(|error| format!("failed to create attachment directory: {error}"))?;

    let mut decoded_files = Vec::with_capacity(input.files.len());
    let mut total_bytes = 0usize;
    for file in input.files {
        let encoded = file
            .data_base64
            .split_once(',')
            .map(|(_, data)| data)
            .unwrap_or(file.data_base64.as_str());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| format!("{} is not valid base64: {error}", file.name))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!("{} exceeds the 20 MB attachment limit", file.name));
        }
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_ATTACHMENT_TOTAL_BYTES {
            return Err("attachments exceed the 50 MB message limit".to_string());
        }
        decoded_files.push((file, bytes));
    }

    let mut staged = Vec::with_capacity(decoded_files.len());
    for (index, (file, bytes)) in decoded_files.into_iter().enumerate() {
        let name = safe_attachment_name(&file.name);
        let id = unique_id("attachment");
        let path = attachment_root.join(format!("{id}-{index}-{name}"));
        fs::write(&path, &bytes)
            .map_err(|error| format!("failed to stage attachment {name}: {error}"))?;
        staged.push(AgentAttachmentView {
            id,
            name,
            path: path.display().to_string(),
            mime_type: normalized_attachment_mime(&file.mime_type, &path),
            size_bytes: bytes.len() as u64,
        });
    }
    Ok(staged)
}

#[tauri::command]
pub(crate) fn remove_agent_attachment(
    state: tauri::State<'_, AppState>,
    input: RemoveAgentAttachmentInput,
) -> Result<(), String> {
    let root = project_root_for_session(&state, &input.session_id)?;
    let path = validated_attachment_path(&root, &input.path)?;
    fs::remove_file(path).map_err(|error| format!("failed to remove attachment: {error}"))
}

#[tauri::command]
pub(crate) fn fork_session(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let (source, project, fork) = {
        let mut config = state
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
        config.sessions.push(fork.clone());
        config.active_project_id = source.project_id.clone();
        config.active_session_id = id;
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
        (source, project, fork)
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let events = store
        .list_by_task(&phase16_task_id())
        .map_err(|error| error.to_string())?;
    for event in agent_session_events(&events, &source.id) {
        let mut metadata = event.metadata;
        metadata.insert("session_id".to_string(), fork.id.clone());
        metadata.insert("session_name".to_string(), fork.name.clone());
        metadata.insert("project_id".to_string(), project.id.clone());
        metadata.insert("project_name".to_string(), project.name.clone());
        metadata.insert("project_root".to_string(), project.root.clone());
        metadata.insert("forked_from_session_id".to_string(), source.id.clone());
        append_event(
            &mut store,
            &phase16_task_id(),
            event.kind,
            event.summary,
            metadata,
        )
        .map_err(|error| error.to_string())?;
    }

    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    Ok(project_session_state(&config, None))
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
    let Some(index) = config
        .sessions
        .iter()
        .position(|session| session.id == input.session_id)
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    let project_id = config.sessions[index].project_id.clone();
    let now = current_time_millis();
    config.sessions[index].archived_at_ms = Some(now);
    config.sessions[index].updated_at_ms = now;
    if let Some(latest_sequence) = latest_sequence {
        config.sessions[index].seen_event_sequence = latest_sequence;
    }
    if config.active_session_id == input.session_id {
        config.active_session_id = ensure_open_session_for_project(&mut config, &project_id);
    }
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
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
    let Some(session) = config
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
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) fn delete_session(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let session_id = input.session_id;
    let (attachment_dir, context_file, mut next_state) = {
        let mut config = state
            .project_session_config
            .lock()
            .map_err(|error| format!("project session config lock poisoned: {error}"))?;
        let Some(index) = config
            .sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return Ok(project_session_state(
                &config,
                Some("session not found".to_string()),
            ));
        };
        let project_id = config.sessions[index].project_id.clone();
        let attachment_dir = config
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| {
                PathBuf::from(&project.root)
                    .join(".cindx")
                    .join("attachments")
                    .join(slug_label(&session_id))
            });
        let context_file = config
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| {
                context_checkpoint_path_for_session(
                    Path::new(&project.root),
                    Some(session_id.as_str()),
                )
            });
        config.sessions.remove(index);
        if config.active_session_id == session_id {
            config.active_session_id = ensure_open_session_for_project(&mut config, &project_id);
        }
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
        (
            attachment_dir,
            context_file,
            project_session_state(&config, None),
        )
    };

    let mut cleanup_errors = Vec::new();
    if let Err(error) = clear_session_runtime_state(&state, std::slice::from_ref(&session_id)) {
        cleanup_errors.push(error);
    }
    if let Err(error) = delete_session_history(&state, std::slice::from_ref(&session_id)) {
        cleanup_errors.push(error);
    }
    if let Some(attachment_dir) = attachment_dir {
        if let Err(error) = remove_staged_attachment_dirs(&[attachment_dir]) {
            cleanup_errors.push(error);
        }
    }
    if let Some(context_file) = context_file {
        if let Err(error) = remove_session_context_files(&[context_file]) {
            cleanup_errors.push(error);
        }
    }
    if !cleanup_errors.is_empty() {
        next_state.last_error = Some(format!(
            "Session deleted, but some related data could not be cleaned up: {}",
            cleanup_errors.join("; ")
        ));
    }
    Ok(next_state)
}

pub(crate) fn clear_session_runtime_state(
    state: &tauri::State<'_, AppState>,
    session_ids: &[String],
) -> Result<(), String> {
    if session_ids.is_empty() {
        return Ok(());
    }
    {
        let mut controls = state
            .agent_run_controls
            .lock()
            .map_err(|error| format!("agent run control lock poisoned: {error}"))?;
        for session_id in session_ids {
            if let Some(control) = controls.remove(session_id) {
                control.request_cancel();
            }
        }
    }
    {
        let mut suspended = state
            .suspended_agent_runs
            .lock()
            .map_err(|error| format!("suspended agent runs lock poisoned: {error}"))?;
        for session_id in session_ids {
            suspended.remove(session_id);
        }
    }
    {
        let mut dispatching = state
            .queue_dispatching_sessions
            .lock()
            .map_err(|error| format!("queue dispatch lock poisoned: {error}"))?;
        for session_id in session_ids {
            dispatching.remove(session_id);
        }
    }
    Ok(())
}

pub(crate) fn delete_session_history(
    state: &tauri::State<'_, AppState>,
    session_ids: &[String],
) -> Result<(), String> {
    if session_ids.is_empty() {
        return Ok(());
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    for session_id in session_ids {
        store
            .delete_records_by_metadata("session_id", session_id)
            .map_err(|error| error.to_string())?;
        store
            .delete_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, session_id)
            .map_err(|error| error.to_string())?;
    }
    store
        .delete_read_model(
            ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
            ROUTING_TELEMETRY_READ_MODEL_KEY,
        )
        .map_err(|error| error.to_string())?;
    store
        .delete_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
        )
        .map_err(|error| error.to_string())?;
    drop(store);
    Ok(())
}

pub(crate) fn delete_project_memory(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    project_id: &str,
) -> Result<(), String> {
    state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?
        .delete_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, project_id)
        .map_err(|error| error.to_string())?;
    let vector_root = memory_lancedb_root_for(workspace_root, project_id);
    if let Err(error) = fs::remove_dir_all(&vector_root) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!(
                "failed to remove project memory vectors at {}: {error}",
                vector_root.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn remove_staged_attachment_dirs(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        if let Err(error) = fs::remove_dir_all(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(format!(
                    "failed to remove staged attachments at {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn remove_session_context_files(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(format!(
                    "failed to remove session context at {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
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
    let Some(project) = config
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
    config.active_project_id = project.id.clone();
    if !config.sessions.iter().any(|session| {
        session.id == config.active_session_id
            && session.project_id == project.id
            && session.archived_at_ms.is_none()
            && !is_schedule_execution_session(session)
    }) {
        config.active_session_id = config
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
                config.sessions.push(SessionRecord {
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
    workspace_config.root = root;
    save_workspace_config_to_disk(&workspace_config).map_err(|error| error.to_string())?;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

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
    let Some(session) = config
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
    let Some(project) = config
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
    config.active_project_id = project.id;
    config.active_session_id = session.id;
    workspace_config.root = root;
    save_workspace_config_to_disk(&workspace_config).map_err(|error| error.to_string())?;
    save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;

    Ok(project_session_state(&config, None))
}

#[tauri::command]
pub(crate) fn acknowledge_session_activity(
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<ProjectSessionState, String> {
    let store = open_app_read_store()?;
    let latest_sequence = load_agent_session_read_model_snapshot(&store, &input.session_id)
        .map_err(|error| error.to_string())?
        .revision;
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let Some(session) = config
        .sessions
        .iter_mut()
        .find(|session| session.id == input.session_id && session.archived_at_ms.is_none())
    else {
        return Ok(project_session_state(
            &config,
            Some("session not found".to_string()),
        ));
    };
    if latest_sequence > session.seen_event_sequence {
        session.seen_event_sequence = latest_sequence;
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    }
    project_session_state_from_store(&config, &store, None).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn save_workspace_root(
    state: tauri::State<'_, AppState>,
    input: WorkspaceInput,
) -> Result<RuntimeStatus, String> {
    let root = validate_workspace_root(&input.path)?;
    {
        let mut config = state
            .workspace_config
            .lock()
            .map_err(|error| format!("workspace config lock poisoned: {error}"))?;
        config.root = root.clone();
        save_workspace_config_to_disk(&config).map_err(|error| error.to_string())?;
    }
    sync_active_project_root(&state, &root)?;

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
            "computer.screenshot".to_string(),
            "computer.click".to_string(),
            "computer.type".to_string(),
            "computer.key".to_string(),
            "computer.scroll".to_string(),
        ],
    }
}
