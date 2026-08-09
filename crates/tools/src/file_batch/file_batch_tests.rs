use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::{Metadata, TaskId, ToolCallId, ToolInvocation, ToolOutcomeStatus, ToolResult};

use crate::{Tool, ToolExecutionControl};

use super::file_batch_projection::successful_item;
use super::file_batch_request::{ReadPath, MAX_BATCH_PATHS};
use super::ReadFilesTool;

fn invocation(input: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId("batch-read".to_string()),
        task_id: TaskId("task".to_string()),
        tool_name: "file.read_many".to_string(),
        input_json: input.to_string(),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    }
}

#[test]
fn reads_legacy_paths_in_first_seen_order() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("z.txt"), "last alphabetically").expect("write z");
    fs::write(root.path().join("a.txt"), "first alphabetically").expect("write a");

    let result = ReadFilesTool::new(root.path())
        .execute(invocation(serde_json::json!({
            "paths": ["z.txt", "a.txt", "z.txt"]
        })))
        .expect("batch read");

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert!(result.output.starts_with("===== z.txt ====="));
    assert!(result.output.find("z.txt").unwrap() < result.output.find("a.txt").unwrap());
    assert_eq!(
        result.metadata.get("paths_requested"),
        Some(&"2".to_string())
    );
}

#[test]
fn mixed_results_fail_top_level_and_preserve_per_file_status() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("ok.txt"), "evidence").expect("write fixture");

    let result = ReadFilesTool::new(root.path())
        .execute(invocation(serde_json::json!({
            "paths": ["ok.txt", "missing.txt"]
        })))
        .expect("batch result");
    let structured: serde_json::Value = serde_json::from_str(
        result
            .structured_output_json
            .as_deref()
            .expect("structured output"),
    )
    .expect("valid structured output");

    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    assert_eq!(structured["complete"], false);
    assert_eq!(structured["items"][0]["status"], "succeeded");
    assert_eq!(structured["items"][1]["status"], "failed");
    assert_eq!(result.metadata.get("paths_read"), Some(&"1".to_string()));
    assert_eq!(result.metadata.get("paths_failed"), Some(&"1".to_string()));
    assert!(
        !result
            .model_observation
            .expect("model observation")
            .evidence_complete
    );
}

#[test]
fn per_file_offset_exposes_an_explicit_continuation() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("long.txt"), "0123456789").expect("write fixture");

    let result = ReadFilesTool::new(root.path())
        .execute(invocation(serde_json::json!({
            "paths": [{ "path": "long.txt", "offset_bytes": 2 }],
            "max_bytes_per_file": 3
        })))
        .expect("batch read");
    let structured: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();

    assert_eq!(structured["items"][0]["offset_bytes"], 2);
    assert_eq!(structured["items"][0]["next_offset_bytes"], 5);
    assert_eq!(structured["items"][0]["truncated"], true);
    assert_eq!(structured["continuations"][0]["offset_bytes"], 5);
    assert!(result
        .model_observation
        .expect("model observation")
        .next_action
        .as_deref()
        .is_some_and(|action| action.contains("long.txt@5")));
}

#[test]
fn same_path_at_distinct_offsets_remains_two_ordered_requests() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("paged.txt"), "abcdef").expect("write fixture");

    let result = ReadFilesTool::new(root.path())
        .execute(invocation(serde_json::json!({
            "paths": [
                { "path": "paged.txt", "offset_bytes": 0 },
                { "path": "paged.txt", "offset_bytes": 3 },
                { "path": "paged.txt", "offset_bytes": 0 }
            ],
            "max_bytes_per_file": 3
        })))
        .expect("batch read");
    let structured: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();

    assert_eq!(structured["paths_requested"], 2);
    assert_eq!(structured["items"][0]["requested_offset_bytes"], 0);
    assert_eq!(structured["items"][1]["requested_offset_bytes"], 3);
}

#[test]
fn cancellation_marks_unread_paths_without_claiming_completion() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("one.txt"), "one").expect("write one");
    fs::write(root.path().join("two.txt"), "two").expect("write two");
    let checks = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&checks);
    let control = ToolExecutionControl::new(move || observed.fetch_add(1, Ordering::SeqCst) >= 1);

    let result = ReadFilesTool::new(root.path())
        .execute_with_control(
            invocation(serde_json::json!({ "paths": ["one.txt", "two.txt"] })),
            &control,
        )
        .expect("cancelled result");
    let structured: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Cancelled);
    assert_eq!(structured["cancelled"], true);
    assert_eq!(structured["complete"], false);
    assert_eq!(structured["items"][1]["status"], "cancelled");
}

#[test]
fn forwards_v2_file_read_hashes_into_batch_items() {
    let request = ReadPath {
        path: "note.txt".to_string(),
        offset_bytes: 0,
    };
    let mut result = ToolResult::text(
        ToolCallId("child".to_string()),
        ToolOutcomeStatus::Succeeded,
        "note",
        Metadata::new(),
    );
    result.structured_output_json = Some(
        serde_json::json!({
            "schema": "cindx.file-read-result.v2",
            "offset_bytes": 0,
            "returned_bytes": 4,
            "next_offset_bytes": 4,
            "total_bytes": 4,
            "truncated": false,
            "page_sha256": "a".repeat(64),
            "sha256": "b".repeat(64)
        })
        .to_string(),
    );

    let item = successful_item(&request, &result);
    assert_eq!(item["hashes"]["page_sha256"], "a".repeat(64));
    assert_eq!(item["hashes"]["sha256"], "b".repeat(64));
}

#[test]
fn enforces_batch_size_limit() {
    let paths = (0..=MAX_BATCH_PATHS)
        .map(|index| format!("{index}.txt"))
        .collect::<Vec<_>>();
    let error = ReadFilesTool::new(std::env::temp_dir())
        .execute(invocation(serde_json::json!({ "paths": paths })))
        .expect_err("oversized batch must fail");
    assert!(error.message.contains("between 1 and 8"));
}
