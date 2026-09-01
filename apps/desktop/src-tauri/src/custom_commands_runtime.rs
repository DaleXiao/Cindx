use crate::app_state::AppState;
use crate::persistence_runtime::active_workspace_root;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const CUSTOM_COMMANDS_SCHEMA: &str = "cindx.custom-commands.v1";
const MAX_CUSTOM_COMMAND_FILES: usize = 24;
const MAX_CUSTOM_COMMAND_FILE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomCommandView {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) effort: Option<String>,
    pub(crate) template: String,
    pub(crate) scope: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomCommandsState {
    pub(crate) schema: String,
    pub(crate) commands: Vec<CustomCommandView>,
    pub(crate) last_error: Option<String>,
}

pub(crate) fn commands_directory(root: &Path) -> PathBuf {
    root.join(".cindx").join("commands")
}

pub(crate) fn parse_custom_command_file(name: &str, scope: &str, text: &str) -> CustomCommandView {
    let (description, effort, template) = split_frontmatter(text);
    CustomCommandView {
        name: name.to_string(),
        description,
        effort: effort
            .as_deref()
            .map(str::trim)
            .map(str::to_lowercase)
            .and_then(|value| agent_core::AgentPolicy::parse_persisted(&value))
            .map(|policy| policy.label().to_string()),
        template,
        scope: scope.to_string(),
    }
}

fn split_frontmatter(text: &str) -> (String, Option<String>, String) {
    let trimmed = text.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (String::new(), None, trimmed.trim().to_string());
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(close) = rest.find("\n---") else {
        return (String::new(), None, trimmed.trim().to_string());
    };
    let header = &rest[..close];
    let template = rest[close + 4..].trim_start_matches(['-', '\n']).trim();
    let mut description = String::new();
    let mut effort = None;
    for line in header.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = value.trim().trim_matches('"').to_string();
        match key.as_str() {
            "description" => description = value,
            "effort" => effort = Some(value),
            _ => {}
        }
    }
    (description, effort, template.to_string())
}

pub(crate) fn discover_custom_commands_in(directory: &Path, scope: &str) -> Vec<CustomCommandView> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|value| value.to_str()) == Some("md")
                && path
                    .symlink_metadata()
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .take(MAX_CUSTOM_COMMAND_FILES)
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_string();
            if name.trim().is_empty() {
                return None;
            }
            let bytes = fs::read(&path).ok()?;
            let mut boundary = bytes.len().min(MAX_CUSTOM_COMMAND_FILE_BYTES);
            while boundary > 0 && boundary < bytes.len() && (bytes[boundary] >> 6) == 0b10 {
                boundary -= 1;
            }
            let text = String::from_utf8_lossy(&bytes[..boundary]).into_owned();
            Some(parse_custom_command_file(&name, scope, &text))
        })
        .collect()
}

pub(crate) fn discover_custom_commands(
    workspace_root: &Path,
    global_root: &Path,
) -> Vec<CustomCommandView> {
    let mut commands = discover_custom_commands_in(&commands_directory(workspace_root), "project");
    let seen: std::collections::BTreeSet<String> = commands
        .iter()
        .map(|command| command.name.clone())
        .collect();
    for global_command in discover_custom_commands_in(&commands_directory(global_root), "global") {
        if !seen.contains(&global_command.name) {
            commands.push(global_command);
        }
    }
    commands
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[tauri::command]
pub(crate) fn get_custom_commands(
    state: tauri::State<'_, AppState>,
) -> Result<CustomCommandsState, String> {
    let workspace_root = active_workspace_root(&state)?;
    let commands = match home_directory() {
        Some(home) => discover_custom_commands(&workspace_root, &home),
        None => discover_custom_commands_in(&commands_directory(&workspace_root), "project"),
    };
    Ok(CustomCommandsState {
        schema: CUSTOM_COMMANDS_SCHEMA.to_string(),
        commands,
        last_error: None,
    })
}

#[cfg(test)]
#[path = "custom_commands_runtime_tests.rs"]
mod tests;
