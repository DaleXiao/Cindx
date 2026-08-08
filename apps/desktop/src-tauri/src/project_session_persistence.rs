use crate::{
    agent_read_model::is_agent_run_start_event,
    app_state::AppState,
    configuration_models::{
        default_agent_effort, is_schedule_execution_session, ProjectRecord, ProjectSessionConfig,
        SessionRecord,
    },
    runtime_values::{current_time_millis, new_session_id},
    session_projection::load_agent_session_read_model_snapshot,
    view_models::{ProjectSessionState, ProjectView, SessionView},
};
use agent_application::{
    merge_persistable_run_context, project_session_lifecycle, SessionLifecycleInput,
    SessionTitleState,
};
use agent_core::{Event, Metadata};
use agent_storage::{SqliteStore, StorageError};

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

pub(crate) fn metadata_with_context(metadata: Metadata, context: &Metadata) -> Metadata {
    merge_persistable_run_context(metadata, context)
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
