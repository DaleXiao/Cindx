use crate::{
    configuration_models::{
        default_agent_effort, ProjectRecord, ProjectSessionConfig, SessionRecord, WorkspaceConfig,
    },
    persistence_runtime::{
        project_session_config_path, validate_workspace_root, workspace_config_path,
    },
    runtime_values::{sanitize_config_value, sanitize_record_field},
    session_title_service::is_automatic_session_name,
};
use agent_application::SessionTitleState;
use agent_core::AgentPolicy;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) fn load_workspace_config() -> WorkspaceConfig {
    load_workspace_config_from_path(&workspace_config_path())
}

fn load_workspace_config_from_path(path: &Path) -> WorkspaceConfig {
    let mut config = WorkspaceConfig::default();
    let Ok(text) = fs::read_to_string(path) else {
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

pub(crate) fn commit_workspace_config(
    current: &mut WorkspaceConfig,
    candidate: WorkspaceConfig,
) -> Result<(), io::Error> {
    commit_workspace_config_to_path(&workspace_config_path(), current, candidate)
}

fn commit_workspace_config_to_path(
    path: &Path,
    current: &mut WorkspaceConfig,
    candidate: WorkspaceConfig,
) -> Result<(), io::Error> {
    save_workspace_config_to_path(path, &candidate)?;
    *current = candidate;
    Ok(())
}

pub(crate) fn publish_workspace_config_cache(
    current: &mut WorkspaceConfig,
    candidate: WorkspaceConfig,
) {
    publish_workspace_config_cache_to_path(&workspace_config_path(), current, candidate);
}

fn publish_workspace_config_cache_to_path(
    path: &Path,
    current: &mut WorkspaceConfig,
    candidate: WorkspaceConfig,
) {
    if *current == candidate {
        return;
    }
    if let Err(error) = save_workspace_config_to_path(path, &candidate) {
        eprintln!("failed to update workspace config cache: {error}");
    }
    *current = candidate;
}

fn save_workspace_config_to_path(path: &Path, config: &WorkspaceConfig) -> Result<(), io::Error> {
    write_private_file_atomically(path, workspace_config_text(config).as_bytes())
}

fn workspace_config_text(config: &WorkspaceConfig) -> String {
    format!("root={}\n", config.root.display())
}

pub(crate) fn apply_authoritative_project_root(
    workspace: &mut WorkspaceConfig,
    projects: &ProjectSessionConfig,
) {
    let Some(project) = projects.active_project() else {
        return;
    };
    if let Ok(root) = validate_workspace_root(&project.root) {
        workspace.root = root;
    }
}

pub(crate) fn load_project_session_config(fallback_root: &Path) -> ProjectSessionConfig {
    load_project_session_config_from_path(&project_session_config_path(), fallback_root)
}

fn load_project_session_config_from_path(
    path: &Path,
    fallback_root: &Path,
) -> ProjectSessionConfig {
    let mut config = ProjectSessionConfig::default_for_root(fallback_root);
    let Ok(text) = fs::read_to_string(path) else {
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
                        .map(|value| AgentPolicy::parse_ingress(value).label().to_string())
                        .unwrap_or_else(default_agent_effort),
                    agent_model: fields
                        .get(11)
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .unwrap_or_default(),
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

pub(crate) fn commit_project_session_config(
    current: &mut ProjectSessionConfig,
    candidate: ProjectSessionConfig,
) -> Result<(), io::Error> {
    commit_project_session_config_to_path(&project_session_config_path(), current, candidate)
}

fn commit_project_session_config_to_path(
    path: &Path,
    current: &mut ProjectSessionConfig,
    candidate: ProjectSessionConfig,
) -> Result<(), io::Error> {
    save_project_session_config_to_path(path, &candidate)?;
    *current = candidate;
    Ok(())
}

fn save_project_session_config_to_path(
    path: &Path,
    config: &ProjectSessionConfig,
) -> Result<(), io::Error> {
    write_private_file_atomically(path, project_session_config_text(config).as_bytes())
}

fn project_session_config_text(config: &ProjectSessionConfig) -> String {
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
            "session\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
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
            session.seen_event_sequence,
            sanitize_record_field(&session.agent_model)
        ));
    }

    text
}

fn write_private_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), io::Error> {
    write_private_file_atomically_with(path, bytes, |temporary, target| {
        temporary
            .persist(target)
            .map_err(|error| error.error)
            .map(|_| ())
    })
}

fn write_private_file_atomically_with(
    path: &Path,
    bytes: &[u8],
    publish: impl FnOnce(tempfile::NamedTempFile, &Path) -> Result<(), io::Error>,
) -> Result<(), io::Error> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("config path has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("cindx-config");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    #[cfg(unix)]
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o600))?;
    temporary.as_file().sync_all()?;
    publish(temporary, path)?;

    #[cfg(unix)]
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_config_persistence_tests.rs"]
mod tests;
