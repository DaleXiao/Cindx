use crate::agent_read_model::agent_events_for_session;
use crate::app_state::AppState;
use crate::persistence_runtime::active_workspace_root;
use crate::runtime_values::{current_time_millis, phase16_task_id};
use agent_core::{Event, EventKind};
use agent_storage::{SqliteStore, StoredReadModel};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tauri::Manager;

pub(crate) const WORKSPACE_UNDO_READ_MODEL_NAMESPACE: &str = "workspace-undo-v1";
const WORKSPACE_UNDO_SCHEMA: &str = "cindx.workspace-undo-registry.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceUndoFile {
    pub(crate) path: String,
    pub(crate) undo_before_path: Option<String>,
    pub(crate) after_artifact_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceUndoEntry {
    pub(crate) tool_call_id: String,
    pub(crate) sequence: u64,
    pub(crate) tool: String,
    pub(crate) path: String,
    pub(crate) action: String,
    pub(crate) undo_before_path: Option<String>,
    pub(crate) after_artifact_path: Option<String>,
    /// The agent run that produced the change, so the thread can attach the
    /// change list to the turn that made it. `None` for entries persisted
    /// before run attribution existed.
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    /// A `file.patch_batch` call is one atomic group: every file shares the
    /// single entry so one undo restores the whole batch. Empty for
    /// single-file tools, which use the flat fields above.
    #[serde(default)]
    pub(crate) files: Vec<WorkspaceUndoFile>,
}

const MAX_UNDO_GROUP_FILES: usize = 16;

#[derive(Debug)]
pub(crate) enum WorkspaceUndoError {
    NothingToUndo,
    NothingToRedo,
    Unsupported(String),
    Conflict(String),
    MissingArtifact(String),
    Io(String),
    Persistence(String),
}

impl WorkspaceUndoError {
    fn message(&self) -> String {
        match self {
            Self::NothingToUndo => "There is nothing to undo.".to_string(),
            Self::NothingToRedo => "There is nothing to redo.".to_string(),
            Self::Unsupported(message)
            | Self::Conflict(message)
            | Self::MissingArtifact(message)
            | Self::Io(message)
            | Self::Persistence(message) => message.clone(),
        }
    }
}

pub(crate) fn project_workspace_undo_entries(events: &[Event]) -> Vec<WorkspaceUndoEntry> {
    let mut entries: Vec<WorkspaceUndoEntry> = events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::ToolCallFinished))
        .filter_map(|event| {
            let metadata = &event.metadata;
            let tool = metadata.get("tool").map(String::as_str)?;
            if tool != "file.write" && tool != "file.patch" && tool != "file.patch_batch" {
                return None;
            }
            if metadata.get("status").map(String::as_str) != Some("succeeded") {
                return None;
            }
            let action = metadata.get("result_undo_action")?.clone();
            let tool_call_id = metadata.get("tool_call_id")?.clone();
            if tool == "file.patch_batch" {
                return project_batch_entry(metadata, tool_call_id, event.sequence, action);
            }
            let path = metadata.get("result_path")?.clone();
            Some(WorkspaceUndoEntry {
                tool_call_id,
                sequence: event.sequence,
                tool: tool.to_string(),
                path,
                action,
                undo_before_path: metadata.get("result_undo_before_path").cloned(),
                after_artifact_path: metadata.get("result_artifact_path").cloned(),
                run_id: metadata.get("agent_run_id").cloned(),
                files: Vec::new(),
            })
        })
        .collect();
    entries.sort_by_key(|entry| entry.sequence);
    entries
}

/// A `file.patch_batch` call carries its per-file undo snapshots in the
/// bounded `result_undo_group` JSON array. The whole batch projects as one
/// entry so undo/redo rolls the group back together.
fn project_batch_entry(
    metadata: &agent_core::Metadata,
    tool_call_id: String,
    sequence: u64,
    action: String,
) -> Option<WorkspaceUndoEntry> {
    let files: Vec<WorkspaceUndoFile> =
        serde_json::from_str(metadata.get("result_undo_group")?).ok()?;
    if files.is_empty()
        || files.len() > MAX_UNDO_GROUP_FILES
        || files.iter().any(|file| file.path.is_empty())
    {
        return None;
    }
    let path = metadata
        .get("result_path")
        .filter(|path| !path.is_empty())
        .cloned()
        .unwrap_or_else(|| files[0].path.clone());
    Some(WorkspaceUndoEntry {
        tool_call_id,
        sequence,
        tool: "file.patch_batch".to_string(),
        path,
        action,
        undo_before_path: None,
        after_artifact_path: None,
        run_id: metadata.get("agent_run_id").cloned(),
        files,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceUndoRegistry {
    schema: String,
    session_id: String,
    undone: Vec<String>,
    updated_at_ms: u64,
}

fn load_registry(
    store: &SqliteStore,
    session_id: &str,
) -> Result<(Option<StoredReadModel>, Vec<String>), WorkspaceUndoError> {
    let stored = store
        .load_read_model(WORKSPACE_UNDO_READ_MODEL_NAMESPACE, session_id)
        .map_err(|error| WorkspaceUndoError::Persistence(error.to_string()))?;
    let undone = stored
        .as_ref()
        .and_then(|model| serde_json::from_str::<WorkspaceUndoRegistry>(&model.payload).ok())
        .filter(|registry| {
            registry.schema == WORKSPACE_UNDO_SCHEMA && registry.session_id == session_id
        })
        .map(|registry| registry.undone)
        .unwrap_or_default();
    Ok((stored, undone))
}

fn persist_registry(
    store: &mut SqliteStore,
    session_id: &str,
    expected: Option<&StoredReadModel>,
    undone: Vec<String>,
) -> Result<bool, WorkspaceUndoError> {
    let revision = store
        .event_revision_by_metadata(&phase16_task_id(), "session_id", session_id)
        .map_err(|error| WorkspaceUndoError::Persistence(error.to_string()))?
        .latest_sequence;
    let registry = WorkspaceUndoRegistry {
        schema: WORKSPACE_UNDO_SCHEMA.to_string(),
        session_id: session_id.to_string(),
        undone,
        updated_at_ms: current_time_millis(),
    };
    let payload = serde_json::to_string(&registry)
        .map_err(|error| WorkspaceUndoError::Persistence(error.to_string()))?;
    store
        .compare_exchange_read_model(
            WORKSPACE_UNDO_READ_MODEL_NAMESPACE,
            session_id,
            expected,
            revision,
            &payload,
        )
        .map_err(|error| WorkspaceUndoError::Persistence(error.to_string()))
}

fn sha256_file(path: &Path) -> Result<String, WorkspaceUndoError> {
    let bytes = fs::read(path)
        .map_err(|error| WorkspaceUndoError::Io(format!("failed to read {path:?}: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Project an entry to its per-file changes: one file for single-file tools,
/// the recorded group for `file.patch_batch`.
fn entry_files(entry: &WorkspaceUndoEntry) -> Vec<WorkspaceUndoFile> {
    if entry.files.is_empty() {
        return vec![WorkspaceUndoFile {
            path: entry.path.clone(),
            undo_before_path: entry.undo_before_path.clone(),
            after_artifact_path: entry.after_artifact_path.clone(),
        }];
    }
    entry.files.clone()
}

fn file_target(workspace_root: &Path, file: &WorkspaceUndoFile) -> std::path::PathBuf {
    workspace_root.join(&file.path)
}

fn file_undo_snapshot(
    workspace_root: &Path,
    file: &WorkspaceUndoFile,
) -> Option<std::path::PathBuf> {
    file.undo_before_path
        .as_ref()
        .map(|relative| workspace_root.join(relative))
}

fn file_after_artifact(
    workspace_root: &Path,
    file: &WorkspaceUndoFile,
) -> Result<std::path::PathBuf, WorkspaceUndoError> {
    file.after_artifact_path
        .as_ref()
        .map(|relative| workspace_root.join(relative))
        .ok_or_else(|| {
            WorkspaceUndoError::MissingArtifact(
                "This change has no preserved after-state snapshot, so it cannot be replayed safely."
                    .to_string(),
            )
        })
}

pub(crate) fn entry_is_undoable(workspace_root: &Path, entry: &WorkspaceUndoEntry) -> bool {
    entry_files(entry).iter().all(|file| {
        if entry.action == "created" {
            return file.after_artifact_path.is_some();
        }
        file_undo_snapshot(workspace_root, file)
            .map(|snapshot| snapshot.is_file())
            .unwrap_or(false)
            && file.after_artifact_path.is_some()
    })
}

fn require_applied_file_state(
    workspace_root: &Path,
    file: &WorkspaceUndoFile,
) -> Result<(), WorkspaceUndoError> {
    let target = file_target(workspace_root, file);
    let artifact = file_after_artifact(workspace_root, file)?;
    if !target.is_file() {
        return Err(WorkspaceUndoError::Conflict(format!(
            "{} is missing, so the recorded change no longer applies.",
            file.path
        )));
    }
    if !artifact.is_file() {
        return Err(WorkspaceUndoError::MissingArtifact(format!(
            "The after-state snapshot for {} is missing.",
            file.path
        )));
    }
    let current = sha256_file(&target)?;
    let expected = sha256_file(&artifact)?;
    if current != expected {
        return Err(WorkspaceUndoError::Conflict(format!(
            "{} changed outside this run, so undoing the recorded change is blocked.",
            file.path
        )));
    }
    Ok(())
}

fn apply_undo(workspace_root: &Path, entry: &WorkspaceUndoEntry) -> Result<(), WorkspaceUndoError> {
    if !entry_is_undoable(workspace_root, entry) {
        return Err(WorkspaceUndoError::Unsupported(format!(
            "The change to {} has no undo snapshot and cannot be undone.",
            entry.path
        )));
    }
    let files = entry_files(entry);
    // Validate the whole group before restoring any file.
    for file in &files {
        require_applied_file_state(workspace_root, file)?;
    }
    for file in &files {
        let target = file_target(workspace_root, file);
        if entry.action == "created" {
            fs::remove_file(&target).map_err(|error| {
                WorkspaceUndoError::Io(format!("failed to remove {target:?}: {error}"))
            })?;
            continue;
        }
        let snapshot = file_undo_snapshot(workspace_root, file).ok_or_else(|| {
            WorkspaceUndoError::Unsupported(format!(
                "The change to {} has no undo snapshot and cannot be undone.",
                file.path
            ))
        })?;
        if !snapshot.is_file() {
            return Err(WorkspaceUndoError::MissingArtifact(format!(
                "The undo snapshot for {} is missing.",
                file.path
            )));
        }
        let before = fs::read(&snapshot).map_err(|error| {
            WorkspaceUndoError::Io(format!("failed to read {snapshot:?}: {error}"))
        })?;
        fs::write(&target, &before).map_err(|error| {
            WorkspaceUndoError::Io(format!("failed to restore {target:?}: {error}"))
        })?;
    }
    Ok(())
}

fn apply_redo(workspace_root: &Path, entry: &WorkspaceUndoEntry) -> Result<(), WorkspaceUndoError> {
    let files = entry_files(entry);
    // Validate the whole group before replaying any file.
    for file in &files {
        let artifact = file_after_artifact(workspace_root, file)?;
        if !artifact.is_file() {
            return Err(WorkspaceUndoError::MissingArtifact(format!(
                "The after-state snapshot for {} is missing.",
                file.path
            )));
        }
        let target = file_target(workspace_root, file);
        if entry.action == "created" {
            if target.exists() {
                return Err(WorkspaceUndoError::Conflict(format!(
                    "{} exists again, so redoing the recorded creation is blocked.",
                    file.path
                )));
            }
        } else {
            let snapshot = file_undo_snapshot(workspace_root, file).ok_or_else(|| {
                WorkspaceUndoError::Unsupported(format!(
                    "The change to {} has no undo snapshot and cannot be redone.",
                    file.path
                ))
            })?;
            if !target.is_file() || !snapshot.is_file() {
                return Err(WorkspaceUndoError::Conflict(format!(
                    "{} no longer matches the undone state, so redo is blocked.",
                    file.path
                )));
            }
            let current = sha256_file(&target)?;
            let expected = sha256_file(&snapshot)?;
            if current != expected {
                return Err(WorkspaceUndoError::Conflict(format!(
                    "{} changed outside this run, so redo is blocked.",
                    file.path
                )));
            }
        }
    }
    for file in &files {
        let artifact = file_after_artifact(workspace_root, file)?;
        let after = fs::read(&artifact).map_err(|error| {
            WorkspaceUndoError::Io(format!("failed to read {artifact:?}: {error}"))
        })?;
        let target = file_target(workspace_root, file);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                WorkspaceUndoError::Io(format!("failed to create {parent:?}: {error}"))
            })?;
        }
        fs::write(&target, &after).map_err(|error| {
            WorkspaceUndoError::Io(format!("failed to write {target:?}: {error}"))
        })?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceUndoEntryView {
    pub(crate) tool_call_id: String,
    pub(crate) sequence: u64,
    pub(crate) tool: String,
    pub(crate) path: String,
    pub(crate) action: String,
    pub(crate) undone: bool,
    pub(crate) undoable: bool,
    pub(crate) run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceUndoState {
    pub(crate) session_id: String,
    pub(crate) entries: Vec<WorkspaceUndoEntryView>,
    pub(crate) can_undo: bool,
    pub(crate) can_redo: bool,
}

fn build_state(
    workspace_root: &Path,
    session_id: &str,
    entries: &[WorkspaceUndoEntry],
    undone: &[String],
) -> WorkspaceUndoState {
    let views = entries
        .iter()
        .map(|entry| WorkspaceUndoEntryView {
            tool_call_id: entry.tool_call_id.clone(),
            sequence: entry.sequence,
            tool: entry.tool.clone(),
            path: entry.path.clone(),
            action: entry.action.clone(),
            undone: undone.iter().any(|id| id == &entry.tool_call_id),
            undoable: entry_is_undoable(workspace_root, entry),
            run_id: entry.run_id.clone(),
        })
        .collect();
    let latest_applied = entries
        .iter()
        .rev()
        .find(|entry| !undone.iter().any(|id| id == &entry.tool_call_id));
    let can_undo = latest_applied
        .map(|entry| entry_is_undoable(workspace_root, entry))
        .unwrap_or(false);
    let can_redo = undone.last().is_some_and(|tool_call_id| {
        entries
            .iter()
            .find(|entry| &entry.tool_call_id == tool_call_id)
            .map(|entry| {
                entry_files(entry)
                    .iter()
                    .all(|file| file.after_artifact_path.is_some())
            })
            .unwrap_or(false)
    });
    WorkspaceUndoState {
        session_id: session_id.to_string(),
        entries: views,
        can_undo,
        can_redo,
    }
}

pub(crate) fn get_workspace_undo_state_for_session(
    store: &SqliteStore,
    workspace_root: &Path,
    session_id: &str,
) -> Result<WorkspaceUndoState, String> {
    let events = agent_events_for_session(store, &phase16_task_id(), Some(session_id))
        .map_err(|error| error.to_string())?;
    let entries = project_workspace_undo_entries(&events);
    let (_, undone) = load_registry(store, session_id).map_err(|error| error.message())?;
    Ok(build_state(workspace_root, session_id, &entries, &undone))
}

fn change_undo_stack(
    store: &mut SqliteStore,
    workspace_root: &Path,
    session_id: &str,
    redo: bool,
) -> Result<WorkspaceUndoState, String> {
    for _attempt in 0..3 {
        let events = agent_events_for_session(store, &phase16_task_id(), Some(session_id))
            .map_err(|error| error.to_string())?;
        let entries = project_workspace_undo_entries(&events);
        let (stored, undone) = load_registry(store, session_id).map_err(|error| error.message())?;
        let result = if redo {
            let Some(tool_call_id) = undone.last().cloned() else {
                return Err(WorkspaceUndoError::NothingToRedo.message());
            };
            let entry = entries
                .iter()
                .find(|entry| entry.tool_call_id == tool_call_id)
                .ok_or_else(|| {
                    WorkspaceUndoError::Conflict(
                        "The recorded redo target no longer exists in this session.".to_string(),
                    )
                    .message()
                })?;
            apply_redo(workspace_root, entry).map(|()| {
                let mut next = undone.clone();
                next.pop();
                next
            })
        } else {
            let entry = entries
                .iter()
                .rev()
                .find(|entry| !undone.iter().any(|id| id == &entry.tool_call_id))
                .ok_or_else(|| WorkspaceUndoError::NothingToUndo.message())?;
            apply_undo(workspace_root, entry).map(|()| {
                let mut next = undone.clone();
                next.push(entry.tool_call_id.clone());
                next
            })
        };
        let next_undone = match result {
            Ok(next) => next,
            Err(error) => return Err(error.message()),
        };
        let committed = persist_registry(store, session_id, stored.as_ref(), next_undone.clone())
            .map_err(|error| error.message())?;
        if committed {
            return Ok(build_state(
                workspace_root,
                session_id,
                &entries,
                &next_undone,
            ));
        }
    }
    Err(WorkspaceUndoError::Persistence(
        "The undo registry could not be committed because its revision changed concurrently."
            .to_string(),
    )
    .message())
}

/// Scans the session's events and stats every undo entry's target file.
///
/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole read (P2-05).
#[tauri::command]
pub(crate) async fn get_workspace_undo_state(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<WorkspaceUndoState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        get_workspace_undo_state_blocking(state, session_id)
    })
    .await
    .map_err(|error| format!("workspace undo state failed to join: {error}"))?
}

fn get_workspace_undo_state_blocking(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<WorkspaceUndoState, String> {
    let root = active_workspace_root(&state)?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    get_workspace_undo_state_for_session(&store, &root, &session_id)
}

#[tauri::command]
pub(crate) fn undo_workspace_change(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<WorkspaceUndoState, String> {
    let root = active_workspace_root(&state)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    change_undo_stack(&mut store, &root, &session_id, false)
}

#[tauri::command]
pub(crate) fn redo_workspace_change(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<WorkspaceUndoState, String> {
    let root = active_workspace_root(&state)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    change_undo_stack(&mut store, &root, &session_id, true)
}

#[cfg(test)]
#[path = "workspace_undo_runtime_tests.rs"]
mod tests;
