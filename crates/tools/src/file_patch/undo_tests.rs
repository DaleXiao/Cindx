use super::*;
use agent_core::{Metadata, TaskId, ToolCallId, ToolOutcomeStatus};

fn invocation(input: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId("patch-call".to_string()),
        task_id: TaskId("task".to_string()),
        tool_name: "file.patch".to_string(),
        input_json: input.to_string(),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    }
}

#[test]
fn patch_captures_undo_before_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.txt");
    fs::write(&path, "base content").unwrap();
    let mut request = invocation(serde_json::json!({
        "path": "note.txt",
        "expected_base_sha256": sha256_bytes(b"base content"),
        "replacement": "next",
        "anchor": "base"
    }));
    request
        .metadata
        .insert("session_id".to_string(), "session-undo".to_string());

    let result = PatchFileTool::new(directory.path())
        .execute(request)
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(
        result.metadata.get("undo_action").map(String::as_str),
        Some("patched")
    );
    let undo_relative = result
        .metadata
        .get("undo_before_path")
        .expect("undo path recorded");
    let undo_bytes = fs::read(directory.path().join(undo_relative)).unwrap();
    assert_eq!(undo_bytes, b"base content");
    assert_eq!(
        result
            .metadata
            .get("undo_before_sha256")
            .map(String::as_str),
        Some(sha256_bytes(b"base content").as_str())
    );
}

#[test]
fn patch_undo_capture_failure_does_not_block_the_patch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.txt");
    fs::write(&path, "base content").unwrap();
    fs::write(
        directory.path().join(".cindx"),
        "blocks history directories",
    )
    .unwrap();
    let mut request = invocation(serde_json::json!({
        "path": "note.txt",
        "expected_base_sha256": sha256_bytes(b"base content"),
        "replacement": "next",
        "anchor": "base"
    }));
    request
        .metadata
        .insert("session_id".to_string(), "session-undo-blocked".to_string());

    let result = PatchFileTool::new(directory.path())
        .execute(request)
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(fs::read_to_string(&path).unwrap(), "next content");
    assert_eq!(
        result.metadata.get("undo_action").map(String::as_str),
        Some("patched"),
        "the change stays disclosed in the undo history even when capture failed"
    );
    assert!(!result.metadata.contains_key("undo_before_path"));
    assert!(!result.metadata.contains_key("undo_before_sha256"));
}
