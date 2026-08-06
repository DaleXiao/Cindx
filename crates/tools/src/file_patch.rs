mod contract;
mod receipt;

#[cfg(test)]
mod tests;

use self::contract::{input_path, patch_spec, PatchIssue, PatchPlan, PatchRequest};
use self::receipt::{failed_patch_result, successful_patch_result};
#[cfg(test)]
use super::workspace_file::atomic_replace_preserving_permissions_if_sha256_with;
use super::workspace_file::{
    atomic_replace_preserving_permissions_if_sha256, sha256_bytes, write_immutable_file_atomically,
    AtomicReplaceError,
};
use super::{
    permission_request, resolve_workspace_path, resolve_workspace_read_path, stable_hash, Tool,
    ToolError,
};
use agent_core::{
    PermissionRequest, PermissionRisk, ToolEffectSemantics, ToolInvocation, ToolResult, ToolSpec,
};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub struct PatchFileTool {
    workspace_root: PathBuf,
}

impl PatchFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    fn prepare(&self, input_json: &str) -> Result<PreparedPatch, PatchIssue> {
        let request = PatchRequest::parse(input_json)?;
        let target = resolve_existing_target(&self.workspace_root, &request.path)?;
        let base = read_utf8_base(&target)?;
        let observed_sha256 = sha256_bytes(base.as_bytes());
        if observed_sha256 != request.expected_base_sha256 {
            return Err(PatchIssue::retryable(
                "file_patch_stale_base",
                "The workspace file no longer matches expected_base_sha256.",
            )
            .with_observed_sha256(observed_sha256));
        }
        let plan = PatchPlan::build(&base, &request)?;
        Ok(PreparedPatch {
            request,
            target,
            plan,
        })
    }

    fn finish_publish(
        &self,
        invocation: ToolInvocation,
        prepared: PreparedPatch,
        result: Result<(), AtomicReplaceError>,
    ) -> Result<ToolResult, ToolError> {
        match result {
            Ok(()) => {
                let snapshot = preserve_output_version(&self.workspace_root, &invocation, &prepared);
                let mut result = successful_patch_result(
                    invocation,
                    &prepared.request.path,
                    &prepared.target,
                    prepared.plan,
                );
                match snapshot {
                    Ok(path) => {
                        result
                            .metadata
                            .insert("artifact_path".to_string(), path.display().to_string());
                        result
                            .metadata
                            .insert("snapshot_status".to_string(), "written".to_string());
                    }
                    Err(()) => {
                        result
                            .metadata
                            .insert("snapshot_status".to_string(), "failed".to_string());
                        result.metadata.insert(
                            "snapshot_error_code".to_string(),
                            "output_history_unavailable".to_string(),
                        );
                    }
                }
                Ok(result)
            }
            Err(AtomicReplaceError::Conflict { observed_sha256 }) => {
                let mut issue = PatchIssue::retryable(
                    "file_patch_race_conflict",
                    "The workspace file changed immediately before the atomic patch was published.",
                );
                if let Some(observed_sha256) = observed_sha256 {
                    issue = issue.with_observed_sha256(observed_sha256);
                }
                Ok(failed_patch_result(
                    invocation,
                    Some(&prepared.request.path),
                    issue,
                ))
            }
            Err(AtomicReplaceError::Io) => Ok(failed_patch_result(
                invocation,
                Some(&prepared.request.path),
                PatchIssue::retryable(
                    "file_patch_publish_failed",
                    "The atomic workspace patch could not be published; the prior file was preserved when publication did not occur.",
                ),
            )),
        }
    }

    #[cfg(test)]
    fn execute_with_hooks<BeforeRecheck, Publish>(
        &self,
        invocation: ToolInvocation,
        before_recheck: BeforeRecheck,
        publish: Publish,
    ) -> Result<ToolResult, ToolError>
    where
        BeforeRecheck: FnOnce(&Path) -> Result<(), io::Error>,
        Publish: FnOnce(tempfile::NamedTempFile, &Path) -> Result<(), io::Error>,
    {
        let path = input_path(&invocation.input_json);
        let prepared = match self.prepare(&invocation.input_json) {
            Ok(prepared) => prepared,
            Err(issue) => return Ok(failed_patch_result(invocation, path.as_deref(), issue)),
        };
        let result = atomic_replace_preserving_permissions_if_sha256_with(
            &prepared.target,
            &prepared.plan.after,
            &prepared.plan.before_sha256,
            contract::MAX_PATCH_FILE_BYTES,
            before_recheck,
            publish,
        );
        self.finish_publish(invocation, prepared, result)
    }
}

impl Tool for PatchFileTool {
    fn spec(&self) -> ToolSpec {
        patch_spec()
    }

    fn effect_spec(&self, invocation: &ToolInvocation) -> ToolSpec {
        let spec = patch_spec();
        match self.prepare(&invocation.input_json) {
            Ok(prepared) => spec.with_effect_semantics(ToolEffectSemantics::Verifiable {
                verifier: format!("workspace_file_sha256_v1:{}", prepared.plan.after_sha256),
            }),
            Err(_) => spec,
        }
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let path = input_path(&invocation.input_json)
            .filter(|path| !path.trim().is_empty())
            .unwrap_or_else(|| "<missing path>".to_string());
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Write,
            "file.patch",
            "Atomically patch an existing file in the selected workspace.",
            &path,
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let path = input_path(&invocation.input_json);
        let prepared = match self.prepare(&invocation.input_json) {
            Ok(prepared) => prepared,
            Err(issue) => return Ok(failed_patch_result(invocation, path.as_deref(), issue)),
        };
        let result = atomic_replace_preserving_permissions_if_sha256(
            &prepared.target,
            &prepared.plan.after,
            &prepared.plan.before_sha256,
            contract::MAX_PATCH_FILE_BYTES,
        );
        self.finish_publish(invocation, prepared, result)
    }
}

struct PreparedPatch {
    request: PatchRequest,
    target: PathBuf,
    plan: PatchPlan,
}

fn resolve_existing_target(workspace_root: &Path, path: &str) -> Result<PathBuf, PatchIssue> {
    let unresolved = resolve_workspace_path(workspace_root, path).map_err(|_| {
        PatchIssue::new(
            "file_patch_invalid_path",
            "The requested path is outside the selected workspace.",
        )
    })?;
    match fs::metadata(&unresolved) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(PatchIssue::new(
                "file_patch_target_not_file",
                "file.patch requires an existing regular file.",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(PatchIssue::new(
                "file_patch_target_missing",
                "file.patch requires an existing file; the requested path does not exist.",
            ))
        }
        Err(_) => {
            return Err(PatchIssue::retryable(
                "file_patch_target_unreadable",
                "The requested workspace file could not be inspected.",
            ))
        }
    }
    resolve_workspace_read_path(workspace_root, &unresolved).map_err(|_| {
        PatchIssue::new(
            "file_patch_invalid_path",
            "The requested path cannot be patched through a workspace-escaping symbolic link.",
        )
    })
}

fn read_utf8_base(path: &Path) -> Result<String, PatchIssue> {
    let metadata = fs::metadata(path).map_err(|_| {
        PatchIssue::retryable(
            "file_patch_target_unreadable",
            "The requested workspace file could not be inspected.",
        )
    })?;
    if metadata.len() > contract::MAX_PATCH_FILE_BYTES as u64 {
        return Err(PatchIssue::new(
            "file_patch_target_too_large",
            "file.patch accepts files no larger than 8 MiB.",
        ));
    }
    let file = fs::File::open(path).map_err(|_| {
        PatchIssue::retryable(
            "file_patch_target_unreadable",
            "The requested workspace file could not be opened.",
        )
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((contract::MAX_PATCH_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            PatchIssue::retryable(
                "file_patch_target_unreadable",
                "The requested workspace file could not be read.",
            )
        })?;
    if bytes.len() > contract::MAX_PATCH_FILE_BYTES {
        return Err(PatchIssue::new(
            "file_patch_target_too_large",
            "file.patch accepts files no larger than 8 MiB.",
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        PatchIssue::new(
            "file_patch_target_not_utf8",
            "file.patch accepts only valid UTF-8 files.",
        )
    })
}

fn preserve_output_version(
    workspace_root: &Path,
    invocation: &ToolInvocation,
    prepared: &PreparedPatch,
) -> Result<PathBuf, ()> {
    let session_key = invocation
        .metadata
        .get("session_id")
        .map(|session_id| stable_hash(session_id).to_string())
        .unwrap_or_else(|| "unscoped".to_string());
    let version_key = stable_hash(&invocation.id.0).to_string();
    let relative = PathBuf::from(".cindx")
        .join("output-history")
        .join(session_key)
        .join(version_key)
        .join(&prepared.request.path);
    let snapshot =
        resolve_workspace_path(workspace_root, &relative.to_string_lossy()).map_err(|_| ())?;
    write_immutable_file_atomically(&snapshot, &prepared.plan.after).map_err(|_| ())?;
    Ok(relative)
}
