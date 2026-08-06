use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{
    Metadata, PermissionRequest, PermissionRisk, ToolFailure, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolSpec,
};

use crate::process_contract::{
    input_projection, input_spec, poll_spec, snapshot_projection, start_projection, start_spec,
    terminate_spec, DEFAULT_PROCESS_CPU_SECONDS, DEFAULT_PROCESS_OUTPUT_BYTES,
    DEFAULT_PROCESS_POLL_BYTES, DEFAULT_PROCESS_TIMEOUT_SECONDS, MAX_PROCESS_CPU_SECONDS,
    MAX_PROCESS_INPUT_CALL_BYTES, MAX_PROCESS_OUTPUT_BYTES, MAX_PROCESS_POLL_BYTES,
    MAX_PROCESS_POLL_WAIT_MS, MAX_PROCESS_TIMEOUT_SECONDS,
};
use crate::process_runtime::{ProcessBudgets, ProcessManager, ProcessManagerError};
use crate::shell::shell_permission_request;
use crate::{
    model_observation, parse_bounded_usize_input, parse_input, permission_request, required_input,
    resolve_workspace_path, resolve_workspace_read_path, stable_hash, tool_result, Tool, ToolError,
    ToolExecutionControl,
};

pub struct ProcessStartTool {
    workspace_root: PathBuf,
    manager: Arc<ProcessManager>,
}

pub struct ProcessPollTool {
    manager: Arc<ProcessManager>,
}

pub struct ProcessInputTool {
    manager: Arc<ProcessManager>,
}

pub struct ProcessTerminateTool {
    manager: Arc<ProcessManager>,
}

impl ProcessStartTool {
    pub fn new(workspace_root: impl Into<PathBuf>, manager: Arc<ProcessManager>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            manager,
        }
    }
}

impl ProcessPollTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self { manager }
    }
}

impl ProcessInputTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self { manager }
    }
}

impl ProcessTerminateTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self { manager }
    }
}

impl Tool for ProcessStartTool {
    fn spec(&self) -> ToolSpec {
        start_spec()
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        shell_permission_request(&self.workspace_root, invocation, "process.start")
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.execute_with_control(invocation, &ToolExecutionControl::never_cancelled())
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Process reservation cancelled before it started.",
                Metadata::new(),
            ));
        }
        let input = parse_input(&invocation.input_json);
        let command = required_input(&input, "command")?;
        let cwd = input.get("cwd").cloned().unwrap_or_else(|| ".".to_string());
        let timeout_seconds = parse_u64(
            &input,
            "timeout_seconds",
            DEFAULT_PROCESS_TIMEOUT_SECONDS,
            1,
            MAX_PROCESS_TIMEOUT_SECONDS,
        )?;
        let cpu_seconds = parse_u64(
            &input,
            "cpu_seconds",
            DEFAULT_PROCESS_CPU_SECONDS.min(timeout_seconds),
            1,
            MAX_PROCESS_CPU_SECONDS.min(timeout_seconds),
        )?;
        let output_limit_bytes = parse_bounded_usize_input(
            &input,
            "output_limit_bytes",
            DEFAULT_PROCESS_OUTPUT_BYTES,
            4_096,
            MAX_PROCESS_OUTPUT_BYTES,
        )?;
        let resolved_cwd = resolve_workspace_path(&self.workspace_root, &cwd)?;
        let resolved_cwd = resolve_workspace_read_path(&self.workspace_root, &resolved_cwd)?;
        let snapshot = match self.manager.reserve(
            &invocation,
            command,
            resolved_cwd,
            cwd,
            ProcessBudgets {
                timeout_seconds,
                cpu_seconds,
                output_limit_bytes: output_limit_bytes as u64,
            },
            control.clone(),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(process_error_result(invocation.id, "process.start", error)),
        };
        let projection = start_projection(&snapshot);
        Ok(projected_result(invocation.id, projection))
    }
}

impl Tool for ProcessPollTool {
    fn spec(&self) -> ToolSpec {
        poll_spec()
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let process_id = required_input(&input, "process_id")?;
        let stdout_offset = parse_u64(&input, "stdout_offset", 0, 0, u64::MAX)?;
        let stderr_offset = parse_u64(&input, "stderr_offset", 0, 0, u64::MAX)?;
        let max_bytes = parse_bounded_usize_input(
            &input,
            "max_bytes",
            DEFAULT_PROCESS_POLL_BYTES,
            1,
            MAX_PROCESS_POLL_BYTES,
        )?;
        let wait_ms = parse_u64(&input, "wait_ms", 0, 0, MAX_PROCESS_POLL_WAIT_MS)?;
        let snapshot = match self.manager.poll(
            &invocation,
            &process_id,
            stdout_offset,
            stderr_offset,
            max_bytes,
            Duration::from_millis(wait_ms),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(process_error_result(invocation.id, "process.poll", error)),
        };
        let projection =
            snapshot_projection("process.poll", "cindx.process-poll-result.v1", &snapshot);
        Ok(projected_result(invocation.id, projection))
    }
}

impl Tool for ProcessInputTool {
    fn spec(&self) -> ToolSpec {
        input_spec()
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let process_id = input
            .get("process_id")
            .cloned()
            .unwrap_or_else(|| "<missing process_id>".to_string());
        let text = input.get("text").cloned().unwrap_or_default();
        let close = input.get("close").is_some_and(|value| value == "true");
        let metadata = [
            ("tool_call_id".to_string(), invocation.id.0.clone()),
            ("tool_name".to_string(), invocation.tool_name.clone()),
            ("process_id".to_string(), process_id.clone()),
            ("input_digest".to_string(), stable_hash(&text).to_string()),
            ("close".to_string(), close.to_string()),
            ("session_reusable".to_string(), "false".to_string()),
        ]
        .into_iter()
        .collect();
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Execute,
            "process.input",
            "Write one input chunk to an interactive local process. This approval cannot be reused.",
            &process_id,
            metadata,
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let process_id = required_input(&input, "process_id")?;
        let text = input.get("text").cloned().unwrap_or_default();
        if text.len() > MAX_PROCESS_INPUT_CALL_BYTES {
            return Err(ToolError::new(format!(
                "process input must not exceed {MAX_PROCESS_INPUT_CALL_BYTES} UTF-8 bytes"
            )));
        }
        let close = input.get("close").is_some_and(|value| value == "true");
        if text.is_empty() && !close {
            return Err(ToolError::new(
                "process.input requires non-empty text or close=true",
            ));
        }
        let written = match self
            .manager
            .input(&invocation, &process_id, text.into_bytes(), close)
        {
            Ok(written) => written,
            Err(error) => return Ok(process_error_result(invocation.id, "process.input", error)),
        };
        Ok(projected_result(
            invocation.id,
            input_projection(&process_id, written, close),
        ))
    }
}

impl Tool for ProcessTerminateTool {
    fn spec(&self) -> ToolSpec {
        terminate_spec()
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let process_id = required_input(&input, "process_id")?;
        let snapshot = match self.manager.terminate(&invocation, &process_id) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Ok(process_error_result(
                    invocation.id,
                    "process.terminate",
                    error,
                ))
            }
        };
        let projection = snapshot_projection(
            "process.terminate",
            "cindx.process-terminate-result.v1",
            &snapshot,
        );
        Ok(projected_result(invocation.id, projection))
    }
}

fn projected_result(
    invocation_id: agent_core::ToolCallId,
    projection: crate::process_contract::ProcessProjection,
) -> ToolResult {
    let mut result = tool_result(
        invocation_id,
        ToolOutcomeStatus::Succeeded,
        projection.output,
        Metadata::new(),
    );
    result.structured_output_json = Some(projection.structured_output_json);
    result.model_observation = Some(projection.observation);
    result
}

fn process_error_result(
    invocation_id: agent_core::ToolCallId,
    tool_name: &str,
    error: ProcessManagerError,
) -> ToolResult {
    let mut result = tool_result(
        invocation_id,
        ToolOutcomeStatus::Failed,
        error.message.clone(),
        [("failure_code".to_string(), error.code.to_string())]
            .into_iter()
            .collect(),
    );
    result.failure = Some(ToolFailure {
        code: error.code.to_string(),
        message: error.message.clone(),
        retryable: error.retryable,
    });
    result.model_observation = Some(model_observation(
        tool_name,
        "Process operation failed without granting workspace verification authority.",
        error.message,
        true,
        [("failure_code".to_string(), error.code.to_string())]
            .into_iter()
            .collect(),
        error.retryable.then(|| {
            "Retry only after the reported resource condition changes; do not repeat unknown input writes."
                .to_string()
        }),
    ));
    result
}

fn parse_u64(
    input: &std::collections::BTreeMap<String, String>,
    key: &str,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64, ToolError> {
    let value = input
        .get(key)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| ToolError::new(format!("{key} must be an integer")))
        })
        .transpose()?
        .unwrap_or(default);
    if value < minimum || value > maximum {
        return Err(ToolError::new(format!(
            "{key} must be between {minimum} and {maximum}"
        )));
    }
    Ok(value)
}
