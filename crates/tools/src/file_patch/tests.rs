use super::*;
use agent_core::{Metadata, TaskId, ToolCallId, ToolOutcomeStatus};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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

fn failure_code(result: &ToolResult) -> Option<&str> {
    result.failure.as_ref().map(|failure| failure.code.as_str())
}

#[test]
fn range_patch_is_atomic_receipted_and_permission_preserving() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.txt");
    fs::write(&path, "hello old world\n").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let base_sha256 = sha256_bytes(b"hello old world\n");
    let mut request = invocation(serde_json::json!({
        "path": "note.txt",
        "expected_base_sha256": base_sha256,
        "replacement": "new",
        "start_byte": 6,
        "end_byte": 9,
        "expected_text": "old"
    }));
    request
        .metadata
        .insert("session_id".to_string(), "session-alpha".to_string());
    let tool = PatchFileTool::new(directory.path());
    let verifier = tool
        .effect_spec(&request)
        .effect_semantics
        .verifier()
        .unwrap()
        .to_string();

    let result = tool.execute(request).unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello new world\n");
    assert_eq!(
        verifier,
        format!(
            "workspace_file_sha256_v1:{}",
            sha256_bytes(b"hello new world\n")
        )
    );
    let receipt: serde_json::Value =
        serde_json::from_str(result.structured_output_json.as_deref().unwrap()).unwrap();
    assert_eq!(receipt["schema"], contract::PATCH_RECEIPT_SCHEMA);
    assert_eq!(receipt["start_byte"], 6);
    assert_eq!(receipt["end_byte"], 9);
    assert_eq!(receipt["before_bytes"], 16);
    assert_eq!(receipt["after_bytes"], 16);
    assert_eq!(receipt["after_sha256"], sha256_bytes(b"hello new world\n"));
    assert_eq!(receipt["diff_sha256"].as_str().unwrap().len(), 64);
    for key in [
        "path",
        "source_path",
        "bytes",
        "before_sha256",
        "after_sha256",
        "diff_preview",
    ] {
        assert!(result.metadata.contains_key(key), "missing metadata {key}");
    }
    assert!(result.metadata["diff_preview"].len() < 160);
    assert!(!result.metadata["diff_preview"].contains("old"));
    assert!(!result.metadata["diff_preview"].contains("new"));
    assert_eq!(result.metadata["snapshot_status"], "written");
    assert_eq!(
        fs::read(directory.path().join(&result.metadata["artifact_path"])).unwrap(),
        b"hello new world\n"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn unique_anchor_succeeds_but_ambiguous_anchor_does_not_mutate() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.txt");
    fs::write(&path, "one TARGET two").unwrap();
    let tool = PatchFileTool::new(directory.path());
    let result = tool
        .execute(invocation(serde_json::json!({
            "path": "note.txt",
            "expected_base_sha256": sha256_bytes(b"one TARGET two"),
            "replacement": "done",
            "anchor": "TARGET"
        })))
        .unwrap();
    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(fs::read_to_string(&path).unwrap(), "one done two");

    fs::write(&path, "x TARGET y TARGET z").unwrap();
    let result = tool
        .execute(invocation(serde_json::json!({
            "path": "note.txt",
            "expected_base_sha256": sha256_bytes(b"x TARGET y TARGET z"),
            "replacement": "done",
            "anchor": "TARGET"
        })))
        .unwrap();
    assert_eq!(failure_code(&result), Some("file_patch_anchor_ambiguous"));
    assert_eq!(fs::read_to_string(path).unwrap(), "x TARGET y TARGET z");
}

#[test]
fn stale_or_mixed_selector_fails_before_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.txt");
    fs::write(&path, "base").unwrap();
    let tool = PatchFileTool::new(directory.path());
    let stale = tool
        .execute(invocation(serde_json::json!({
            "path": "note.txt",
            "expected_base_sha256": sha256_bytes(b"older"),
            "replacement": "next",
            "anchor": "base"
        })))
        .unwrap();
    assert_eq!(failure_code(&stale), Some("file_patch_stale_base"));

    let mixed = tool
        .execute(invocation(serde_json::json!({
            "path": "note.txt",
            "expected_base_sha256": sha256_bytes(b"base"),
            "replacement": "next",
            "anchor": "base",
            "start_byte": 0,
            "end_byte": 4,
            "expected_text": "base"
        })))
        .unwrap();
    assert_eq!(failure_code(&mixed), Some("file_patch_mixed_selectors"));
    assert_eq!(fs::read_to_string(path).unwrap(), "base");
}

#[test]
fn injected_race_and_publish_failure_preserve_the_observed_target() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.txt");
    fs::write(&path, "base").unwrap();
    let tool = PatchFileTool::new(directory.path());
    let request = || {
        invocation(serde_json::json!({
            "path": "note.txt",
            "expected_base_sha256": sha256_bytes(b"base"),
            "replacement": "next",
            "anchor": "base"
        }))
    };
    let raced = tool
        .execute_with_hooks(
            request(),
            |target| fs::write(target, "racing writer"),
            |temporary, target| {
                temporary
                    .persist(target)
                    .map_err(|error| error.error)
                    .map(|_| ())
            },
        )
        .unwrap();
    assert_eq!(failure_code(&raced), Some("file_patch_race_conflict"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "racing writer");

    fs::write(&path, "base").unwrap();
    let failed = tool
        .execute_with_hooks(
            request(),
            |_| Ok(()),
            |_temporary, _target| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected publish failure",
                ))
            },
        )
        .unwrap();
    assert_eq!(failure_code(&failed), Some("file_patch_publish_failed"));
    assert_eq!(fs::read_to_string(path).unwrap(), "base");
}

#[test]
fn invalid_utf8_and_missing_targets_have_stable_codes() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("binary.bin"), [0xff, 0xfe]).unwrap();
    let tool = PatchFileTool::new(directory.path());
    let binary = tool
        .execute(invocation(serde_json::json!({
            "path": "binary.bin",
            "expected_base_sha256": sha256_bytes(&[0xff, 0xfe]),
            "replacement": "x",
            "anchor": "x"
        })))
        .unwrap();
    assert_eq!(failure_code(&binary), Some("file_patch_target_not_utf8"));

    let missing = tool
        .execute(invocation(serde_json::json!({
            "path": "missing.txt",
            "expected_base_sha256": sha256_bytes(b""),
            "replacement": "x",
            "anchor": "x"
        })))
        .unwrap();
    assert_eq!(failure_code(&missing), Some("file_patch_target_missing"));
}

#[test]
fn snapshot_failure_is_explicit_but_does_not_reverse_the_applied_patch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("note.txt");
    fs::write(&path, "base").unwrap();
    fs::write(directory.path().join(".cindx"), "blocks snapshot directory").unwrap();
    let result = PatchFileTool::new(directory.path())
        .execute(invocation(serde_json::json!({
            "path": "note.txt",
            "expected_base_sha256": sha256_bytes(b"base"),
            "replacement": "next",
            "anchor": "base"
        })))
        .unwrap();

    assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
    assert_eq!(fs::read_to_string(path).unwrap(), "next");
    assert_eq!(result.metadata["snapshot_status"], "failed");
    assert_eq!(
        result.metadata["snapshot_error_code"],
        "output_history_unavailable"
    );
    assert!(!result.metadata.contains_key("artifact_path"));
}
