use agent_core::{Metadata, ToolEffectSemantics, ToolObservationV2, ToolRisk, ToolSpec};

use crate::process_runtime::ManagedProcessSnapshot;
use crate::{bounded_model_text, model_observation};

pub(crate) const DEFAULT_PROCESS_TIMEOUT_SECONDS: u64 = 600;
pub(crate) const MAX_PROCESS_TIMEOUT_SECONDS: u64 = 1_800;
pub(crate) const DEFAULT_PROCESS_CPU_SECONDS: u64 = 300;
pub(crate) const MAX_PROCESS_CPU_SECONDS: u64 = 900;
pub(crate) const DEFAULT_PROCESS_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_PROCESS_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const DEFAULT_PROCESS_POLL_BYTES: usize = 16 * 1024;
pub(crate) const MAX_PROCESS_POLL_BYTES: usize = 64 * 1024;
pub(crate) const MAX_PROCESS_POLL_WAIT_MS: u64 = 2_000;
pub(crate) const MAX_PROCESS_INPUT_CALL_BYTES: usize = 64 * 1024;

pub(crate) struct ProcessProjection {
    pub(crate) output: String,
    pub(crate) structured_output_json: String,
    pub(crate) observation: ToolObservationV2,
}

pub(crate) fn start_spec() -> ToolSpec {
    ToolSpec::builtin(
        "process.start",
        "process",
        "Reserve a bounded background process in the workspace. The authorized command activates on the first poll or input call.",
        ToolRisk::ExecutesProcess,
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "minLength": 1, "description": "zsh command to execute after the durable start receipt." },
                "cwd": { "type": "string", "default": ".", "description": "Workspace-relative working directory." },
                "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": MAX_PROCESS_TIMEOUT_SECONDS, "default": DEFAULT_PROCESS_TIMEOUT_SECONDS },
                "cpu_seconds": { "type": "integer", "minimum": 1, "maximum": MAX_PROCESS_CPU_SECONDS, "default": DEFAULT_PROCESS_CPU_SECONDS },
                "output_limit_bytes": { "type": "integer", "minimum": 4096, "maximum": MAX_PROCESS_OUTPUT_BYTES, "default": DEFAULT_PROCESS_OUTPUT_BYTES }
            },
            "required": ["command"],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": "cindx.process-start-result.v1" },
                "process_id": { "type": "string", "pattern": "^proc_[0-9a-f]{36}$" },
                "generation": { "type": "integer", "minimum": 1 },
                "start_call_id": { "type": "string" },
                "state": { "const": "pending" },
                "cwd": { "type": "string" },
                "timeout_seconds": { "type": "integer", "minimum": 1 },
                "cpu_seconds": { "type": "integer", "minimum": 1 },
                "output_limit_bytes": { "type": "integer", "minimum": 4096 }
            },
            "required": ["schema", "process_id", "generation", "start_call_id", "state", "cwd", "timeout_seconds", "cpu_seconds", "output_limit_bytes"],
            "additionalProperties": false
        })
        .to_string(),
    )
}

pub(crate) fn poll_spec() -> ToolSpec {
    ToolSpec::builtin(
        "process.poll",
        "process",
        "Activate or poll an owned process and read one bounded stdout/stderr delta. Child failure is reported in the typed process state, not as transport failure.",
        ToolRisk::ExecutesProcess,
        serde_json::json!({
            "type": "object",
            "properties": {
                "process_id": { "type": "string", "minLength": 1 },
                "stdout_offset": { "type": "integer", "minimum": 0, "default": 0 },
                "stderr_offset": { "type": "integer", "minimum": 0, "default": 0 },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": MAX_PROCESS_POLL_BYTES, "default": DEFAULT_PROCESS_POLL_BYTES },
                "wait_ms": { "type": "integer", "minimum": 0, "maximum": MAX_PROCESS_POLL_WAIT_MS, "default": 0 }
            },
            "required": ["process_id"],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(process_snapshot_schema("cindx.process-poll-result.v1"))
}

pub(crate) fn input_spec() -> ToolSpec {
    ToolSpec::builtin(
        "process.input",
        "process",
        "Write one bounded input chunk to an owned process. Each call is permission-gated because interactive input can execute new behavior.",
        ToolRisk::ExecutesProcess,
        serde_json::json!({
            "type": "object",
            "properties": {
                "process_id": { "type": "string", "minLength": 1 },
                "text": { "type": "string", "maxLength": MAX_PROCESS_INPUT_CALL_BYTES },
                "close": { "type": "boolean", "default": false }
            },
            "required": ["process_id"],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": "cindx.process-input-result.v1" },
                "process_id": { "type": "string" },
                "written_bytes": { "type": "integer", "minimum": 0 },
                "stdin_closed": { "type": "boolean" }
            },
            "required": ["schema", "process_id", "written_bytes", "stdin_closed"],
            "additionalProperties": false
        })
        .to_string(),
    )
}

pub(crate) fn terminate_spec() -> ToolSpec {
    ToolSpec::builtin(
        "process.terminate",
        "process",
        "Idempotently terminate an owned process group and return its bounded terminal observation.",
        ToolRisk::ExecutesProcess,
        serde_json::json!({
            "type": "object",
            "properties": {
                "process_id": { "type": "string", "minLength": 1 }
            },
            "required": ["process_id"],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_effect_semantics(ToolEffectSemantics::Idempotent)
    .with_output_schema(process_snapshot_schema(
        "cindx.process-terminate-result.v1",
    ))
}

fn process_snapshot_schema(schema: &str) -> String {
    serde_json::json!({
        "type": "object",
        "properties": {
            "schema": { "const": schema },
            "process_id": { "type": "string" },
            "generation": { "type": "integer", "minimum": 1 },
            "start_call_id": { "type": "string" },
            "state": { "enum": ["pending", "running", "terminal"] },
            "termination": { "type": ["string", "null"] },
            "exit_code": { "type": ["integer", "null"] },
            "signal": { "type": ["integer", "null"] },
            "elapsed_ms": { "type": "integer", "minimum": 0 },
            "cwd": { "type": "string" },
            "timeout_seconds": { "type": "integer", "minimum": 1 },
            "cpu_seconds": { "type": "integer", "minimum": 1 },
            "output_limit_bytes": { "type": "integer", "minimum": 4096 },
            "stdout": { "$ref": "#/$defs/stream" },
            "stderr": { "$ref": "#/$defs/stream" }
        },
        "required": ["schema", "process_id", "generation", "start_call_id", "state", "termination", "exit_code", "signal", "elapsed_ms", "cwd", "timeout_seconds", "cpu_seconds", "output_limit_bytes", "stdout", "stderr"],
        "additionalProperties": false,
        "$defs": {
            "stream": {
                "type": "object",
                "properties": {
                    "offset": { "type": "integer", "minimum": 0 },
                    "next_offset": { "type": "integer", "minimum": 0 },
                    "captured_bytes": { "type": "integer", "minimum": 0 },
                    "produced_bytes": { "type": "integer", "minimum": 0 },
                    "text": { "type": "string" },
                    "more_available": { "type": "boolean" },
                    "complete": { "type": "boolean" },
                    "truncated": { "type": "boolean" }
                },
                "required": ["offset", "next_offset", "captured_bytes", "produced_bytes", "text", "more_available", "complete", "truncated"],
                "additionalProperties": false
            }
        }
    })
    .to_string()
}

pub(crate) fn start_projection(snapshot: &ManagedProcessSnapshot) -> ProcessProjection {
    let value = serde_json::json!({
        "schema": "cindx.process-start-result.v1",
        "process_id": snapshot.process_id,
        "generation": snapshot.generation,
        "start_call_id": snapshot.start_call_id,
        "state": snapshot.snapshot.state,
        "cwd": snapshot.cwd,
        "timeout_seconds": snapshot.budgets.timeout_seconds,
        "cpu_seconds": snapshot.budgets.cpu_seconds,
        "output_limit_bytes": snapshot.budgets.output_limit_bytes
    });
    let facts = snapshot_facts(snapshot);
    ProcessProjection {
        output: format!(
            "Reserved process {}. Poll it to activate the authorized command.",
            snapshot.process_id
        ),
        structured_output_json: value.to_string(),
        observation: model_observation(
            "process.start",
            "Reserved a bounded process session; the command has not executed yet.",
            format!("process_id={}", snapshot.process_id),
            true,
            facts,
            Some(format!(
                "Call process.poll with process_id={} to activate it, then continue from stdout_offset=0 and stderr_offset=0.",
                snapshot.process_id
            )),
        ),
    }
}

pub(crate) fn snapshot_projection(
    tool_name: &str,
    schema: &str,
    snapshot: &ManagedProcessSnapshot,
) -> ProcessProjection {
    let stdout = stream_value(&snapshot.snapshot.stdout);
    let stderr = stream_value(&snapshot.snapshot.stderr);
    let value = serde_json::json!({
        "schema": schema,
        "process_id": snapshot.process_id,
        "generation": snapshot.generation,
        "start_call_id": snapshot.start_call_id,
        "state": snapshot.snapshot.state,
        "termination": snapshot.snapshot.termination,
        "exit_code": snapshot.snapshot.exit_code,
        "signal": snapshot.snapshot.signal,
        "elapsed_ms": snapshot.snapshot.elapsed_ms,
        "cwd": snapshot.cwd,
        "timeout_seconds": snapshot.budgets.timeout_seconds,
        "cpu_seconds": snapshot.budgets.cpu_seconds,
        "output_limit_bytes": snapshot.budgets.output_limit_bytes,
        "stdout": stdout,
        "stderr": stderr
    });
    let raw_evidence = format!(
        "stdout:\n{}\nstderr:\n{}",
        snapshot.snapshot.stdout.text, snapshot.snapshot.stderr.text
    );
    let (evidence, evidence_truncated) = bounded_model_text(&raw_evidence, 5_200);
    let stream_complete = snapshot.snapshot.stdout.complete && snapshot.snapshot.stderr.complete;
    let source_truncated = snapshot.snapshot.stdout.truncated || snapshot.snapshot.stderr.truncated;
    let complete =
        snapshot.snapshot.terminal && stream_complete && !source_truncated && !evidence_truncated;
    let next_action = if snapshot.snapshot.terminal {
        if !stream_complete || evidence_truncated {
            Some(format!(
                "Continue process.poll with process_id={}, stdout_offset={}, stderr_offset={}; treat omitted output as partial evidence.",
                snapshot.process_id,
                snapshot.snapshot.stdout.next_offset,
                snapshot.snapshot.stderr.next_offset
            ))
        } else if source_truncated {
            Some(
                "The process exceeded its hard output budget. Rerun a narrower command if the omitted output is required; do not infer it."
                    .to_string(),
            )
        } else {
            None
        }
    } else {
        Some(format!(
            "Poll process_id={} again with stdout_offset={} and stderr_offset={}.",
            snapshot.process_id,
            snapshot.snapshot.stdout.next_offset,
            snapshot.snapshot.stderr.next_offset
        ))
    };
    ProcessProjection {
        output: evidence.clone(),
        structured_output_json: value.to_string(),
        observation: model_observation(
            tool_name,
            format!(
                "Process {} is {}; termination={}, exit_code={}.",
                snapshot.process_id,
                snapshot.snapshot.state,
                snapshot.snapshot.termination.unwrap_or("none"),
                snapshot
                    .snapshot
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "null".to_string())
            ),
            evidence,
            complete,
            snapshot_facts(snapshot),
            next_action,
        ),
    }
}

pub(crate) fn input_projection(
    process_id: &str,
    written_bytes: usize,
    close: bool,
) -> ProcessProjection {
    let value = serde_json::json!({
        "schema": "cindx.process-input-result.v1",
        "process_id": process_id,
        "written_bytes": written_bytes,
        "stdin_closed": close
    });
    let facts = [
        ("process_id".to_string(), process_id.to_string()),
        ("written_bytes".to_string(), written_bytes.to_string()),
        ("stdin_closed".to_string(), close.to_string()),
    ]
    .into_iter()
    .collect();
    ProcessProjection {
        output: format!("Wrote {written_bytes} bytes to process {process_id}."),
        structured_output_json: value.to_string(),
        observation: model_observation(
            "process.input",
            format!("Wrote {written_bytes} acknowledged bytes to an owned process."),
            format!("process_id={process_id}"),
            true,
            facts,
            Some(format!(
                "Call process.poll with process_id={process_id} to observe the resulting output."
            )),
        ),
    }
}

fn snapshot_facts(snapshot: &ManagedProcessSnapshot) -> Metadata {
    [
        ("process_id".to_string(), snapshot.process_id.clone()),
        ("generation".to_string(), snapshot.generation.to_string()),
        ("start_call_id".to_string(), snapshot.start_call_id.clone()),
        ("state".to_string(), snapshot.snapshot.state.to_string()),
        (
            "termination".to_string(),
            snapshot.snapshot.termination.unwrap_or("none").to_string(),
        ),
        (
            "exit_code".to_string(),
            snapshot
                .snapshot
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "null".to_string()),
        ),
        (
            "stdout_next_offset".to_string(),
            snapshot.snapshot.stdout.next_offset.to_string(),
        ),
        (
            "stderr_next_offset".to_string(),
            snapshot.snapshot.stderr.next_offset.to_string(),
        ),
        (
            "output_limit_bytes".to_string(),
            snapshot.budgets.output_limit_bytes.to_string(),
        ),
    ]
    .into_iter()
    .collect()
}

fn stream_value(stream: &crate::process_capture::ProcessStreamPage) -> serde_json::Value {
    serde_json::json!({
        "offset": stream.offset,
        "next_offset": stream.next_offset,
        "captured_bytes": stream.captured_bytes,
        "produced_bytes": stream.produced_bytes,
        "text": stream.text,
        "more_available": stream.more_available,
        "complete": stream.complete,
        "truncated": stream.truncated
    })
}
