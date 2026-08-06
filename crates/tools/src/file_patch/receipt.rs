use super::contract::{PatchIssue, PatchPlan, PATCH_FAILURE_SCHEMA, PATCH_RECEIPT_SCHEMA};
use agent_core::{
    Metadata, ToolFailure, ToolInvocation, ToolObservationV2, ToolOutcomeStatus, ToolResult,
};
use std::path::Path;

pub(super) fn successful_patch_result(
    invocation: ToolInvocation,
    path: &str,
    source_path: &Path,
    plan: PatchPlan,
) -> ToolResult {
    let diff_preview = format!(
        "bytes {}..{}: -{} +{}; diff_sha256={}",
        plan.start_byte,
        plan.end_byte,
        plan.replaced_bytes,
        plan.replacement_bytes,
        &plan.diff_sha256[..12]
    );
    let receipt = serde_json::json!({
        "schema": PATCH_RECEIPT_SCHEMA,
        "path": path,
        "before_sha256": plan.before_sha256,
        "after_sha256": plan.after_sha256,
        "diff_sha256": plan.diff_sha256,
        "start_byte": plan.start_byte,
        "end_byte": plan.end_byte,
        "before_bytes": plan.before_bytes,
        "after_bytes": plan.after.len(),
        "replaced_bytes": plan.replaced_bytes,
        "replacement_bytes": plan.replacement_bytes,
    });
    let facts = [
        ("path".to_string(), path.to_string()),
        ("source_path".to_string(), source_path.display().to_string()),
        ("bytes".to_string(), plan.after.len().to_string()),
        ("before_sha256".to_string(), plan.before_sha256.clone()),
        ("after_sha256".to_string(), plan.after_sha256.clone()),
        ("diff_sha256".to_string(), plan.diff_sha256.clone()),
        ("diff_preview".to_string(), diff_preview.clone()),
        ("start_byte".to_string(), plan.start_byte.to_string()),
        ("end_byte".to_string(), plan.end_byte.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let output = format!(
        "Patched {path} atomically at bytes {}..{} ({} bytes -> {} bytes).",
        plan.start_byte,
        plan.end_byte,
        plan.before_bytes,
        plan.after.len()
    );
    let mut result = ToolResult::text(
        invocation.id,
        ToolOutcomeStatus::Succeeded,
        output,
        facts.clone(),
    );
    result.structured_output_json = Some(receipt.to_string());
    result.model_observation = Some(ToolObservationV2::new(
        "file.patch",
        format!(
            "Atomically patched {path} at bytes {}..{}.",
            plan.start_byte, plan.end_byte
        ),
        format!("{diff_preview}\nreceipt={receipt}"),
        true,
        facts,
    ));
    result
}

pub(super) fn failed_patch_result(
    invocation: ToolInvocation,
    path: Option<&str>,
    issue: PatchIssue,
) -> ToolResult {
    let path = path.unwrap_or("<invalid>");
    let mut receipt = serde_json::json!({
        "schema": PATCH_FAILURE_SCHEMA,
        "path": path,
        "code": issue.code,
        "retryable": issue.retryable,
    });
    if let Some(observed_sha256) = issue.observed_sha256.as_deref() {
        receipt["observed_sha256"] = serde_json::Value::String(observed_sha256.to_string());
    }
    let mut facts = [
        ("path".to_string(), path.to_string()),
        ("failure_code".to_string(), issue.code.to_string()),
        ("retryable".to_string(), issue.retryable.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(observed_sha256) = issue.observed_sha256 {
        facts.insert("observed_sha256".to_string(), observed_sha256);
    }
    let mut result = ToolResult::text(
        invocation.id,
        ToolOutcomeStatus::Failed,
        issue.message,
        facts.clone(),
    );
    result.failure = Some(ToolFailure {
        code: issue.code.to_string(),
        message: issue.message.to_string(),
        retryable: issue.retryable,
    });
    result.structured_output_json = Some(receipt.to_string());
    result.model_observation = Some(
        ToolObservationV2::new(
            "file.patch",
            issue.message,
            format!("code={}", issue.code),
            true,
            facts,
        )
        .with_next_action(if issue.retryable {
            "Read the current file again and propose a patch against its new SHA-256."
        } else {
            "Correct the selector or target before proposing another patch."
        }),
    );
    result
}
