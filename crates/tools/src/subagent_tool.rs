use crate::{Tool, ToolError, ToolInvocation, ToolResult};
use agent_core::{PermissionRequest, ToolExposure, ToolRisk, ToolSource, ToolSpec};

const TASK_SCHEMA: &str = r#"{"type":"object","properties":{"description":{"type":"string"},"prompt":{"type":"string"},"allow_patches":{"type":"boolean"}},"required":["description"]}"#;

/// Delegates a sub-task to an isolated child run. The actual child execution is
/// performed by the desktop agent loop (which owns the model provider); this spec
/// only makes the tool discoverable and well-formed.
pub struct SubagentTaskTool;

impl SubagentTaskTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SubagentTaskTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for SubagentTaskTool {
    fn spec(&self) -> ToolSpec {
        // Inline so the model emits a native `task` call the loop intercepts; it is
        // never deferred, so tool.invoke (the registry path) is never used for it.
        ToolSpec::new(
            "task",
            "agent",
            "Delegate a self-contained sub-task to an isolated subagent with its own context and read-only tools. Give a short `description` and optional `prompt` detail; the subagent returns a concise result. Prefer this for exploration or analysis that would otherwise flood your own context. Set `allow_patches: true` only when the delegation must patch existing workspace files: the subagent then also gets file.patch/file.patch_batch, and every patch call requires an explicit per-call user approval.",
            ToolRisk::ReadOnly,
            ToolSource::BuiltIn,
            ToolExposure::Inline,
            TASK_SCHEMA,
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, _invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        Err(ToolError::new(
            "task delegation must be executed by the agent loop, not the tool registry",
        ))
    }
}
