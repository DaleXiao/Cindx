//! `file.patch_batch`: an atomic multi-file anchored patch. Every item carries
//! the exact `file.patch` single-file semantics (path + base SHA-256 + anchor
//! or byte-range selector + replacement) and is prepared through the
//! single-file `PatchFileTool` pipeline. All targets are fully validated
//! before any file is written; a validation failure writes nothing. Publishing
//! follows the input order through the same locked atomic replace; a racing
//! publish failure stops the batch and rolls the applied prefix back from the
//! in-memory before-images.

use super::contract::{PatchIssue, MAX_PATCH_FILE_BYTES};
use super::undo::preserve_history_version;
use super::{preserve_output_version, PatchFileTool, PreparedPatch};
use crate::workspace_file::{atomic_replace_preserving_permissions_if_sha256, AtomicReplaceError};
use crate::{permission_request, Tool, ToolError};
use agent_core::{
    Metadata, PermissionRequest, PermissionRisk, ToolFailure, ToolInvocation, ToolObservationV2,
    ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(crate) const MAX_BATCH_PATCHES: usize = 16;
pub(crate) const MAX_BATCH_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub(super) const BATCH_RECEIPT_SCHEMA: &str = "cindx.file-patch-batch-result.v1";
const MAX_DISPLAY_PATHS_CHARS: usize = 512;

pub struct PatchBatchFileTool {
    workspace_root: PathBuf,
}

struct BatchItem {
    path: String,
    prepared: PreparedPatch,
}

enum ItemVerdict {
    Ready(Box<BatchItem>),
    Failed {
        path: Option<String>,
        issue: PatchIssue,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemStatus {
    Validated,
    Applied,
    RolledBack,
    RollbackFailed,
    Failed,
    Skipped,
}

impl ItemStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Validated => "validated",
            Self::Applied => "applied",
            Self::RolledBack => "rolled_back",
            Self::RollbackFailed => "rollback_failed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

impl PatchBatchFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    fn validate(&self, items: &[serde_json::Value]) -> Vec<ItemVerdict> {
        let single = PatchFileTool::new(self.workspace_root.clone());
        let mut seen = BTreeSet::new();
        let mut total_bytes = 0usize;
        items
            .iter()
            .map(|item| {
                let path = item
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                if let Some(path) = path.as_deref() {
                    if !seen.insert(path.to_string()) {
                        return ItemVerdict::Failed {
                            path: Some(path.to_string()),
                            issue: PatchIssue::new(
                                "file_patch_batch_duplicate_path",
                                "Each file.patch_batch item must target a distinct file.",
                            ),
                        };
                    }
                }
                let item_json = serde_json::to_string(item).unwrap_or_else(|_| "null".to_string());
                match single.prepare(&item_json) {
                    Ok(prepared) => {
                        total_bytes = total_bytes
                            .saturating_add(prepared.plan.before_bytes)
                            .saturating_add(prepared.plan.after.len());
                        if total_bytes > MAX_BATCH_TOTAL_BYTES {
                            return ItemVerdict::Failed {
                                path,
                                issue: PatchIssue::new(
                                    "file_patch_batch_total_too_large",
                                    "The batch exceeds the 32 MiB total content limit.",
                                ),
                            };
                        }
                        ItemVerdict::Ready(Box::new(BatchItem {
                            path: prepared.request.path.clone(),
                            prepared,
                        }))
                    }
                    Err(issue) => ItemVerdict::Failed { path, issue },
                }
            })
            .collect()
    }

    fn run_batch(
        &self,
        invocation: ToolInvocation,
        publish: &mut dyn FnMut(&PreparedPatch) -> Result<(), AtomicReplaceError>,
    ) -> Result<ToolResult, ToolError> {
        let items_value = match parse_envelope(&invocation.input_json) {
            Ok(items) => items,
            Err(issue) => return Ok(envelope_failure_result(invocation, issue)),
        };
        let verdicts = self.validate(&items_value);
        if verdicts
            .iter()
            .any(|verdict| matches!(verdict, ItemVerdict::Failed { .. }))
        {
            return Ok(validation_failure_result(invocation, &verdicts));
        }
        let items: Vec<BatchItem> = verdicts
            .into_iter()
            .map(|verdict| match verdict {
                ItemVerdict::Ready(item) => *item,
                ItemVerdict::Failed { .. } => unreachable!("failures returned above"),
            })
            .collect();

        // Capture the whole group's undo before-images before any target write.
        let undo_paths: Vec<Option<String>> = items
            .iter()
            .map(|item| {
                preserve_history_version(
                    &self.workspace_root,
                    &invocation,
                    "undo-history",
                    &item.path,
                    &item.prepared.before,
                )
                .ok()
                .map(|path| path.display().to_string())
            })
            .collect();

        // Publish deterministically in input order.
        let mut statuses = vec![ItemStatus::Applied; items.len()];
        let mut failed_at: Option<(usize, PatchIssue)> = None;
        for (index, item) in items.iter().enumerate() {
            match publish(&item.prepared) {
                Ok(()) => {}
                Err(AtomicReplaceError::Conflict { observed_sha256 }) => {
                    let mut issue = PatchIssue::retryable(
                        "file_patch_race_conflict",
                        "The workspace file changed immediately before the atomic patch was published.",
                    );
                    if let Some(observed_sha256) = observed_sha256 {
                        issue = issue.with_observed_sha256(observed_sha256);
                    }
                    failed_at = Some((index, issue));
                    break;
                }
                Err(AtomicReplaceError::Io) => {
                    failed_at = Some((
                        index,
                        PatchIssue::retryable(
                            "file_patch_publish_failed",
                            "The atomic workspace patch could not be published; the prior file was preserved when publication did not occur.",
                        ),
                    ));
                    break;
                }
            }
        }

        if let Some((failed_index, issue)) = failed_at {
            statuses[failed_index] = ItemStatus::Failed;
            for status in statuses.iter_mut().skip(failed_index + 1) {
                *status = ItemStatus::Skipped;
            }
            // Roll the applied prefix back (reverse order) from the in-memory
            // before-images; each restore is guarded by the after-hash this
            // batch itself published, so an external change fails closed.
            for index in (0..failed_index).rev() {
                statuses[index] = match restore_prepared(&items[index].prepared) {
                    Ok(()) => ItemStatus::RolledBack,
                    Err(_) => ItemStatus::RollbackFailed,
                };
            }
            return Ok(publish_failure_result(invocation, &items, &statuses, issue));
        }

        let artifact_paths: Vec<Option<String>> = items
            .iter()
            .map(|item| {
                preserve_output_version(&self.workspace_root, &invocation, &item.prepared)
                    .ok()
                    .map(|path| path.display().to_string())
            })
            .collect();
        Ok(success_result(
            invocation,
            &items,
            &undo_paths,
            &artifact_paths,
        ))
    }

    #[cfg(test)]
    pub(super) fn execute_with_publish_failure_at(
        &self,
        invocation: ToolInvocation,
        fail_at: usize,
    ) -> Result<ToolResult, ToolError> {
        let mut position = 0usize;
        let mut hook = move |prepared: &PreparedPatch| {
            let current = position;
            position += 1;
            if current == fail_at {
                return Err(AtomicReplaceError::Conflict {
                    observed_sha256: None,
                });
            }
            publish_prepared(prepared)
        };
        self.run_batch(invocation, &mut hook)
    }
}

impl Tool for PatchBatchFileTool {
    fn spec(&self) -> ToolSpec {
        batch_spec()
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Write,
            "file.patch_batch",
            "Atomically patch a bounded set of existing files in the selected workspace.",
            &batch_scope(&invocation.input_json),
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.run_batch(invocation, &mut publish_prepared)
    }
}

fn publish_prepared(prepared: &PreparedPatch) -> Result<(), AtomicReplaceError> {
    atomic_replace_preserving_permissions_if_sha256(
        &prepared.target,
        &prepared.plan.after,
        &prepared.plan.before_sha256,
        MAX_PATCH_FILE_BYTES,
    )
}

fn restore_prepared(prepared: &PreparedPatch) -> Result<(), AtomicReplaceError> {
    atomic_replace_preserving_permissions_if_sha256(
        &prepared.target,
        &prepared.before,
        &prepared.plan.after_sha256,
        MAX_PATCH_FILE_BYTES,
    )
}

fn parse_envelope(input_json: &str) -> Result<Vec<serde_json::Value>, PatchIssue> {
    if input_json.len() > MAX_BATCH_TOTAL_BYTES {
        return Err(PatchIssue::new(
            "file_patch_batch_input_too_large",
            "file.patch_batch input exceeds the 32 MiB batch limit.",
        ));
    }
    let value = serde_json::from_str::<serde_json::Value>(input_json).map_err(|_| {
        PatchIssue::new(
            "file_patch_batch_invalid_input",
            "file.patch_batch input must be one JSON object with a patches array.",
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        PatchIssue::new(
            "file_patch_batch_invalid_input",
            "file.patch_batch input must be one JSON object with a patches array.",
        )
    })?;
    if object.keys().any(|key| key != "patches") {
        return Err(PatchIssue::new(
            "file_patch_batch_invalid_input",
            "file.patch_batch input contains an unsupported field.",
        ));
    }
    let patches = object
        .get("patches")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            PatchIssue::new(
                "file_patch_batch_invalid_input",
                "file.patch_batch requires a patches array.",
            )
        })?;
    if patches.is_empty() {
        return Err(PatchIssue::new(
            "file_patch_batch_empty",
            "file.patch_batch requires at least one patch.",
        ));
    }
    if patches.len() > MAX_BATCH_PATCHES {
        return Err(PatchIssue::new(
            "file_patch_batch_too_many",
            "file.patch_batch accepts at most 16 files per batch.",
        ));
    }
    Ok(patches.clone())
}

fn batch_scope(input_json: &str) -> String {
    let paths = parse_envelope(input_json).ok().and_then(|items| {
        let mut paths = items
            .iter()
            .filter_map(|item| item.get("path").and_then(serde_json::Value::as_str))
            .filter(|path| !path.trim().is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        (!paths.is_empty()).then_some(paths)
    });
    paths
        .and_then(|paths| serde_json::to_string(&paths).ok())
        .unwrap_or_else(|| "<missing paths>".to_string())
}

fn display_paths(paths: &[&str]) -> String {
    let joined = paths.join(", ");
    if joined.chars().count() <= MAX_DISPLAY_PATHS_CHARS {
        return joined;
    }
    let mut truncated: String = joined.chars().take(MAX_DISPLAY_PATHS_CHARS).collect();
    truncated.push('…');
    truncated
}

fn item_receipt(
    path: Option<&str>,
    status: ItemStatus,
    prepared: Option<&PreparedPatch>,
    issue: Option<&PatchIssue>,
) -> serde_json::Value {
    let mut receipt = serde_json::json!({
        "path": path.unwrap_or("<invalid>"),
        "status": status.label(),
    });
    if let Some(prepared) = prepared {
        receipt["before_sha256"] = serde_json::Value::String(prepared.plan.before_sha256.clone());
        receipt["after_sha256"] = serde_json::Value::String(prepared.plan.after_sha256.clone());
        receipt["start_byte"] = serde_json::Value::from(prepared.plan.start_byte);
        receipt["end_byte"] = serde_json::Value::from(prepared.plan.end_byte);
        receipt["after_bytes"] = serde_json::Value::from(prepared.plan.after.len());
    }
    if let Some(issue) = issue {
        receipt["code"] = serde_json::Value::String(issue.code.to_string());
        receipt["message"] = serde_json::Value::String(issue.message.to_string());
        receipt["retryable"] = serde_json::Value::Bool(issue.retryable);
        if let Some(observed_sha256) = issue.observed_sha256.as_deref() {
            receipt["observed_sha256"] = serde_json::Value::String(observed_sha256.to_string());
        }
    }
    receipt
}

fn finish_result(
    invocation: ToolInvocation,
    status: ToolOutcomeStatus,
    output: impl Into<String>,
    receipt: serde_json::Value,
    failure: Option<ToolFailure>,
    facts: Metadata,
    next_action: &str,
) -> ToolResult {
    let mut result = ToolResult::text(invocation.id, status, output, facts.clone());
    result.failure = failure;
    result.structured_output_json = Some(receipt.to_string());
    result.model_observation = Some(
        ToolObservationV2::new(
            "file.patch_batch",
            receipt["summary"]
                .as_str()
                .unwrap_or("file.patch_batch")
                .to_string(),
            receipt.to_string(),
            true,
            facts,
        )
        .with_next_action(next_action),
    );
    result
}

fn envelope_failure_result(invocation: ToolInvocation, issue: PatchIssue) -> ToolResult {
    let receipt = serde_json::json!({
        "schema": BATCH_RECEIPT_SCHEMA,
        "status": "failed",
        "patches_total": 0,
        "patches_applied": 0,
        "failure_code": issue.code,
        "summary": issue.message,
        "items": [],
    });
    let facts = [
        ("failure_code".to_string(), issue.code.to_string()),
        ("retryable".to_string(), issue.retryable.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    finish_result(
        invocation,
        ToolOutcomeStatus::Failed,
        issue.message,
        receipt,
        Some(ToolFailure {
            code: issue.code.to_string(),
            message: issue.message.to_string(),
            retryable: issue.retryable,
        }),
        facts,
        "Correct the batch envelope before proposing another batch; nothing was written.",
    )
}

fn validation_failure_result(invocation: ToolInvocation, verdicts: &[ItemVerdict]) -> ToolResult {
    let items: Vec<serde_json::Value> = verdicts
        .iter()
        .map(|verdict| match verdict {
            ItemVerdict::Ready(item) => item_receipt(
                Some(&item.path),
                ItemStatus::Validated,
                Some(&item.prepared),
                None,
            ),
            ItemVerdict::Failed { path, issue } => {
                item_receipt(path.as_deref(), ItemStatus::Failed, None, Some(issue))
            }
        })
        .collect();
    let failed = verdicts
        .iter()
        .filter_map(|verdict| match verdict {
            ItemVerdict::Failed { issue, .. } => Some(issue),
            ItemVerdict::Ready(_) => None,
        })
        .collect::<Vec<_>>();
    let retryable = failed.iter().all(|issue| issue.retryable);
    let summary = format!(
        "No files were patched: {} of {} batch items failed validation.",
        failed.len(),
        verdicts.len()
    );
    let receipt = serde_json::json!({
        "schema": BATCH_RECEIPT_SCHEMA,
        "status": "failed",
        "patches_total": verdicts.len(),
        "patches_applied": 0,
        "failure_code": "file_patch_batch_validation_failed",
        "summary": summary,
        "items": items,
    });
    let mut facts = [
        (
            "failure_code".to_string(),
            "file_patch_batch_validation_failed".to_string(),
        ),
        ("retryable".to_string(), retryable.to_string()),
        ("patch_count".to_string(), verdicts.len().to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let paths = verdicts
        .iter()
        .filter_map(|verdict| match verdict {
            ItemVerdict::Ready(item) => Some(item.path.as_str()),
            ItemVerdict::Failed { path, .. } => path.as_deref(),
        })
        .collect::<Vec<_>>();
    facts.insert("path".to_string(), display_paths(&paths));
    finish_result(
        invocation,
        ToolOutcomeStatus::Failed,
        summary,
        receipt,
        Some(ToolFailure {
            code: "file_patch_batch_validation_failed".to_string(),
            message:
                "The batch was not applied; every target failed validation is reported per item."
                    .to_string(),
            retryable,
        }),
        facts,
        "Read the current files again and propose a corrected batch; nothing was written.",
    )
}

fn publish_failure_result(
    invocation: ToolInvocation,
    items: &[BatchItem],
    statuses: &[ItemStatus],
    issue: PatchIssue,
) -> ToolResult {
    let receipts: Vec<serde_json::Value> = items
        .iter()
        .zip(statuses)
        .map(|(item, status)| {
            let item_issue = (*status == ItemStatus::Failed).then_some(&issue);
            item_receipt(Some(&item.path), *status, Some(&item.prepared), item_issue)
        })
        .collect();
    let rollback_failed = statuses.contains(&ItemStatus::RollbackFailed);
    let summary = if rollback_failed {
        format!(
            "The batch hit a publish failure and the rollback left some files patched; inspect {} files before proceeding.",
            items.len()
        )
    } else {
        "The batch hit a publish race; every applied file was rolled back and nothing was left patched.".to_string()
    };
    let receipt = serde_json::json!({
        "schema": BATCH_RECEIPT_SCHEMA,
        "status": "failed",
        "patches_total": items.len(),
        "patches_applied": 0,
        "failure_code": issue.code,
        "rollback_failed": rollback_failed,
        "summary": summary,
        "items": receipts,
    });
    let paths = items
        .iter()
        .map(|item| item.path.as_str())
        .collect::<Vec<_>>();
    let facts = [
        ("path".to_string(), display_paths(&paths)),
        ("failure_code".to_string(), issue.code.to_string()),
        ("retryable".to_string(), issue.retryable.to_string()),
        ("patch_count".to_string(), items.len().to_string()),
        ("rollback_failed".to_string(), rollback_failed.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    finish_result(
        invocation,
        ToolOutcomeStatus::Failed,
        summary.clone(),
        receipt,
        Some(ToolFailure {
            code: issue.code.to_string(),
            message: summary,
            retryable: issue.retryable,
        }),
        facts,
        "Re-read the current files and propose a fresh batch against their new base hashes.",
    )
}

fn success_result(
    invocation: ToolInvocation,
    items: &[BatchItem],
    undo_paths: &[Option<String>],
    artifact_paths: &[Option<String>],
) -> ToolResult {
    let receipts: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            item_receipt(
                Some(&item.path),
                ItemStatus::Applied,
                Some(&item.prepared),
                None,
            )
        })
        .collect();
    let before_bytes: usize = items
        .iter()
        .map(|item| item.prepared.plan.before_bytes)
        .sum();
    let after_bytes: usize = items
        .iter()
        .map(|item| item.prepared.plan.after.len())
        .sum();
    let summary = format!(
        "Patched {} files atomically in input order ({} bytes -> {} bytes).",
        items.len(),
        before_bytes,
        after_bytes
    );
    let receipt = serde_json::json!({
        "schema": BATCH_RECEIPT_SCHEMA,
        "status": "applied",
        "patches_total": items.len(),
        "patches_applied": items.len(),
        "summary": summary,
        "items": receipts,
    });
    let group: Vec<serde_json::Value> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            serde_json::json!({
                "path": item.path,
                "undo_before_path": undo_paths[index],
                "after_artifact_path": artifact_paths[index],
            })
        })
        .collect();
    let snapshots_complete = group.iter().all(|entry| {
        !entry["undo_before_path"].is_null() && !entry["after_artifact_path"].is_null()
    });
    let paths = items
        .iter()
        .map(|item| item.path.as_str())
        .collect::<Vec<_>>();
    let mut facts = [
        ("path".to_string(), display_paths(&paths)),
        ("patch_count".to_string(), items.len().to_string()),
        ("undo_action".to_string(), "patched".to_string()),
        (
            "undo_group".to_string(),
            serde_json::to_string(&group).unwrap_or_else(|_| "[]".to_string()),
        ),
        (
            "snapshot_status".to_string(),
            if snapshots_complete {
                "written"
            } else {
                "partial"
            }
            .to_string(),
        ),
        ("before_bytes".to_string(), before_bytes.to_string()),
        ("after_bytes".to_string(), after_bytes.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    for (index, item) in items.iter().enumerate() {
        facts.insert(
            format!("after_sha256_{index}"),
            item.prepared.plan.after_sha256.clone(),
        );
    }
    finish_result(
        invocation,
        ToolOutcomeStatus::Succeeded,
        summary,
        receipt,
        None,
        facts,
        "Verify the patched files only if the task requires it.",
    )
}

fn batch_spec() -> ToolSpec {
    ToolSpec::builtin(
        "file.patch_batch",
        "file",
        "Atomically apply up to 16 file.patch operations to distinct existing workspace files. Every target is validated (path, base SHA-256, anchor or byte-range selector) before any file is written; a validation failure writes nothing.",
        ToolRisk::WritesWorkspace,
        serde_json::json!({
            "type": "object",
            "properties": {
                "patches": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_BATCH_PATCHES,
                    "description": "Ordered single-file patches; each item has the exact file.patch semantics.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "minLength": 1, "description": "Existing workspace-relative UTF-8 file." },
                            "expected_base_sha256": { "type": "string", "pattern": "^[0-9a-fA-F]{64}$", "description": "SHA-256 of the complete base file." },
                            "replacement": { "type": "string", "description": "Replacement UTF-8 text; may be empty." },
                            "start_byte": { "type": "integer", "minimum": 0, "maximum": MAX_PATCH_FILE_BYTES },
                            "end_byte": { "type": "integer", "minimum": 0, "maximum": MAX_PATCH_FILE_BYTES },
                            "expected_text": { "type": "string", "description": "Exact UTF-8 text expected in the selected byte range; may be empty for insertion." },
                            "anchor": { "type": "string", "minLength": 1, "description": "Exact UTF-8 text that must occur exactly once." }
                        },
                        "required": ["path", "expected_base_sha256", "replacement"],
                        "oneOf": [
                            {
                                "required": ["start_byte", "end_byte", "expected_text"],
                                "not": { "required": ["anchor"] }
                            },
                            {
                                "required": ["anchor"],
                                "not": {
                                    "anyOf": [
                                        { "required": ["start_byte"] },
                                        { "required": ["end_byte"] },
                                        { "required": ["expected_text"] }
                                    ]
                                }
                            }
                        ],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["patches"],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": BATCH_RECEIPT_SCHEMA },
                "status": { "enum": ["applied", "failed"] },
                "patches_total": { "type": "integer", "minimum": 0 },
                "patches_applied": { "type": "integer", "minimum": 0 },
                "summary": { "type": "string" },
                "items": { "type": "array" }
            },
            "required": ["schema", "status", "patches_total", "patches_applied", "summary", "items"],
            "additionalProperties": true
        })
        .to_string(),
    )
}
