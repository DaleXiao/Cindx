use super::*;

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn create_project(
    app: tauri::AppHandle,
    input: CreateProjectInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        create_project_blocking(state, input)
    })
    .await
    .map_err(|error| format!("project creation failed to join: {error}"))?
}

fn create_project_blocking(
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
        agent_model: String::new(),
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

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn rename_project(
    app: tauri::AppHandle,
    input: RenameProjectInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        rename_project_blocking(state, input)
    })
    .await
    .map_err(|error| format!("project rename failed to join: {error}"))?
}

fn rename_project_blocking(
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

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn delete_project(
    app: tauri::AppHandle,
    input: ProjectActionInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        delete_project_blocking(state, input)
    })
    .await
    .map_err(|error| format!("project deletion failed to join: {error}"))?
}

fn delete_project_blocking(
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

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn select_project(
    app: tauri::AppHandle,
    input: SelectProjectInput,
) -> Result<ProjectSessionState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        select_project_blocking(state, input)
    })
    .await
    .map_err(|error| format!("project selection failed to join: {error}"))?
}

fn select_project_blocking(
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
                    agent_model: String::new(),
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

/// Runs off the invoke thread (P2-05). Its mutation is serialized by the
/// `project_session_config` guard it holds across the read-modify-write and its
/// commit, and its lifecycle transition by `session_lifecycle_gate`, so moving it
/// off the main thread changes where the wait happens and not what may interleave.
#[tauri::command]
pub(crate) async fn save_workspace_root(
    app: tauri::AppHandle,
    input: WorkspaceInput,
) -> Result<RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        save_workspace_root_blocking(state, input)
    })
    .await
    .map_err(|error| format!("workspace root save failed to join: {error}"))?
}

fn save_workspace_root_blocking(
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
        source_revision: option_env!("CINDX_SOURCE_REVISION")
            .unwrap_or("unknown")
            .to_string(),
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
            default: agent_run_budget_view("default"),
            high: agent_run_budget_view("high"),
            xhigh: agent_run_budget_view("xhigh"),
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
