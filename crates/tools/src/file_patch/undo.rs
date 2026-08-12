use crate::workspace_file::write_immutable_file_atomically;
use crate::{resolve_workspace_path, stable_hash};
use agent_core::ToolInvocation;
use std::path::{Path, PathBuf};

pub(super) fn history_relative_path(
    invocation: &ToolInvocation,
    history_dir: &str,
    path: &str,
) -> PathBuf {
    let session_key = invocation
        .metadata
        .get("session_id")
        .map(|session_id| stable_hash(session_id).to_string())
        .unwrap_or_else(|| "unscoped".to_string());
    let version_key = stable_hash(&invocation.id.0).to_string();
    PathBuf::from(".cindx")
        .join(history_dir)
        .join(session_key)
        .join(version_key)
        .join(path)
}

pub(super) fn undo_version_relative_path(invocation: &ToolInvocation, path: &str) -> PathBuf {
    history_relative_path(invocation, "undo-history", path)
}

pub(super) fn preserve_history_version(
    workspace_root: &Path,
    invocation: &ToolInvocation,
    history_dir: &str,
    path: &str,
    bytes: &[u8],
) -> Result<PathBuf, ()> {
    let relative = history_relative_path(invocation, history_dir, path);
    let target =
        resolve_workspace_path(workspace_root, &relative.to_string_lossy()).map_err(|_| ())?;
    write_immutable_file_atomically(&target, bytes).map_err(|_| ())?;
    Ok(relative)
}
