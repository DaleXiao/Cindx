use agent_core::{Metadata, ToolFailure, ToolInvocation, ToolOutcomeStatus, ToolResult};

use crate::{bounded_model_text, model_observation, tool_result, ToolError};

use super::file_batch_request::ReadPath;

const MODEL_BATCH_EVIDENCE_CHARS: usize = 5_200;

pub(super) struct BatchResultProjection {
    paths_requested: usize,
    max_bytes_per_file: usize,
    sections: Vec<String>,
    items: Vec<serde_json::Value>,
    continuations: Vec<serde_json::Value>,
    read_count: usize,
    error_count: usize,
    truncated_count: usize,
    cancelled: bool,
}

impl BatchResultProjection {
    pub(super) fn new(paths_requested: usize, max_bytes_per_file: usize) -> Self {
        Self {
            paths_requested,
            max_bytes_per_file,
            sections: Vec::with_capacity(paths_requested),
            items: Vec::with_capacity(paths_requested),
            continuations: Vec::new(),
            read_count: 0,
            error_count: 0,
            truncated_count: 0,
            cancelled: false,
        }
    }

    pub(super) fn record_success(&mut self, request: &ReadPath, result: &ToolResult) {
        self.read_count += 1;
        self.sections
            .push(format!("===== {} =====\n{}", request.path, result.output));
        let item = successful_item(request, result);
        if item
            .get("truncated")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            self.truncated_count += 1;
            if let Some(offset_bytes) = item
                .get("next_offset_bytes")
                .and_then(serde_json::Value::as_u64)
            {
                self.continuations.push(serde_json::json!({
                    "path": request.path,
                    "offset_bytes": offset_bytes,
                }));
            }
        }
        self.items.push(item);
    }

    pub(super) fn record_non_success(&mut self, request: &ReadPath, result: &ToolResult) {
        self.error_count += 1;
        self.sections.push(format!(
            "===== {} =====\n[read failed: {}]",
            request.path, result.output
        ));
        self.items.push(non_success_item(request, result));
    }

    pub(super) fn record_error(&mut self, request: &ReadPath, error: &ToolError) {
        self.error_count += 1;
        self.sections.push(format!(
            "===== {} =====\n[read failed: {error}]",
            request.path
        ));
        self.items.push(error_item(request, error));
    }

    pub(super) fn cancel_remaining(&mut self, paths: &[ReadPath]) {
        self.cancelled = true;
        for request in paths {
            self.sections.push(format!(
                "===== {} =====\n[read cancelled before this path]",
                request.path
            ));
            self.items.push(serde_json::json!({
                "path": request.path,
                "status": "cancelled",
                "requested_offset_bytes": request.offset_bytes,
            }));
        }
    }

    pub(super) fn into_result(self, invocation: ToolInvocation) -> ToolResult {
        let complete = !self.cancelled && self.error_count == 0 && self.truncated_count == 0;
        let output = self.sections.join("\n\n");
        let mut metadata = Metadata::new();
        metadata.insert(
            "paths_requested".to_string(),
            self.paths_requested.to_string(),
        );
        metadata.insert("paths_read".to_string(), self.read_count.to_string());
        metadata.insert("paths_failed".to_string(), self.error_count.to_string());
        metadata.insert(
            "paths_truncated".to_string(),
            self.truncated_count.to_string(),
        );
        metadata.insert(
            "max_bytes_per_file".to_string(),
            self.max_bytes_per_file.to_string(),
        );
        metadata.insert("complete".to_string(), complete.to_string());
        metadata.insert("cancelled".to_string(), self.cancelled.to_string());

        let structured_output = serde_json::json!({
            "schema": "cindx.file-read-many-result.v2",
            "paths_requested": self.paths_requested,
            "paths_read": self.read_count,
            "paths_failed": self.error_count,
            "paths_truncated": self.truncated_count,
            "max_bytes_per_file": self.max_bytes_per_file,
            "complete": complete,
            "cancelled": self.cancelled,
            "items": self.items,
            "continuations": self.continuations,
        });
        let status = if self.cancelled {
            ToolOutcomeStatus::Cancelled
        } else if self.error_count > 0 {
            ToolOutcomeStatus::Failed
        } else {
            ToolOutcomeStatus::Succeeded
        };
        let (evidence, evidence_truncated) =
            bounded_model_text(&output, MODEL_BATCH_EVIDENCE_CHARS);
        let next_action = if self.cancelled {
            Some("Retry only the still-required paths; this batch was cancelled.".to_string())
        } else if !self.continuations.is_empty() {
            Some(format!(
                "Continue truncated files with their per-file offset_bytes values: {}.",
                self.continuations
                    .iter()
                    .filter_map(|continuation| Some(format!(
                        "{}@{}",
                        continuation.get("path")?.as_str()?,
                        continuation.get("offset_bytes")?.as_u64()?
                    )))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        } else if self.error_count > 0 {
            Some("Inspect each failed item and retry only recoverable paths.".to_string())
        } else if evidence_truncated {
            Some(
                "Use a smaller batch or lower max_bytes_per_file for model-visible evidence."
                    .to_string(),
            )
        } else {
            None
        };
        let facts = [
            (
                "paths_requested".to_string(),
                self.paths_requested.to_string(),
            ),
            ("paths_read".to_string(), self.read_count.to_string()),
            ("paths_failed".to_string(), self.error_count.to_string()),
            (
                "paths_truncated".to_string(),
                self.truncated_count.to_string(),
            ),
            ("complete".to_string(), complete.to_string()),
            ("cancelled".to_string(), self.cancelled.to_string()),
        ]
        .into_iter()
        .collect();
        let mut result = tool_result(invocation.id, status, output, metadata);
        if matches!(result.status, ToolOutcomeStatus::Failed) {
            result.failure = Some(ToolFailure {
                code: "file_read_many_partial_failure".to_string(),
                message: format!(
                    "{} of {} requested paths failed",
                    self.error_count, self.paths_requested
                ),
                retryable: true,
            });
        }
        result.structured_output_json = Some(structured_output.to_string());
        result.model_observation = Some(model_observation(
            "file.read_many",
            if self.cancelled {
                format!(
                    "Batch read was cancelled after {} successful paths.",
                    self.read_count
                )
            } else if self.error_count > 0 {
                format!(
                    "Read {} paths and failed {} paths.",
                    self.read_count, self.error_count
                )
            } else {
                format!("Read {} paths.", self.read_count)
            },
            evidence,
            complete && !evidence_truncated,
            facts,
            next_action,
        ));
        result
    }
}

pub(super) fn successful_item(request: &ReadPath, result: &ToolResult) -> serde_json::Value {
    let child = result
        .structured_output_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok());
    let child = child.as_ref().and_then(serde_json::Value::as_object);
    let mut hashes = serde_json::Map::new();
    if let Some(child) = child {
        for (key, value) in child {
            if key == "sha256" || key.ends_with("_sha256") {
                hashes.insert(key.clone(), value.clone());
            }
        }
    }
    for (key, value) in &result.metadata {
        if (key == "sha256" || key.ends_with("_sha256")) && !hashes.contains_key(key) {
            hashes.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
    }

    serde_json::json!({
        "path": request.path,
        "status": "succeeded",
        "requested_offset_bytes": request.offset_bytes,
        "offset_bytes": child_u64(child, "offset_bytes", request.offset_bytes),
        "returned_bytes": child_u64(child, "returned_bytes", 0),
        "next_offset_bytes": child_u64(child, "next_offset_bytes", request.offset_bytes),
        "total_bytes": child_u64(child, "total_bytes", 0),
        "truncated": child_bool(child, "truncated", false),
        "hashes": hashes,
    })
}

fn non_success_item(request: &ReadPath, result: &ToolResult) -> serde_json::Value {
    let failure = result.failure.clone().unwrap_or_else(|| ToolFailure {
        code: status_label(&result.status).to_string(),
        message: result.output.clone(),
        retryable: false,
    });
    serde_json::json!({
        "path": request.path,
        "status": status_label(&result.status),
        "requested_offset_bytes": request.offset_bytes,
        "failure": {
            "code": failure.code,
            "message": failure.message,
            "retryable": failure.retryable,
        }
    })
}

fn error_item(request: &ReadPath, error: &ToolError) -> serde_json::Value {
    serde_json::json!({
        "path": request.path,
        "status": "failed",
        "requested_offset_bytes": request.offset_bytes,
        "failure": {
            "code": error.code,
            "message": error.message,
            "retryable": error.retryable,
        }
    })
}

fn child_u64(
    child: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
    fallback: u64,
) -> u64 {
    child
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(fallback)
}

fn child_bool(
    child: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
    fallback: bool,
) -> bool {
    child
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(fallback)
}

fn status_label(status: &ToolOutcomeStatus) -> &'static str {
    match status {
        ToolOutcomeStatus::Succeeded => "succeeded",
        ToolOutcomeStatus::Failed => "failed",
        ToolOutcomeStatus::Cancelled => "cancelled",
        ToolOutcomeStatus::Denied => "denied",
    }
}
