use super::batch::{PatchBatchFileTool, BATCH_RECEIPT_SCHEMA, MAX_BATCH_TOTAL_BYTES};
use crate::workspace_file::sha256_bytes;
use crate::Tool;
use agent_core::{
    Metadata, PermissionRisk, TaskId, ToolCallId, ToolInvocation, ToolOutcomeStatus, ToolResult,
};
use std::fs;

fn invocation(input: serde_json::Value) -> ToolInvocation {
    invocation_raw(input.to_string())
}

fn invocation_raw(input_json: String) -> ToolInvocation {
    let mut invocation = ToolInvocation {
        id: ToolCallId("batch-call".to_string()),
        task_id: TaskId("task".to_string()),
        tool_name: "file.patch_batch".to_string(),
        input_json,
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    };
    invocation
        .metadata
        .insert("session_id".to_string(), "session-batch".to_string());
    invocation
}

fn anchor_patch(path: &str, base: &[u8], anchor: &str, replacement: &str) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "expected_base_sha256": sha256_bytes(base),
        "anchor": anchor,
        "replacement": replacement,
    })
}

fn failure_code(result: &ToolResult) -> Option<&str> {
    result.failure.as_ref().map(|failure| failure.code.as_str())
}

fn receipt(result: &ToolResult) -> serde_json::Value {
    serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap()
}

#[test]
fn batch_applies_every_patch_in_input_order_with_one_group_undo_entry() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "alpha OLD a").unwrap();
    fs::write(directory.path().join("b.txt"), "beta OLD b").unwrap();
    fs::write(directory.path().join("c.txt"), "gamma OLD c").unwrap();
    let tool = PatchBatchFileTool::new(directory.path());

    let result = tool
        .execute(invocation(serde_json::json!({
            "patches": [
                anchor_patch("c.txt", b"gamma OLD c", "OLD", "new"),
                anchor_patch("a.txt", b"alpha OLD a", "OLD", "new"),
                {
                    "path": "b.txt",
                    "expected_base_sha256": sha256_bytes(b"beta OLD b"),
                    "replacement": "beta new b",
                    "start_byte": 0,
                    "end_byte": 10,
                    "expected_text": "beta OLD b"
                }
            ]
        })))
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(
        fs::read_to_string(directory.path().join("a.txt")).unwrap(),
        "alpha new a"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("b.txt")).unwrap(),
        "beta new b"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("c.txt")).unwrap(),
        "gamma new c"
    );

    let receipt = receipt(&result);
    assert_eq!(receipt["schema"], BATCH_RECEIPT_SCHEMA);
    assert_eq!(receipt["status"], "applied");
    assert_eq!(receipt["patches_total"], 3);
    assert_eq!(receipt["patches_applied"], 3);
    // The receipt and the undo group preserve the deterministic input order.
    let items = receipt["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["c.txt", "a.txt", "b.txt"]
    );
    assert!(items.iter().all(|item| item["status"] == "applied"));
    assert_eq!(items[1]["after_sha256"], sha256_bytes(b"alpha new a"));
    assert_eq!(items[2]["start_byte"], 0);
    assert_eq!(items[2]["end_byte"], 10);

    assert_eq!(
        result.metadata.get("undo_action").map(String::as_str),
        Some("patched")
    );
    assert_eq!(
        result.metadata.get("snapshot_status").map(String::as_str),
        Some("written")
    );
    let group: Vec<serde_json::Value> =
        serde_json::from_str(&result.metadata["undo_group"]).unwrap();
    assert_eq!(group.len(), 3);
    assert_eq!(
        group
            .iter()
            .map(|entry| entry["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["c.txt", "a.txt", "b.txt"]
    );
    // Every before-image shares the one invocation-scoped group directory.
    let group_dir = directory
        .path()
        .join(group[0]["undo_before_path"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(group_dir.starts_with(directory.path().join(".cindx/undo-history")));
    for (entry, before) in group.iter().zip([
        b"gamma OLD c".as_slice(),
        b"alpha OLD a".as_slice(),
        b"beta OLD b".as_slice(),
    ]) {
        let undo_path = entry["undo_before_path"].as_str().unwrap();
        let artifact_path = entry["after_artifact_path"].as_str().unwrap();
        assert_eq!(
            fs::read(directory.path().join(undo_path))
                .unwrap()
                .as_slice(),
            before
        );
        assert_eq!(
            directory.path().join(undo_path).parent().unwrap(),
            group_dir
        );
        assert!(directory.path().join(artifact_path).is_file());
    }
}

#[test]
fn batch_validation_failure_writes_nothing_and_reports_each_item() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "alpha base").unwrap();
    fs::write(directory.path().join("b.txt"), "beta base").unwrap();
    fs::write(directory.path().join("c.txt"), "gamma base").unwrap();
    let tool = PatchBatchFileTool::new(directory.path());

    let result = tool
        .execute(invocation(serde_json::json!({
            "patches": [
                anchor_patch("a.txt", b"alpha base", "base", "new"),
                anchor_patch("b.txt", b"beta base", "MISSING", "new"),
                anchor_patch("c.txt", b"stale base", "base", "new")
            ]
        })))
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        failure_code(&result),
        Some("file_patch_batch_validation_failed")
    );
    for (name, expected) in [
        ("a.txt", "alpha base"),
        ("b.txt", "beta base"),
        ("c.txt", "gamma base"),
    ] {
        assert_eq!(
            fs::read_to_string(directory.path().join(name)).unwrap(),
            expected
        );
    }
    // Zero writes: no snapshot or history directory was created at all.
    assert!(!directory.path().join(".cindx").exists());

    let receipt = receipt(&result);
    assert_eq!(receipt["patches_applied"], 0);
    let items = receipt["items"].as_array().unwrap();
    assert_eq!(items[0]["status"], "validated");
    assert_eq!(items[1]["status"], "failed");
    assert_eq!(items[1]["code"], "file_patch_anchor_missing");
    assert_eq!(items[2]["status"], "failed");
    assert_eq!(items[2]["code"], "file_patch_stale_base");
}

#[test]
fn batch_rejects_paths_outside_the_workspace() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "alpha base").unwrap();
    let tool = PatchBatchFileTool::new(directory.path());

    let result = tool
        .execute(invocation(serde_json::json!({
            "patches": [
                anchor_patch("a.txt", b"alpha base", "base", "new"),
                anchor_patch("../escape.txt", b"whatever", "base", "new")
            ]
        })))
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        fs::read_to_string(directory.path().join("a.txt")).unwrap(),
        "alpha base"
    );
    let items = receipt(&result);
    let items = items["items"].as_array().unwrap();
    assert_eq!(items[1]["code"], "file_patch_invalid_path");
}

#[test]
fn batch_rejects_duplicate_paths_and_oversized_batches() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "alpha base").unwrap();
    let tool = PatchBatchFileTool::new(directory.path());

    let duplicate = tool
        .execute(invocation(serde_json::json!({
            "patches": [
                anchor_patch("a.txt", b"alpha base", "base", "new"),
                anchor_patch("a.txt", b"alpha base", "alpha", "new")
            ]
        })))
        .unwrap();
    assert_eq!(duplicate.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        fs::read_to_string(directory.path().join("a.txt")).unwrap(),
        "alpha base"
    );
    let items = receipt(&duplicate);
    assert_eq!(items["items"][1]["code"], "file_patch_batch_duplicate_path");

    let oversized: Vec<serde_json::Value> = (0..17)
        .map(|index| {
            let mut item = anchor_patch("a.txt", b"alpha base", "base", "new");
            item["path"] = serde_json::Value::String(format!("f{index}.txt"));
            item
        })
        .collect();
    let too_many = tool
        .execute(invocation(serde_json::json!({ "patches": oversized })))
        .unwrap();
    assert_eq!(too_many.status, ToolOutcomeStatus::Failed);
    assert_eq!(failure_code(&too_many), Some("file_patch_batch_too_many"));
    assert!(receipt(&too_many)["items"].as_array().unwrap().is_empty());
}

#[test]
fn batch_enforces_the_total_byte_bound() {
    let directory = tempfile::tempdir().unwrap();
    let base = "x".repeat(8 * 1024 * 1024);
    let mut patches = Vec::new();
    for index in 0..3 {
        let name = format!("big{index}.txt");
        fs::write(directory.path().join(&name), &base).unwrap();
        patches.push(serde_json::json!({
            "path": name,
            "expected_base_sha256": sha256_bytes(base.as_bytes()),
            "replacement": "y",
            "start_byte": 0,
            "end_byte": 1,
            "expected_text": "x"
        }));
    }
    let tool = PatchBatchFileTool::new(directory.path());

    let result = tool
        .execute(invocation(serde_json::json!({ "patches": patches })))
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    let items = receipt(&result);
    let items = items["items"].as_array().unwrap();
    assert_eq!(items[0]["status"], "validated");
    assert_eq!(items[1]["status"], "validated");
    assert_eq!(items[2]["code"], "file_patch_batch_total_too_large");
    for index in 0..3 {
        assert_eq!(
            fs::metadata(directory.path().join(format!("big{index}.txt")))
                .unwrap()
                .len(),
            8 * 1024 * 1024
        );
    }
}

#[test]
fn batch_publish_race_rolls_back_the_applied_prefix() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "alpha base").unwrap();
    fs::write(directory.path().join("b.txt"), "beta base").unwrap();
    fs::write(directory.path().join("c.txt"), "gamma base").unwrap();
    let tool = PatchBatchFileTool::new(directory.path());

    let result = tool
        .execute_with_publish_failure_at(
            invocation(serde_json::json!({
                "patches": [
                    anchor_patch("a.txt", b"alpha base", "base", "new"),
                    anchor_patch("b.txt", b"beta base", "base", "new"),
                    anchor_patch("c.txt", b"gamma base", "base", "new")
                ]
            })),
            1,
        )
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Failed);
    assert_eq!(failure_code(&result), Some("file_patch_race_conflict"));
    for (name, expected) in [
        ("a.txt", "alpha base"),
        ("b.txt", "beta base"),
        ("c.txt", "gamma base"),
    ] {
        assert_eq!(
            fs::read_to_string(directory.path().join(name)).unwrap(),
            expected,
            "{name} must hold its original content"
        );
    }
    let receipt = receipt(&result);
    assert_eq!(receipt["patches_applied"], 0);
    assert_eq!(receipt["rollback_failed"], false);
    let items = receipt["items"].as_array().unwrap();
    assert_eq!(items[0]["status"], "rolled_back");
    assert_eq!(items[1]["status"], "failed");
    assert_eq!(items[1]["code"], "file_patch_race_conflict");
    assert_eq!(items[2]["status"], "skipped");
}

#[test]
fn batch_permission_request_binds_the_exact_sorted_path_set() {
    let tool = PatchBatchFileTool::new("/tmp/unused-workspace");
    let first = tool
        .permission_request(&invocation(serde_json::json!({
            "patches": [
                anchor_patch("b.txt", b"beta base", "base", "new"),
                anchor_patch("a.txt", b"alpha base", "base", "new")
            ]
        })))
        .expect("batch patch requires permission");
    let reordered = tool
        .permission_request(&invocation(serde_json::json!({
            "patches": [
                anchor_patch("a.txt", b"alpha base", "base", "new"),
                anchor_patch("b.txt", b"beta base", "base", "new")
            ]
        })))
        .expect("batch patch requires permission");
    let extended = tool
        .permission_request(&invocation(serde_json::json!({
            "patches": [
                anchor_patch("a.txt", b"alpha base", "base", "new"),
                anchor_patch("b.txt", b"beta base", "base", "new"),
                anchor_patch("c.txt", b"gamma base", "base", "new")
            ]
        })))
        .expect("batch patch requires permission");
    let malformed = tool
        .permission_request(&invocation(serde_json::json!({ "patches": "nope" })))
        .expect("batch patch requires permission");

    assert_eq!(first.action, "file.patch_batch");
    assert_eq!(first.risk, PermissionRisk::Write);
    // Session reuse requires the identical path set, independent of input order.
    assert_eq!(first.scope, "[\"a.txt\",\"b.txt\"]");
    assert_eq!(first.scope, reordered.scope);
    assert_eq!(first.id, reordered.id);
    assert_ne!(first.scope, extended.scope);
    assert_ne!(first.id, extended.id);
    assert_eq!(malformed.scope, "<missing paths>");
}

#[test]
fn batch_rejects_malformed_envelopes() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "alpha base").unwrap();
    let tool = PatchBatchFileTool::new(directory.path());

    for input in [
        serde_json::json!(["not-an-object"]),
        serde_json::json!({ "other": [] }),
        serde_json::json!({ "patches": [] }),
    ] {
        let result = tool.execute(invocation(input)).unwrap();
        assert_eq!(result.status, ToolOutcomeStatus::Failed);
        assert_eq!(
            fs::read_to_string(directory.path().join("a.txt")).unwrap(),
            "alpha base"
        );
    }

    let non_object_item = tool
        .execute(invocation(serde_json::json!({ "patches": ["nope"] })))
        .unwrap();
    assert_eq!(non_object_item.status, ToolOutcomeStatus::Failed);
    assert_eq!(
        receipt(&non_object_item)["items"][0]["code"],
        "file_patch_invalid_input"
    );

    let oversized_input = serde_json::json!({
        "patches": [{
            "path": "a.txt",
            "expected_base_sha256": sha256_bytes(b"alpha base"),
            "replacement": "x".repeat(MAX_BATCH_TOTAL_BYTES),
            "anchor": "base"
        }]
    })
    .to_string();
    let too_large = tool.execute(invocation_raw(oversized_input)).unwrap();
    assert_eq!(
        failure_code(&too_large),
        Some("file_patch_batch_input_too_large")
    );
}

#[test]
fn batch_undo_capture_failure_is_disclosed_without_blocking_the_patch() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "alpha base").unwrap();
    fs::write(directory.path().join("b.txt"), "beta base").unwrap();
    fs::write(
        directory.path().join(".cindx"),
        "blocks history directories",
    )
    .unwrap();
    let tool = PatchBatchFileTool::new(directory.path());

    let result = tool
        .execute(invocation(serde_json::json!({
            "patches": [
                anchor_patch("a.txt", b"alpha base", "base", "new"),
                anchor_patch("b.txt", b"beta base", "base", "new")
            ]
        })))
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(
        fs::read_to_string(directory.path().join("a.txt")).unwrap(),
        "alpha new"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("b.txt")).unwrap(),
        "beta new"
    );
    assert_eq!(
        result.metadata.get("undo_action").map(String::as_str),
        Some("patched")
    );
    assert_eq!(
        result.metadata.get("snapshot_status").map(String::as_str),
        Some("partial")
    );
    let group: Vec<serde_json::Value> =
        serde_json::from_str(&result.metadata["undo_group"]).unwrap();
    assert!(
        group
            .iter()
            .all(|entry| entry["undo_before_path"].is_null()
                && entry["after_artifact_path"].is_null())
    );
}
