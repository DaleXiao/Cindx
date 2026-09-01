mod file_batch_projection;
mod file_batch_request;

#[cfg(test)]
mod file_batch_evidence_tests;
#[cfg(test)]
mod file_batch_tests;

use std::path::PathBuf;

use agent_core::{
    PermissionRequest, PostconditionVerifierKind, ToolEffectSemantics, ToolExecutionConcurrency,
    ToolInvocation, ToolOutcomeStatus, ToolPostconditionEvidence, ToolResult, ToolRisk, ToolSpec,
};

use super::{ReadFileTool, Tool, ToolError, ToolExecutionControl};
use file_batch_projection::BatchResultProjection;
use file_batch_request::{
    parse_request, DEFAULT_BYTES_PER_FILE, MAX_BATCH_PATHS, MAX_BYTES_PER_FILE,
};

pub struct ReadFilesTool {
    workspace_root: PathBuf,
}

impl ReadFilesTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    fn execute_batch(
        &self,
        invocation: ToolInvocation,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<agent_core::ToolResult, ToolError> {
        let request = parse_request(&invocation.input_json)?;
        let reader = ReadFileTool::new(self.workspace_root.clone());
        let mut projection =
            BatchResultProjection::new(request.paths.len(), request.max_bytes_per_file);

        for (index, path) in request.paths.iter().enumerate() {
            if should_cancel() {
                projection.cancel_remaining(&request.paths[index..]);
                break;
            }

            let mut child = invocation.clone();
            child.tool_name = "file.read".to_string();
            child.input_json = serde_json::json!({
                "path": path.path,
                "offset_bytes": path.offset_bytes,
                "max_bytes": request.max_bytes_per_file
            })
            .to_string();
            match reader.execute(child) {
                Ok(result) if matches!(result.status, ToolOutcomeStatus::Succeeded) => {
                    projection.record_success(path, &result);
                }
                Ok(result) => projection.record_non_success(path, &result),
                Err(error) => projection.record_error(path, &error),
            }

            if should_cancel() && index + 1 < request.paths.len() {
                projection.cancel_remaining(&request.paths[index + 1..]);
                break;
            }
        }

        Ok(projection.into_result(invocation))
    }
}

impl Tool for ReadFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::builtin(
            "file.read_many",
            "file",
            "Read several bounded UTF-8 files from the workspace in one evidence call. Each path may carry its own continuation offset.",
            ToolRisk::ReadOnly,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                { "type": "string", "minLength": 1 },
                                {
                                    "type": "object",
                                    "properties": {
                                        "path": { "type": "string", "minLength": 1 },
                                        "offset_bytes": { "type": "integer", "minimum": 0, "default": 0 }
                                    },
                                    "required": ["path"],
                                    "additionalProperties": false
                                }
                            ]
                        },
                        "minItems": 1,
                        "maxItems": MAX_BATCH_PATHS,
                        "description": "Workspace-relative file paths or per-file continuation requests"
                    },
                    "max_bytes_per_file": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_BYTES_PER_FILE,
                        "default": DEFAULT_BYTES_PER_FILE,
                        "description": "Maximum UTF-8 bytes returned for each file"
                    }
                },
                "required": ["paths"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .with_output_schema(
            serde_json::json!({
                "type": "object",
                "properties": {
                    "schema": { "const": "cindx.file-read-many-result.v2" },
                    "paths_requested": { "type": "integer", "minimum": 1 },
                    "paths_read": { "type": "integer", "minimum": 0 },
                    "paths_failed": { "type": "integer", "minimum": 0 },
                    "paths_truncated": { "type": "integer", "minimum": 0 },
                    "max_bytes_per_file": { "type": "integer", "minimum": 1 },
                    "complete": { "type": "boolean" },
                    "cancelled": { "type": "boolean" },
                    "items": { "type": "array" },
                    "continuations": { "type": "array" }
                },
                "required": ["schema", "paths_requested", "paths_read", "paths_failed", "paths_truncated", "max_bytes_per_file", "complete", "cancelled", "items", "continuations"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceExactReadbackV1)
        .with_effect_semantics(ToolEffectSemantics::ReadOnly)
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
        exact_readback_evidence(invocation, result)
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<agent_core::ToolResult, ToolError> {
        self.execute_batch(invocation, &|| false)
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<agent_core::ToolResult, ToolError> {
        self.execute_batch(invocation, &|| control.should_cancel())
    }
}

fn exact_readback_evidence(
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Option<ToolPostconditionEvidence> {
    if result.invocation_id != invocation.id
        || !matches!(result.status, ToolOutcomeStatus::Succeeded)
    {
        return None;
    }
    let request = parse_request(&invocation.input_json).ok()?;
    if request.paths.iter().any(|path| path.offset_bytes != 0) {
        return None;
    }
    let structured =
        serde_json::from_str::<serde_json::Value>(result.structured_output_json.as_deref()?)
            .ok()?;
    let items = structured.get("items")?.as_array()?;
    if structured.get("schema")?.as_str()? != "cindx.file-read-many-result.v2"
        || !structured.get("complete")?.as_bool()?
        || structured.get("cancelled")?.as_bool()?
        || structured.get("paths_requested")?.as_u64()? != request.paths.len() as u64
        || structured.get("paths_read")?.as_u64()? != request.paths.len() as u64
        || structured.get("paths_failed")?.as_u64()? != 0
        || structured.get("paths_truncated")?.as_u64()? != 0
        || items.len() != request.paths.len()
    {
        return None;
    }
    for (requested, item) in request.paths.iter().zip(items) {
        if item.get("path")?.as_str()? != requested.path
            || item.get("status")?.as_str()? != "succeeded"
            || item.get("requested_offset_bytes")?.as_u64()? != 0
            || item.get("offset_bytes")?.as_u64()? != 0
            || item.get("truncated")?.as_bool()?
            || item.get("returned_bytes")?.as_u64()? != item.get("total_bytes")?.as_u64()?
        {
            return None;
        }
    }
    Some(ToolPostconditionEvidence {
        kind: PostconditionVerifierKind::WorkspaceExactReadbackV1,
        target_input_json: serde_json::json!({
            "paths": request.paths.iter().map(|path| &path.path).collect::<Vec<_>>()
        })
        .to_string(),
    })
}
