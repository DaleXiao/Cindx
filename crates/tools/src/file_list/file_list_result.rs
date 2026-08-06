use agent_core::{Metadata, ToolFailure, ToolInvocation, ToolOutcomeStatus, ToolResult};

use crate::{bounded_model_text, model_observation, tool_result, ToolError};

use super::file_list_contract::MODEL_LIST_EVIDENCE_CHARS;
use super::file_list_cursor::{encode_cursor, list_snapshot_hash};
use super::file_list_model::{ListCursor, ListSnapshot};

pub(super) fn build_result(
    invocation: ToolInvocation,
    path: String,
    glob: Option<String>,
    max_results: usize,
    cursor: Option<ListCursor>,
    scope: u64,
    snapshot: ListSnapshot,
) -> Result<ToolResult, ToolError> {
    let matching = snapshot.entries.iter().collect::<Vec<_>>();
    let snapshot_hash = list_snapshot_hash(
        &matching,
        &snapshot.failures,
        snapshot.cancelled,
        snapshot.discovery_limit_reached,
    );
    if cursor.is_some_and(|cursor| cursor.snapshot != snapshot_hash) {
        return Err(ToolError::new(
            "file.list result set changed since the cursor was issued; restart without a cursor",
        ));
    }
    let start = cursor.map(|cursor| cursor.offset).unwrap_or(0);
    if start > matching.len() {
        return Err(ToolError::new(
            "file.list cursor is beyond the current result set; restart without a cursor",
        ));
    }
    let end = start.saturating_add(max_results).min(matching.len());
    let page = &matching[start..end];
    let next_cursor = (!snapshot.cancelled
        && !snapshot.discovery_limit_reached
        && snapshot.failures.is_empty()
        && end < matching.len())
    .then(|| encode_cursor(scope, snapshot_hash, end));
    let complete = !snapshot.cancelled
        && !snapshot.discovery_limit_reached
        && snapshot.failures.is_empty()
        && next_cursor.is_none();
    let output = page
        .iter()
        .map(|entry| entry.row())
        .collect::<Vec<_>>()
        .join("\n");
    let failures = snapshot
        .failures
        .iter()
        .map(|failure| {
            serde_json::json!({
                "name": failure.name,
                "message": failure.message,
            })
        })
        .collect::<Vec<_>>();
    let structured_output = serde_json::json!({
        "schema": "cindx.file-list-result.v2",
        "path": path,
        "glob": glob,
        "entries": page.iter().map(|entry| entry.structured()).collect::<Vec<_>>(),
        "failures": failures,
        "discovered": snapshot.discovered,
        "discovery_limit": snapshot.discovery_limit,
        "discovery_limit_reached": snapshot.discovery_limit_reached,
        "returned": page.len(),
        "total": matching.len(),
        "next_cursor": next_cursor,
        "complete": complete,
        "cancelled": snapshot.cancelled,
    });
    let mut metadata = Metadata::new();
    metadata.insert("path".to_string(), path.clone());
    metadata.insert("entries".to_string(), page.len().to_string());
    metadata.insert("entries_total".to_string(), matching.len().to_string());
    metadata.insert(
        "entries_discovered".to_string(),
        snapshot.discovered.to_string(),
    );
    metadata.insert(
        "discovery_limit".to_string(),
        snapshot.discovery_limit.to_string(),
    );
    metadata.insert(
        "discovery_limit_reached".to_string(),
        snapshot.discovery_limit_reached.to_string(),
    );
    metadata.insert(
        "entry_failures".to_string(),
        snapshot.failures.len().to_string(),
    );
    metadata.insert("complete".to_string(), complete.to_string());
    metadata.insert("cancelled".to_string(), snapshot.cancelled.to_string());
    if let Some(cursor) = next_cursor.as_ref() {
        metadata.insert("next_cursor".to_string(), cursor.clone());
    }

    let status = if snapshot.cancelled {
        ToolOutcomeStatus::Cancelled
    } else if snapshot.discovery_limit_reached || !snapshot.failures.is_empty() {
        ToolOutcomeStatus::Failed
    } else {
        ToolOutcomeStatus::Succeeded
    };
    let (evidence, evidence_truncated) = bounded_model_text(&output, MODEL_LIST_EVIDENCE_CHARS);
    let next_action = if snapshot.cancelled {
        Some("Retry this listing only if it is still required.".to_string())
    } else if snapshot.discovery_limit_reached {
        Some(format!(
            "Directory discovery stopped at the hard {}-entry limit; narrow the path before retrying.",
            snapshot.discovery_limit
        ))
    } else if !snapshot.failures.is_empty() {
        Some("Some directory entries could not be inspected; retry or narrow the path.".to_string())
    } else if let Some(cursor) = next_cursor.as_ref() {
        Some(format!(
            "Continue file.list with the same path and glob using cursor={cursor}."
        ))
    } else if evidence_truncated {
        Some("Use a narrower glob or smaller page for model-visible evidence.".to_string())
    } else {
        None
    };
    let facts = [
        ("path".to_string(), path),
        ("returned".to_string(), page.len().to_string()),
        ("total".to_string(), matching.len().to_string()),
        ("discovered".to_string(), snapshot.discovered.to_string()),
        (
            "discovery_limit".to_string(),
            snapshot.discovery_limit.to_string(),
        ),
        (
            "discovery_limit_reached".to_string(),
            snapshot.discovery_limit_reached.to_string(),
        ),
        ("failures".to_string(), snapshot.failures.len().to_string()),
        ("complete".to_string(), complete.to_string()),
        ("cancelled".to_string(), snapshot.cancelled.to_string()),
    ]
    .into_iter()
    .collect();
    let mut result = tool_result(invocation.id, status, output, metadata);
    if matches!(result.status, ToolOutcomeStatus::Failed) {
        result.failure = Some(if snapshot.discovery_limit_reached {
            ToolFailure {
                code: "file_list_discovery_limit".to_string(),
                message: format!(
                    "directory discovery exceeded the hard {}-entry limit",
                    snapshot.discovery_limit
                ),
                retryable: true,
            }
        } else {
            ToolFailure {
                code: "file_list_partial_failure".to_string(),
                message: format!(
                    "{} directory entries could not be inspected",
                    snapshot.failures.len()
                ),
                retryable: true,
            }
        });
    }
    result.structured_output_json = Some(structured_output.to_string());
    result.model_observation = Some(model_observation(
        "file.list",
        if snapshot.cancelled {
            format!(
                "Directory listing was cancelled after {} entries.",
                page.len()
            )
        } else if snapshot.discovery_limit_reached {
            format!(
                "Directory listing stopped at the hard discovery limit after {} entries.",
                snapshot.discovered
            )
        } else if snapshot.failures.is_empty() {
            format!(
                "Listed {} of {} matching entries.",
                page.len(),
                matching.len()
            )
        } else {
            format!(
                "Listed {} entries; {} entries failed inspection.",
                page.len(),
                snapshot.failures.len()
            )
        },
        evidence,
        complete && !evidence_truncated,
        facts,
        next_action,
    ));
    Ok(result)
}
