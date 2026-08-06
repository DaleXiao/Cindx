use super::{bounded_model_text, model_observation, ToolError};
use agent_core::{
    Metadata, ToolEffectSemantics, ToolObservationV2, ToolResult, ToolRisk, ToolSpec,
};
use std::collections::BTreeMap;
use std::path::Path;

const MODEL_SHELL_EVIDENCE_CHARS: usize = 5_200;
pub(crate) const FILE_READ_RESULT_SCHEMA: &str = "cindx.file-read-result.v2";

pub(crate) fn file_read_spec(default_bytes: usize, max_bytes: usize) -> ToolSpec {
    ToolSpec::builtin(
        "file.read",
        "file",
        "Read a bounded UTF-8 byte range inside the workspace. Large files return a continuation offset.",
        ToolRisk::ReadOnly,
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1, "description": "Workspace-relative file path." },
                "offset_bytes": { "type": "integer", "minimum": 0, "default": 0, "description": "UTF-8 byte offset to start reading." },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": max_bytes, "default": default_bytes, "description": "Maximum source bytes to read." },
                "include_sha256": { "type": "boolean", "default": false, "description": "When true, hash the complete file when it is no larger than 8 MiB. Complete first-page reads include this hash without an extra scan." }
            },
            "required": ["path"],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": FILE_READ_RESULT_SCHEMA },
                "path": { "type": "string" },
                "offset_bytes": { "type": "integer", "minimum": 0 },
                "returned_bytes": { "type": "integer", "minimum": 0 },
                "next_offset_bytes": { "type": "integer", "minimum": 0 },
                "total_bytes": { "type": "integer", "minimum": 0 },
                "truncated": { "type": "boolean" },
                "page_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
                "sha256": { "type": ["string", "null"], "pattern": "^[0-9a-f]{64}$" }
            },
            "required": ["schema", "path", "offset_bytes", "returned_bytes", "next_offset_bytes", "total_bytes", "truncated", "page_sha256", "sha256"],
            "additionalProperties": false
        })
        .to_string(),
    )
}

pub(crate) fn file_read_observation(
    path: &str,
    offset_bytes: u64,
    returned_bytes: usize,
    next_offset_bytes: u64,
    total_bytes: u64,
    hashes: (String, Option<String>),
    evidence: String,
) -> ToolObservationV2 {
    let (page_sha256, sha256) = hashes;
    let truncated = next_offset_bytes < total_bytes;
    let facts = [
        ("path".to_string(), path.to_string()),
        ("offset_bytes".to_string(), offset_bytes.to_string()),
        ("returned_bytes".to_string(), returned_bytes.to_string()),
        (
            "next_offset_bytes".to_string(),
            next_offset_bytes.to_string(),
        ),
        ("total_bytes".to_string(), total_bytes.to_string()),
        ("truncated".to_string(), truncated.to_string()),
    ]
    .into_iter()
    .chain([("page_sha256".to_string(), page_sha256)])
    .chain(sha256.map(|sha256| ("sha256".to_string(), sha256)))
    .collect();
    model_observation(
        "file.read",
        format!("Read {returned_bytes} visible bytes from {path} at offset {offset_bytes}."),
        evidence,
        !truncated,
        facts,
        truncated.then(|| {
            format!(
                "Continue with file.read using offset_bytes={next_offset_bytes}; do not skip to the larger raw-result offset."
            )
        }),
    )
}

pub(crate) fn shell_run_spec(default_timeout: u64, max_timeout: u64) -> ToolSpec {
    ToolSpec::builtin(
        "shell.run",
        "shell",
        "Run a bounded foreground shell command in the workspace. Background processes are terminated when the command finishes.",
        ToolRisk::ExecutesProcess,
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "minLength": 1, "description": "Foreground zsh command to run." },
                "cwd": { "type": "string", "default": ".", "description": "Optional workspace-relative working directory." },
                "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": max_timeout, "default": default_timeout }
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
                "schema": { "const": "cindx.shell-result.v1" },
                "cwd": { "type": "string" },
                "exit_code": { "type": ["integer", "null"] },
                "termination": { "enum": ["exit", "signal", "timeout", "cancelled"] },
                "stdout_bytes": { "type": "integer", "minimum": 0 },
                "stderr_bytes": { "type": "integer", "minimum": 0 },
                "output_truncated": { "type": "boolean" },
                "stdout_artifact": { "type": ["string", "null"] },
                "stderr_artifact": { "type": ["string", "null"] },
                "stdout_complete": { "type": "boolean" },
                "stderr_complete": { "type": "boolean" }
            },
            "required": ["schema", "cwd", "exit_code", "termination", "stdout_bytes", "stderr_bytes", "output_truncated", "stdout_artifact", "stderr_artifact", "stdout_complete", "stderr_complete"],
            "additionalProperties": false
        })
        .to_string(),
    )
}

pub(crate) fn parse_shell_timeout(
    input: &BTreeMap<String, String>,
    default_timeout: u64,
    max_timeout: u64,
) -> Result<u64, ToolError> {
    let timeout = match input.get("timeout_seconds") {
        Some(value) => value.parse::<u64>().map_err(|_| {
            ToolError::new(format!(
                "timeout_seconds must be an integer between 1 and {max_timeout}"
            ))
        })?,
        None => default_timeout,
    };
    if !(1..=max_timeout).contains(&timeout) {
        return Err(ToolError::new(format!(
            "timeout_seconds must be an integer between 1 and {max_timeout}"
        )));
    }
    Ok(timeout)
}

pub(crate) struct ShellResultMetadataInput<'a> {
    pub(crate) command: &'a str,
    pub(crate) cwd: &'a str,
    pub(crate) timeout_seconds: u64,
    pub(crate) timed_out: bool,
    pub(crate) cancelled: bool,
    pub(crate) stdout_bytes: u64,
    pub(crate) stderr_bytes: u64,
    pub(crate) output_truncated: bool,
    pub(crate) exit_code: Option<i32>,
}

pub(crate) fn shell_result_metadata(input: ShellResultMetadataInput<'_>) -> Metadata {
    [
        ("command".to_string(), input.command.to_string()),
        ("cwd".to_string(), input.cwd.to_string()),
        (
            "timeout_seconds".to_string(),
            input.timeout_seconds.to_string(),
        ),
        ("timed_out".to_string(), input.timed_out.to_string()),
        ("cancelled".to_string(), input.cancelled.to_string()),
        (
            "environment_policy".to_string(),
            "developer_safe_v1".to_string(),
        ),
        ("stdout_bytes".to_string(), input.stdout_bytes.to_string()),
        ("stderr_bytes".to_string(), input.stderr_bytes.to_string()),
        (
            "output_truncated".to_string(),
            input.output_truncated.to_string(),
        ),
        (
            "exit_code".to_string(),
            input
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
        ),
    ]
    .into_iter()
    .collect()
}

pub(crate) struct ShellContractInput<'a> {
    pub(crate) cwd: &'a str,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout_bytes: u64,
    pub(crate) stderr_bytes: u64,
    pub(crate) output_truncated: bool,
    pub(crate) stdout_artifact: Option<&'a str>,
    pub(crate) stderr_artifact: Option<&'a str>,
    pub(crate) stdout_complete: bool,
    pub(crate) stderr_complete: bool,
    pub(crate) timed_out: bool,
    pub(crate) cancelled: bool,
    pub(crate) status_succeeded: bool,
    pub(crate) output: &'a str,
}

pub(crate) struct ShellContractOutput {
    pub(crate) structured_output_json: String,
    pub(crate) observation: ToolObservationV2,
    pub(crate) failure_code: Option<&'static str>,
}

pub(crate) fn shell_contract(input: ShellContractInput<'_>) -> ShellContractOutput {
    let termination = if input.cancelled {
        "cancelled"
    } else if input.timed_out {
        "timeout"
    } else if input.exit_code.is_some() {
        "exit"
    } else {
        "signal"
    };
    let streams_complete = input.stdout_complete && input.stderr_complete;
    let structured_output = serde_json::json!({
        "schema": "cindx.shell-result.v1",
        "cwd": input.cwd,
        "exit_code": input.exit_code,
        "termination": termination,
        "stdout_bytes": input.stdout_bytes,
        "stderr_bytes": input.stderr_bytes,
        "output_truncated": input.output_truncated,
        "stdout_artifact": input.stdout_artifact,
        "stderr_artifact": input.stderr_artifact,
        "stdout_complete": input.stdout_complete,
        "stderr_complete": input.stderr_complete,
    });
    let facts = object_string_facts(&structured_output);
    let (evidence, evidence_truncated) =
        bounded_model_text(input.output, MODEL_SHELL_EVIDENCE_CHARS);
    let artifacts = [input.stdout_artifact, input.stderr_artifact]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let next_action = if input.timed_out {
        Some("Verify filesystem and process effects before considering a narrower retry; a timed-out command may have partially completed.".to_string())
    } else if input.cancelled {
        Some("Treat this output as partial and verify filesystem and process effects before deciding whether a narrower retry is safe.".to_string())
    } else if termination == "signal" {
        Some("Treat this output as partial, identify the terminating signal or environmental cause, and change the approach before retrying.".to_string())
    } else if !streams_complete {
        Some(if artifacts.is_empty() {
            "Rerun a narrower diagnostic command because the complete stream exceeded a safety limit or could not be retained.".to_string()
        } else {
            format!(
                "Treat {} as partial evidence and rerun a narrower diagnostic command; the complete stream exceeded the artifact safety limit.",
                artifacts.join(" and ")
            )
        })
    } else if input.output_truncated || evidence_truncated {
        if artifacts.is_empty() {
            Some("Rerun a narrower diagnostic command because the complete output is unavailable to the model.".to_string())
        } else {
            Some(format!(
                "Inspect the complete stream with file.read at {} before deciding the next command.",
                artifacts.join(" or ")
            ))
        }
    } else if !input.status_succeeded {
        Some("Use the exit status and stderr to change the command or approach; do not repeat the identical failing call.".to_string())
    } else {
        None
    };
    let failure_code = if input.timed_out {
        Some("shell_timeout")
    } else if termination == "signal" {
        Some("shell_signal")
    } else if !input.status_succeeded && !input.cancelled {
        Some("shell_exit_nonzero")
    } else {
        None
    };
    ShellContractOutput {
        structured_output_json: structured_output.to_string(),
        observation: model_observation(
            "shell.run",
            format!(
                "Shell command ended with termination={}, exit_code={}.",
                termination,
                input
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "null".to_string())
            ),
            evidence,
            !input.output_truncated
                && !evidence_truncated
                && !input.timed_out
                && !input.cancelled
                && termination != "signal",
            facts,
            next_action,
        ),
        failure_code,
    }
}

pub(crate) fn workspace_relative_artifact(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

pub(crate) fn browser_extract_spec(
    description: &str,
    risk: ToolRisk,
    effect_semantics: ToolEffectSemantics,
    response_schema: &str,
) -> ToolSpec {
    ToolSpec::builtin(
        "browser.extract_text",
        "browser",
        description,
        risk,
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "format": "uri", "description": "Optional HTTP(S) URL; omit to use the current page." },
                "tab_id": { "type": "string", "description": "Optional CDP target id." },
                "frame": { "type": "string", "description": "Optional unique frame name or URL fragment." },
                "selector": { "type": "string", "description": "CSS target. Use at most one target strategy." },
                "role": { "type": "string", "description": "Accessible role target; name may refine it." },
                "name": { "type": "string", "description": "Accessible name used only with role." },
                "label": { "type": "string", "description": "Form label target. Use at most one target strategy." },
                "placeholder": { "type": "string", "description": "Placeholder target. Use at most one target strategy." },
                "text_target": { "type": "string", "description": "Visible text target. Use at most one target strategy." },
                "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 120000, "default": 30000 },
                "session_id": { "type": "string" },
                "output_dir": { "type": "string", "description": "Optional workspace-relative artifact directory." }
            },
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": response_schema },
                "ok": { "type": "boolean" },
                "id": { "type": "string" },
                "action": { "const": "extract_text" },
                "session_id": { "type": "string" },
                "controller": { "type": "string" },
                "page": { "type": ["object", "null"] },
                "output": { "type": "string" },
                "artifacts": { "type": "array" },
                "trace_path": { "type": "string" },
                "duration_ms": { "type": "integer", "minimum": 0 }
            },
            "required": ["schema", "ok", "id", "action", "session_id", "controller", "output", "artifacts", "trace_path", "duration_ms"]
        })
        .to_string(),
    )
    .with_effect_semantics(effect_semantics)
}

pub(crate) fn validate_browser_extract_target(
    input: &BTreeMap<String, String>,
) -> Result<(), ToolError> {
    if input
        .get("name")
        .is_some_and(|value| !value.trim().is_empty())
        && input
            .get("role")
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(ToolError::new(
            "browser.extract_text name requires an accessible role",
        ));
    }
    let target_count = ["selector", "role", "label", "placeholder", "text_target"]
        .iter()
        .filter(|key| {
            input
                .get(**key)
                .is_some_and(|value| !value.trim().is_empty())
        })
        .count();
    if target_count > 1 {
        return Err(ToolError::new(
            "browser.extract_text accepts at most one target strategy",
        ));
    }
    Ok(())
}

pub(crate) fn attach_browser_extract_observation(result: &mut ToolResult) {
    let (evidence, evidence_truncated) = bounded_model_text(&result.output, 5_200);
    let facts = [
        ("url", "url"),
        ("title", "title"),
        ("tab_id", "tab_id"),
        ("duration_ms", "duration_ms"),
        ("text_path", "text_path"),
    ]
    .into_iter()
    .filter_map(|(fact, metadata_key)| {
        result
            .metadata
            .get(metadata_key)
            .cloned()
            .map(|value| (fact.to_string(), value))
    })
    .chain(std::iter::once((
        "observation_truncated".to_string(),
        evidence_truncated.to_string(),
    )))
    .collect::<Metadata>();
    let next_action = evidence_truncated.then(|| {
        result
            .metadata
            .get("text_path")
            .map(|path| {
                format!(
                    "Read {path} with file.read for more body text, or call browser.extract_text again with one narrower target for accessibility detail."
                )
            })
            .unwrap_or_else(|| {
                "Call browser.extract_text again with one narrower target to retrieve the omitted page evidence."
                    .to_string()
            })
    });
    let page_label = result
        .metadata
        .get("title")
        .or_else(|| result.metadata.get("url"))
        .cloned()
        .unwrap_or_else(|| "the current page".to_string());
    result.model_observation = Some(model_observation(
        "browser.extract_text",
        format!("Extracted browser text and accessibility evidence from {page_label}."),
        evidence,
        !evidence_truncated,
        facts,
        next_action,
    ));
}

fn object_string_facts(value: &serde_json::Value) -> Metadata {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.as_str() != "schema")
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string()),
            )
        })
        .collect()
}
