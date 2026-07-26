use crate::{
    agent_read_model::is_agent_run_start_event,
    app_state::AppState,
    configuration_models::{
        default_agent_effort, is_schedule_execution_session, AgentEffort, ProjectRecord,
        ProjectSessionConfig, SessionRecord, WorkspaceConfig,
    },
    persistence_runtime::{
        project_session_config_path, validate_workspace_root, workspace_config_path,
    },
    runtime_values::{
        current_time_millis, new_session_id, sanitize_config_value, sanitize_record_field,
    },
    session_projection::load_agent_session_read_model_snapshot,
    session_title_service::is_automatic_session_name,
    view_models::{ProjectSessionState, ProjectView, SessionView},
};
use agent_application::{project_session_lifecycle, SessionLifecycleInput, SessionTitleState};
use agent_core::{Event, Metadata};
use agent_storage::{SqliteStore, StorageError};
use std::{fs, io::Write, path::Path};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub(crate) fn load_workspace_config() -> WorkspaceConfig {
    let mut config = WorkspaceConfig::default();
    let Ok(text) = fs::read_to_string(workspace_config_path()) else {
        return config;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "root" {
            if let Ok(root) = validate_workspace_root(value) {
                config.root = root;
            }
        }
    }

    config
}

pub(crate) fn save_workspace_config_to_disk(
    config: &WorkspaceConfig,
) -> Result<(), std::io::Error> {
    let path = workspace_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(format!("root={}\n", config.root.display()).as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

pub(crate) fn load_project_session_config(fallback_root: &Path) -> ProjectSessionConfig {
    let mut config = ProjectSessionConfig::default_for_root(fallback_root);
    let Ok(text) = fs::read_to_string(project_session_config_path()) else {
        return config;
    };

    let mut active_project_id = String::new();
    let mut active_session_id = String::new();
    let mut projects = Vec::new();
    let mut sessions = Vec::new();

    for line in text.lines() {
        if let Some(value) = line.strip_prefix("active_project_id=") {
            active_project_id = value.to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("active_session_id=") {
            active_session_id = value.to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("project\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() >= 6 {
                projects.push(ProjectRecord {
                    id: fields[0].to_string(),
                    name: fields[1].to_string(),
                    root: fields[2].to_string(),
                    detail: fields[3].to_string(),
                    created_at_ms: fields[4].parse().unwrap_or_default(),
                    updated_at_ms: fields[5].parse().unwrap_or_default(),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("session\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() >= 7 {
                let name = fields[2].to_string();
                sessions.push(SessionRecord {
                    id: fields[0].to_string(),
                    project_id: fields[1].to_string(),
                    title_state: SessionTitleState::parse(
                        fields.get(9).copied(),
                        is_automatic_session_name(&name),
                    ),
                    name,
                    detail: fields[3].to_string(),
                    effort: fields
                        .get(8)
                        .map(|value| AgentEffort::parse(value).label().to_string())
                        .unwrap_or_else(default_agent_effort),
                    seen_event_sequence: fields
                        .get(10)
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or_default(),
                    created_at_ms: fields[4].parse().unwrap_or_default(),
                    updated_at_ms: fields[5].parse().unwrap_or_default(),
                    archived_at_ms: fields
                        .get(7)
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|value| *value > 0),
                });
                if fields[6] == "active" {
                    active_session_id = fields[0].to_string();
                }
            }
        }
    }

    config.projects = projects;
    config.sessions = sessions;
    config.active_project_id = active_project_id;
    config.active_session_id = active_session_id;
    if config.projects.is_empty() {
        config.sessions.clear();
        config.active_project_id.clear();
        config.active_session_id.clear();
        return config;
    }
    config.ensure_consistent(fallback_root);

    config
}

pub(crate) fn save_project_session_config_to_disk(
    config: &ProjectSessionConfig,
) -> Result<(), std::io::Error> {
    let path = project_session_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut text = format!(
        "active_project_id={}\nactive_session_id={}\n",
        sanitize_config_value(&config.active_project_id),
        sanitize_config_value(&config.active_session_id)
    );
    for project in &config.projects {
        text.push_str(&format!(
            "project\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sanitize_record_field(&project.id),
            sanitize_record_field(&project.name),
            sanitize_record_field(&project.root),
            sanitize_record_field(&project.detail),
            project.created_at_ms,
            project.updated_at_ms
        ));
    }
    for session in &config.sessions {
        let active_marker = if session.id == config.active_session_id {
            "active"
        } else {
            ""
        };
        text.push_str(&format!(
            "session\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sanitize_record_field(&session.id),
            sanitize_record_field(&session.project_id),
            sanitize_record_field(&session.name),
            sanitize_record_field(&session.detail),
            session.created_at_ms,
            session.updated_at_ms,
            active_marker,
            session.archived_at_ms.unwrap_or_default(),
            sanitize_record_field(&session.effort),
            session.title_state.label(),
            session.seen_event_sequence
        ));
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(text.as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

pub(crate) fn project_session_state(
    config: &ProjectSessionConfig,
    last_error: Option<String>,
) -> ProjectSessionState {
    ProjectSessionState {
        projects: config
            .projects
            .iter()
            .map(|project| ProjectView {
                id: project.id.clone(),
                name: project.name.clone(),
                root: project.root.clone(),
                detail: project.detail.clone(),
                status: if project.id == config.active_project_id {
                    "Selected".to_string()
                } else {
                    "Ready".to_string()
                },
                active: project.id == config.active_project_id,
                created_at_ms: project.created_at_ms,
                updated_at_ms: project.updated_at_ms,
            })
            .collect(),
        sessions: config
            .sessions
            .iter()
            .filter(|session| !is_schedule_execution_session(session))
            .map(|session| SessionView {
                id: session.id.clone(),
                project_id: session.project_id.clone(),
                name: session.name.clone(),
                title_state: session.title_state.label().to_string(),
                detail: session.detail.clone(),
                effort: session.effort.clone(),
                status: if session.id == config.active_session_id {
                    "Active".to_string()
                } else if session.archived_at_ms.is_some() {
                    "Archived".to_string()
                } else {
                    "Ready".to_string()
                },
                activity: "idle".to_string(),
                attention_reason: None,
                unseen_result: false,
                latest_sequence: session.seen_event_sequence,
                active: session.id == config.active_session_id,
                archived: session.archived_at_ms.is_some(),
                archived_at_ms: session.archived_at_ms,
                created_at_ms: session.created_at_ms,
                updated_at_ms: session.updated_at_ms,
            })
            .collect(),
        active_project_id: config.active_project_id.clone(),
        active_session_id: config.active_session_id.clone(),
        last_error,
    }
}

pub(crate) fn apply_session_lifecycle_projections(
    state: &mut ProjectSessionState,
    config: &ProjectSessionConfig,
    store: &SqliteStore,
) -> Result<(), StorageError> {
    for session in &mut state.sessions {
        if session.archived {
            continue;
        }
        let Some(record) = config
            .sessions
            .iter()
            .find(|record| record.id == session.id)
        else {
            continue;
        };
        let model = load_agent_session_read_model_snapshot(store, &session.id)?;
        let projection = project_session_lifecycle(SessionLifecycleInput {
            status: &model.state.status,
            can_continue: model.state.can_continue,
            latest_sequence: model.state.latest_sequence,
            seen_event_sequence: record.seen_event_sequence,
        });
        session.status = projection.status_label.to_string();
        session.activity = projection.activity.to_string();
        session.attention_reason = projection.attention_reason.map(str::to_string);
        session.unseen_result = projection.unseen_result;
        session.latest_sequence = projection.latest_sequence;
    }
    Ok(())
}

pub(crate) fn project_session_state_from_store(
    config: &ProjectSessionConfig,
    store: &SqliteStore,
    last_error: Option<String>,
) -> Result<ProjectSessionState, StorageError> {
    let mut state = project_session_state(config, last_error);
    apply_session_lifecycle_projections(&mut state, config, store)?;
    Ok(state)
}

pub(crate) fn remove_project_from_config(
    config: &mut ProjectSessionConfig,
    project_id: &str,
) -> Option<(ProjectRecord, Vec<String>)> {
    let project_index = config
        .projects
        .iter()
        .position(|project| project.id == project_id)?;
    let project = config.projects[project_index].clone();
    let deleted_session_ids = config
        .sessions
        .iter()
        .filter(|session| session.project_id == project.id)
        .map(|session| session.id.clone())
        .collect::<Vec<_>>();
    let deleting_active_project = config.active_project_id == project.id;

    config.projects.remove(project_index);
    config
        .sessions
        .retain(|session| session.project_id != project.id);

    if config.projects.is_empty() {
        config.active_project_id.clear();
        config.active_session_id.clear();
        return Some((project, deleted_session_ids));
    }
    if deleting_active_project {
        config.active_project_id = config.projects
            [project_index.min(config.projects.len().saturating_sub(1))]
        .id
        .clone();
    }
    let active_session_is_valid = config.sessions.iter().any(|session| {
        session.id == config.active_session_id
            && session.project_id == config.active_project_id
            && session.archived_at_ms.is_none()
            && !is_schedule_execution_session(session)
    });
    if !active_session_is_valid {
        let active_project_id = config.active_project_id.clone();
        config.active_session_id = ensure_open_session_for_project(config, &active_project_id);
    }

    Some((project, deleted_session_ids))
}

pub(crate) fn ensure_open_session_for_project(
    config: &mut ProjectSessionConfig,
    project_id: &str,
) -> String {
    if let Some(session) = config.sessions.iter().find(|session| {
        session.project_id == project_id
            && session.archived_at_ms.is_none()
            && !is_schedule_execution_session(session)
    }) {
        return session.id.clone();
    }

    let project_name = config
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .map(|project| project.name.clone())
        .unwrap_or_else(|| "Runtime".to_string());
    let name = format!("{project_name} Session");
    let id = new_session_id();
    let now = current_time_millis();
    config.sessions.push(SessionRecord {
        id: id.clone(),
        project_id: project_id.to_string(),
        name,
        title_state: SessionTitleState::Pending,
        detail: "timeline + chat".to_string(),
        effort: default_agent_effort(),
        seen_event_sequence: 0,
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    id
}

pub(crate) fn unique_fork_name(config: &ProjectSessionConfig, source: &SessionRecord) -> String {
    let base = format!("{} Fork", source.name);
    if !config
        .sessions
        .iter()
        .any(|session| session.project_id == source.project_id && session.name == base)
    {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{} Fork {suffix}", source.name);
        if !config
            .sessions
            .iter()
            .any(|session| session.project_id == source.project_id && session.name == candidate)
        {
            return candidate;
        }
        suffix += 1;
    }
}

pub(crate) fn project_session_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<ProjectSessionState, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    Ok(project_session_state(&config, Some(message.into())))
}

pub(crate) fn sync_active_project_root(
    state: &tauri::State<'_, AppState>,
    root: &Path,
) -> Result<(), String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let active_project_id = config.active_project_id.clone();
    if let Some(project) = config
        .projects
        .iter_mut()
        .find(|project| project.id == active_project_id)
    {
        project.root = root.display().to_string();
        project.updated_at_ms = current_time_millis();
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn project_session_metadata_for_session(
    state: &tauri::State<'_, AppState>,
    session_id: Option<&str>,
) -> Result<Metadata, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    let Some(session_id) = session_id else {
        return Ok(project_session_metadata(&config));
    };
    let session = config
        .sessions
        .iter()
        .find(|session| session.id == session_id && session.archived_at_ms.is_none())
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    let project = config
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| format!("project not found for session: {session_id}"))?;

    Ok([
        ("project_id".to_string(), project.id.clone()),
        ("project_name".to_string(), project.name.clone()),
        ("project_root".to_string(), project.root.clone()),
        ("session_id".to_string(), session.id.clone()),
        ("session_name".to_string(), session.name.clone()),
    ]
    .into_iter()
    .collect())
}

pub(crate) fn project_session_metadata(config: &ProjectSessionConfig) -> Metadata {
    let mut metadata = Metadata::new();
    if let Some(project) = config.active_project() {
        metadata.insert("project_id".to_string(), project.id.clone());
        metadata.insert("project_name".to_string(), project.name.clone());
        metadata.insert("project_root".to_string(), project.root.clone());
    }
    if let Some(session) = config.active_session() {
        metadata.insert("session_id".to_string(), session.id.clone());
        metadata.insert("session_name".to_string(), session.name.clone());
    }
    metadata
}

pub(crate) fn metadata_with_context(mut metadata: Metadata, context: &Metadata) -> Metadata {
    for (key, value) in context {
        metadata.entry(key.clone()).or_insert_with(|| value.clone());
    }
    metadata
}

pub(crate) fn agent_run_context_from_events(events: &[Event]) -> AgentRunContext {
    let start = events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .or_else(|| {
            events
                .iter()
                .find(|event| event.metadata.contains_key("session_id"))
        });
    AgentRunContext {
        project_id: start.and_then(|event| event.metadata.get("project_id").cloned()),
        project_name: start.and_then(|event| event.metadata.get("project_name").cloned()),
        session_id: start.and_then(|event| event.metadata.get("session_id").cloned()),
        session_name: start.and_then(|event| event.metadata.get("session_name").cloned()),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AgentRunContext {
    pub(crate) project_id: Option<String>,
    pub(crate) project_name: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) session_name: Option<String>,
}
