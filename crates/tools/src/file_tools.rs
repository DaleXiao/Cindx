use super::tool_contract_v2::{file_read_observation, file_read_spec, FILE_READ_RESULT_SCHEMA};
use super::workspace_file::{sha256_bytes, sha256_file_bounded};
use super::{
    builtin_tool_spec, input_value_is_true, parse_bounded_usize_input, parse_input,
    permission_request, required_input, resolve_workspace_path, resolve_workspace_read_path,
    stable_hash, tool_result, Tool, ToolError,
};
use agent_core::{
    Metadata, PermissionRequest, PermissionRisk, PostconditionVerifierKind, ToolEffectSemantics,
    ToolExecutionConcurrency, ToolInvocation, ToolOutcomeStatus, ToolPostconditionEvidence,
    ToolResult, ToolRisk, ToolSpec,
};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

const DEFAULT_FILE_READ_BYTES: usize = 128 * 1024;
const MAX_FILE_READ_BYTES: usize = 256 * 1024;
const MODEL_FILE_READ_BYTES: usize = 5 * 1024;
const MAX_FILE_HASH_BYTES: usize = 8 * 1024 * 1024;

pub struct ReadFileTool {
    workspace_root: PathBuf,
}

impl ReadFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        file_read_spec(DEFAULT_FILE_READ_BYTES, MAX_FILE_READ_BYTES)
            .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceExactReadbackV1)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn postcondition_evidence(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> Option<ToolPostconditionEvidence> {
        crate::postcondition_evidence::file_read(invocation, result)
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let path = required_input(&input, "path")?;
        let offset_bytes = parse_bounded_usize_input(&input, "offset_bytes", 0, 0, usize::MAX)?;
        let max_bytes = parse_bounded_usize_input(
            &input,
            "max_bytes",
            DEFAULT_FILE_READ_BYTES,
            1,
            MAX_FILE_READ_BYTES,
        )?;
        let include_sha256 = input
            .get("include_sha256")
            .is_some_and(|value| input_value_is_true(value));
        let resolved = resolve_workspace_path(&self.workspace_root, &path)?;
        let resolved = resolve_workspace_read_path(&self.workspace_root, &resolved)?;
        reject_sensitive_read_path(&self.workspace_root, &resolved)?;
        let mut file = fs::File::open(&resolved)
            .map_err(|error| ToolError::new(format!("failed to read file: {error}")))?;
        let total_bytes = file
            .metadata()
            .map_err(|error| ToolError::new(format!("failed to inspect file: {error}")))?
            .len();
        let offset = (offset_bytes as u64).min(total_bytes);
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| ToolError::new(format!("failed to seek file: {error}")))?;
        let mut bytes = Vec::with_capacity(max_bytes.saturating_add(4));
        file.take(max_bytes.saturating_add(4) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| ToolError::new(format!("failed to read file range: {error}")))?;
        let skipped_prefix = bytes
            .iter()
            .take(3)
            .take_while(|byte| **byte & 0b1100_0000 == 0b1000_0000)
            .count();
        if skipped_prefix > 0 {
            bytes.drain(..skipped_prefix);
        }
        let offset = offset.saturating_add(skipped_prefix as u64);
        let end = utf8_page_end(&bytes, max_bytes);
        bytes.truncate(end);
        let returned_bytes = bytes.len();
        let next_offset = offset.saturating_add(returned_bytes as u64);
        let truncated = next_offset < total_bytes;
        let model_end = utf8_page_end(&bytes, MODEL_FILE_READ_BYTES.min(bytes.len()));
        let model_returned_bytes = model_end;
        let model_next_offset = offset.saturating_add(model_returned_bytes as u64);
        let model_evidence = String::from_utf8_lossy(&bytes[..model_end]).to_string();
        let page_sha256 = sha256_bytes(&bytes);
        let sha256 = if offset == 0 && !truncated && returned_bytes as u64 == total_bytes {
            Some(page_sha256.clone())
        } else if include_sha256 {
            sha256_file_bounded(&resolved, MAX_FILE_HASH_BYTES)
                .map_err(|error| ToolError::new(format!("failed to hash file: {error}")))?
        } else {
            None
        };
        let mut output = String::from_utf8_lossy(&bytes).to_string();
        if truncated {
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!(
                "\n[File read bounded at {returned_bytes} bytes. Continue with offset_bytes={next_offset}. Total file size: {total_bytes} bytes.]"
            ));
        }
        let mut metadata = Metadata::new();
        metadata.insert("path".to_string(), path.clone());
        metadata.insert("bytes".to_string(), total_bytes.to_string());
        metadata.insert("offset_bytes".to_string(), offset.to_string());
        metadata.insert("returned_bytes".to_string(), returned_bytes.to_string());
        metadata.insert("next_offset_bytes".to_string(), next_offset.to_string());
        metadata.insert("truncated".to_string(), truncated.to_string());
        metadata.insert("page_sha256".to_string(), page_sha256.clone());
        if let Some(sha256) = sha256.as_ref() {
            metadata.insert("sha256".to_string(), sha256.clone());
        }

        let structured_output = serde_json::json!({
            "schema": FILE_READ_RESULT_SCHEMA,
            "path": path,
            "offset_bytes": offset,
            "returned_bytes": returned_bytes,
            "next_offset_bytes": next_offset,
            "total_bytes": total_bytes,
            "truncated": truncated,
            "page_sha256": page_sha256,
            "sha256": sha256,
        });
        let mut result = tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            output,
            metadata,
        );
        result.structured_output_json = Some(structured_output.to_string());
        result.model_observation = Some(file_read_observation(
            &path,
            offset,
            model_returned_bytes,
            model_next_offset,
            total_bytes,
            (
                structured_output["page_sha256"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                structured_output["sha256"].as_str().map(str::to_string),
            ),
            model_evidence,
        ));
        Ok(result)
    }
}

fn utf8_page_end(bytes: &[u8], max_bytes: usize) -> usize {
    let candidate = bytes.len().min(max_bytes);
    match std::str::from_utf8(&bytes[..candidate]) {
        Ok(_) => candidate,
        Err(error) if error.error_len().is_none() => {
            let valid = error.valid_up_to();
            if valid > 0 {
                return valid;
            }
            let width = utf8_sequence_width(bytes.first().copied().unwrap_or_default());
            if width > 1 && bytes.len() >= width && std::str::from_utf8(&bytes[..width]).is_ok() {
                width
            } else {
                candidate
            }
        }
        Err(_) => candidate,
    }
}

fn utf8_sequence_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
    }
}

pub struct WriteFileTool {
    workspace_root: PathBuf,
}

impl WriteFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "file.write",
            "Write UTF-8 content to a file inside the workspace.",
            ToolRisk::WritesWorkspace,
            "path=<workspace-relative-path>\ncontent=<utf-8 content>",
        )
        .with_effect_semantics(ToolEffectSemantics::Verifiable {
            verifier: "workspace_file_content_v1".to_string(),
        })
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let path = input
            .get("path")
            .cloned()
            .unwrap_or_else(|| "<missing path>".to_string());
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Write,
            "file.write",
            "Write a file in the selected workspace.",
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
        let input = parse_input(&invocation.input_json);
        let path = required_input(&input, "path")?;
        let content = required_input(&input, "content")?;
        let resolved = resolve_workspace_path(&self.workspace_root, &path)?;
        let session_key = invocation
            .metadata
            .get("session_id")
            .map(|session_id| stable_hash(session_id).to_string())
            .unwrap_or_else(|| "unscoped".to_string());
        let version_key = stable_hash(&invocation.id.0).to_string();
        let snapshot_relative = PathBuf::from(".cindx")
            .join("output-history")
            .join(session_key)
            .join(version_key)
            .join(&path);
        let snapshot = self.workspace_root.join(&snapshot_relative);
        if let Some(parent) = snapshot.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolError::new(format!(
                    "failed to create output history directory: {error}"
                ))
            })?;
        }
        fs::write(&snapshot, content.as_bytes()).map_err(|error| {
            ToolError::new(format!("failed to preserve output version: {error}"))
        })?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolError::new(format!("failed to create parent directory: {error}"))
            })?;
        }
        fs::write(&resolved, content.as_bytes())
            .map_err(|error| ToolError::new(format!("failed to write file: {error}")))?;

        let mut metadata = Metadata::new();
        metadata.insert("path".to_string(), path);
        metadata.insert("source_path".to_string(), resolved.display().to_string());
        metadata.insert(
            "artifact_path".to_string(),
            snapshot_relative.display().to_string(),
        );
        metadata.insert("bytes".to_string(), content.len().to_string());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            "file written".to_string(),
            metadata,
        ))
    }
}

pub(super) fn reject_sensitive_read_path(
    workspace_root: &Path,
    path: &Path,
) -> Result<(), ToolError> {
    if is_sensitive_workspace_path(workspace_root, path) {
        Err(ToolError::new(
            "access to local credential files is blocked; configure providers in Settings",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn is_sensitive_workspace_path(workspace_root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(workspace_root).unwrap_or(path);
    let normalized = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let joined = normalized.join("/");
    if joined == ".cindx/provider.conf" || joined.ends_with("/.cindx/provider.conf") {
        return true;
    }

    let Some(file_name) = normalized.last().map(String::as_str) else {
        return false;
    };
    let is_env_file = (file_name == ".env" || file_name.starts_with(".env."))
        && !file_name.ends_with(".example")
        && !file_name.ends_with(".sample")
        && !file_name.ends_with(".template");
    is_env_file
        || matches!(
            file_name,
            ".npmrc" | ".pypirc" | "credentials" | "credentials.json" | "id_rsa" | "id_ed25519"
        )
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
}
