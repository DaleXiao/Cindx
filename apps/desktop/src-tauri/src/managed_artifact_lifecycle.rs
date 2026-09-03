use agent_core::Event;
use agent_storage::SqliteStore;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

const SCALAR_FILE_PATH_KEYS: [&str; 9] = [
    "result_artifact_path",
    "result_source_path",
    "result_text_path",
    "result_trace_path",
    "result_screenshot_path",
    "result_structured_output_path",
    "result_stdout_artifact_path",
    "result_stderr_artifact_path",
    "result_redaction_manifest_path",
];
const MANAGED_ROOTS: [&str; 10] = [
    "artifacts",
    "attachments",
    "browser-actions",
    "browser-artifacts",
    "browser-captures",
    "browser-sessions",
    "computer-actions",
    "output-history",
    "screenshots",
    "tool-output",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedPathKind {
    File,
    BrowserSessionDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedPath {
    path: PathBuf,
    root: &'static str,
    kind: ManagedPathKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedArtifactRetirement {
    project_root: PathBuf,
    candidates: Vec<ManagedPath>,
    preserved_live_paths: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ManagedArtifactCleanupStats {
    pub(crate) planned_paths: usize,
    pub(crate) preserved_live_paths: usize,
    pub(crate) removed_files: usize,
    pub(crate) removed_directories: usize,
    pub(crate) already_absent: usize,
    pub(crate) removed_empty_parent_directories: usize,
}

pub(crate) fn plan_managed_artifact_retirement(
    store: &SqliteStore,
    project_root: &Path,
    deleted_session_ids: &[String],
) -> Result<ManagedArtifactRetirement, String> {
    let project_root = match fs::canonicalize(project_root) {
        Ok(project_root) => project_root,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(ManagedArtifactRetirement {
                project_root: project_root.to_path_buf(),
                candidates: Vec::new(),
                preserved_live_paths: 0,
            });
        }
        Err(error) => return Err(format!("failed to resolve artifact workspace: {error}")),
    };
    if !project_root.is_dir() {
        return Err("artifact workspace is not a directory".to_string());
    }
    if deleted_session_ids.is_empty() {
        return Ok(ManagedArtifactRetirement {
            project_root,
            candidates: Vec::new(),
            preserved_live_paths: 0,
        });
    }

    let deleted = deleted_session_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut candidates = BTreeMap::<PathBuf, ManagedPath>::new();
    let mut live = BTreeMap::<PathBuf, ManagedPath>::new();
    for event in store
        .list_all_events()
        .map_err(|error| format!("failed to load artifact references: {error}"))?
    {
        let target = if event
            .metadata
            .get("session_id")
            .is_some_and(|session_id| deleted.contains(session_id.as_str()))
        {
            &mut candidates
        } else {
            &mut live
        };
        for reference in managed_paths_for_event(&project_root, &event)? {
            insert_reference(target, reference)?;
        }
    }

    let live_paths = live.keys().collect::<Vec<_>>();
    let mut preserved_live_paths = 0usize;
    let mut candidates = candidates
        .into_values()
        .filter(|candidate| {
            let shared = live_paths
                .iter()
                .any(|live| paths_overlap(&candidate.path, live));
            if shared {
                preserved_live_paths = preserved_live_paths.saturating_add(1);
            }
            !shared
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .path
            .components()
            .count()
            .cmp(&left.path.components().count())
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(ManagedArtifactRetirement {
        project_root,
        candidates,
        preserved_live_paths,
    })
}

pub(crate) fn apply_managed_artifact_retirement(
    retirement: &ManagedArtifactRetirement,
) -> Result<ManagedArtifactCleanupStats, String> {
    for candidate in &retirement.candidates {
        validate_managed_path(&retirement.project_root, candidate)?;
        ensure_browser_session_is_inactive(&retirement.project_root, candidate)?;
    }

    let mut stats = ManagedArtifactCleanupStats {
        planned_paths: retirement.candidates.len(),
        preserved_live_paths: retirement.preserved_live_paths,
        ..ManagedArtifactCleanupStats::default()
    };
    for candidate in &retirement.candidates {
        validate_managed_path(&retirement.project_root, candidate)?;
        match fs::symlink_metadata(&candidate.path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing to remove symlinked managed artifact {}",
                    candidate.path.display()
                ));
            }
            Ok(metadata) if metadata.is_file() && candidate.kind == ManagedPathKind::File => {
                fs::remove_file(&candidate.path).map_err(|error| {
                    format!(
                        "failed to remove managed artifact {}: {error}",
                        candidate.path.display()
                    )
                })?;
                stats.removed_files = stats.removed_files.saturating_add(1);
            }
            Ok(metadata)
                if metadata.is_dir()
                    && candidate.kind == ManagedPathKind::BrowserSessionDirectory =>
            {
                remove_tree_without_following_links(&candidate.path)?;
                stats.removed_directories = stats.removed_directories.saturating_add(1);
            }
            Ok(_) => {
                return Err(format!(
                    "managed artifact type changed before cleanup: {}",
                    candidate.path.display()
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                stats.already_absent = stats.already_absent.saturating_add(1);
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "failed to inspect managed artifact {}: {error}",
                    candidate.path.display()
                ));
            }
        }
        let managed_root = retirement.project_root.join(".cindx").join(candidate.root);
        stats.removed_empty_parent_directories = stats
            .removed_empty_parent_directories
            .saturating_add(remove_empty_parents(
                candidate.path.parent(),
                &managed_root,
            )?);
    }
    Ok(stats)
}

pub(crate) fn retire_managed_browser_sessions(
    retirement: &ManagedArtifactRetirement,
) -> Result<(), String> {
    for candidate in retirement
        .candidates
        .iter()
        .filter(|candidate| candidate.root == "browser-sessions")
    {
        let relative = candidate
            .path
            .strip_prefix(&retirement.project_root)
            .map_err(|_| {
                "managed browser session path escaped the artifact workspace".to_string()
            })?;
        tools::retire_browser_session(&retirement.project_root, relative)
            .map_err(|error| format!("failed to retire browser session: {error}"))?;
    }
    Ok(())
}

fn managed_paths_for_event(project_root: &Path, event: &Event) -> Result<Vec<ManagedPath>, String> {
    let mut paths = Vec::new();
    for key in SCALAR_FILE_PATH_KEYS {
        if let Some(path) = event.metadata.get(key) {
            push_managed_path(project_root, path, ManagedPathKind::File, &mut paths)?;
        }
    }
    if let Some(path) = event.metadata.get("result_session_path") {
        push_managed_path(
            project_root,
            path,
            ManagedPathKind::BrowserSessionDirectory,
            &mut paths,
        )?;
    }
    for key in ["attachment_paths", "image_paths"] {
        if let Some(values) = event.metadata.get(key) {
            for path in values.lines() {
                push_managed_path(project_root, path, ManagedPathKind::File, &mut paths)?;
            }
        }
    }
    if let Some(encoded) = event.metadata.get("result_artifacts_json") {
        if let Ok(Value::Array(artifacts)) = serde_json::from_str::<Value>(encoded) {
            for path in artifacts
                .iter()
                .filter_map(|artifact| artifact.get("path"))
                .filter_map(Value::as_str)
            {
                push_managed_path(project_root, path, ManagedPathKind::File, &mut paths)?;
            }
        }
    }
    if let Some(encoded) = event.metadata.get("queue_payload") {
        if let Ok(payload) = serde_json::from_str::<Value>(encoded) {
            for path in payload
                .get("attachments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|attachment| attachment.get("path"))
                .filter_map(Value::as_str)
            {
                push_managed_path(project_root, path, ManagedPathKind::File, &mut paths)?;
            }
        }
    }
    Ok(paths)
}

fn push_managed_path(
    project_root: &Path,
    value: &str,
    kind: ManagedPathKind,
    paths: &mut Vec<ManagedPath>,
) -> Result<(), String> {
    if let Some(path) = resolve_managed_path(project_root, value, kind)? {
        paths.push(path);
    }
    Ok(())
}

fn resolve_managed_path(
    project_root: &Path,
    value: &str,
    kind: ManagedPathKind,
) -> Result<Option<ManagedPath>, String> {
    let raw = Path::new(value.trim());
    if value.trim().is_empty() {
        return Ok(None);
    }
    let canonical_absolute;
    let relative = if raw.is_absolute() {
        if let Ok(relative) = raw.strip_prefix(project_root) {
            relative
        } else {
            canonical_absolute = match fs::canonicalize(raw) {
                Ok(path) => path,
                Err(_) => return Ok(None),
            };
            let Ok(relative) = canonical_absolute.strip_prefix(project_root) else {
                return Ok(None);
            };
            relative
        }
    } else {
        raw
    };
    let mut normal = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => normal.push(value.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "managed artifact path contains a parent traversal: {}",
                    raw.display()
                ));
            }
            Component::Prefix(_) | Component::RootDir => return Ok(None),
        }
    }
    if normal.first().and_then(|value| value.to_str()) != Some(".cindx") || normal.len() < 3 {
        return Ok(None);
    }
    let Some(root) = normal
        .get(1)
        .and_then(|value| value.to_str())
        .and_then(managed_root)
    else {
        return Ok(None);
    };
    if kind == ManagedPathKind::BrowserSessionDirectory
        && (root != "browser-sessions" || normal.len() != 3)
    {
        return Err("browser session path is not an exact managed session directory".to_string());
    }

    let path = normal
        .iter()
        .fold(project_root.to_path_buf(), |mut path, component| {
            path.push(component);
            path
        });
    let managed = ManagedPath { path, root, kind };
    validate_managed_path(project_root, &managed)?;
    Ok(Some(managed))
}

fn insert_reference(
    target: &mut BTreeMap<PathBuf, ManagedPath>,
    reference: ManagedPath,
) -> Result<(), String> {
    if let Some(existing) = target.get(&reference.path) {
        if existing != &reference {
            return Err(format!(
                "conflicting managed artifact reference types for {}",
                reference.path.display()
            ));
        }
        return Ok(());
    }
    target.insert(reference.path.clone(), reference);
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn managed_root(value: &str) -> Option<&'static str> {
    MANAGED_ROOTS.iter().copied().find(|root| *root == value)
}

fn validate_managed_path(project_root: &Path, managed: &ManagedPath) -> Result<(), String> {
    let cindx_root = project_root.join(".cindx");
    let managed_root = cindx_root.join(managed.root);
    if !managed.path.starts_with(&managed_root) || managed.path == managed_root {
        return Err("managed artifact path escaped its fixed root".to_string());
    }
    validate_existing_component(&cindx_root, true)?;
    validate_existing_component(&managed_root, true)?;

    let relative = managed
        .path
        .strip_prefix(&managed_root)
        .map_err(|_| "managed artifact path escaped its fixed root".to_string())?;
    let components = relative.components().collect::<Vec<_>>();
    let mut current = managed_root;
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(value) = component else {
            return Err("managed artifact path contains an unsafe component".to_string());
        };
        current.push(value);
        let final_component = index + 1 == components.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "managed artifact path crosses a symbolic link: {}",
                    current.display()
                ));
            }
            Ok(metadata) if !final_component && !metadata.is_dir() => {
                return Err(format!(
                    "managed artifact ancestor is not a directory: {}",
                    current.display()
                ));
            }
            Ok(metadata)
                if final_component
                    && managed.kind == ManagedPathKind::File
                    && !metadata.is_file() =>
            {
                return Err(format!(
                    "managed artifact is not a regular file: {}",
                    current.display()
                ));
            }
            Ok(metadata)
                if final_component
                    && managed.kind == ManagedPathKind::BrowserSessionDirectory
                    && !metadata.is_dir() =>
            {
                return Err(format!(
                    "managed browser session is not a directory: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(format!(
                    "failed to inspect managed artifact path {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_existing_component(path: &Path, must_be_directory: bool) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "managed artifact root is a symbolic link: {}",
            path.display()
        )),
        Ok(metadata) if must_be_directory && !metadata.is_dir() => Err(format!(
            "managed artifact root is not a directory: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to inspect managed artifact root {}: {error}",
            path.display()
        )),
    }
}

fn ensure_browser_session_is_inactive(
    project_root: &Path,
    candidate: &ManagedPath,
) -> Result<(), String> {
    if candidate.root != "browser-sessions" {
        return Ok(());
    }
    let sessions_root = project_root.join(".cindx").join("browser-sessions");
    let session_name = candidate
        .path
        .strip_prefix(&sessions_root)
        .ok()
        .and_then(|relative| relative.components().next())
        .and_then(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .ok_or_else(|| "managed browser session path is invalid".to_string())?;
    let session_root = sessions_root.join(session_name);
    for marker in ["session-state.json", ".action-lock.json"] {
        match fs::symlink_metadata(session_root.join(marker)) {
            Ok(_) => {
                return Err(format!(
                    "browser session is still active or locked: {}",
                    session_root.display()
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("failed to inspect browser session marker: {error}"));
            }
        }
    }
    Ok(())
}

fn remove_tree_without_following_links(path: &Path) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|error| {
        format!(
            "failed to inspect managed directory {}: {error}",
            path.display()
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "failed to inspect managed directory {}: {error}",
                path.display()
            )
        })?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|error| {
            format!(
                "failed to inspect managed artifact {}: {error}",
                child.display()
            )
        })?;
        if metadata.file_type().is_symlink() || metadata.is_file() {
            fs::remove_file(&child).map_err(|error| {
                format!(
                    "failed to remove managed artifact {}: {error}",
                    child.display()
                )
            })?;
        } else if metadata.is_dir() {
            remove_tree_without_following_links(&child)?;
        } else {
            return Err(format!(
                "refusing to remove special managed artifact {}",
                child.display()
            ));
        }
    }
    fs::remove_dir(path).map_err(|error| {
        format!(
            "failed to remove managed directory {}: {error}",
            path.display()
        )
    })
}

pub(crate) fn remove_empty_parents(
    mut current: Option<&Path>,
    stop: &Path,
) -> Result<usize, String> {
    let mut removed = 0usize;
    while let Some(path) = current {
        if path == stop || !path.starts_with(stop) {
            break;
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(format!(
                    "managed artifact parent changed during cleanup: {}",
                    path.display()
                ));
            }
            Ok(_) => match fs::remove_dir(path) {
                Ok(()) => removed = removed.saturating_add(1),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::DirectoryNotEmpty | ErrorKind::NotFound
                    ) =>
                {
                    break;
                }
                Err(error) => {
                    return Err(format!(
                        "failed to remove empty managed directory {}: {error}",
                        path.display()
                    ));
                }
            },
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to inspect managed artifact parent {}: {error}",
                    path.display()
                ));
            }
        }
        current = path.parent();
    }
    Ok(removed)
}

#[cfg(test)]
#[path = "managed_artifact_lifecycle_tests.rs"]
mod tests;
