use super::*;
use agent_core::{EventKind, Metadata, TaskId};
use agent_storage::SqliteStore;
use std::fs;
use tempfile::tempdir;

fn append_event(store: &mut SqliteStore, session_id: &str, metadata: Metadata) {
    let mut metadata = metadata;
    metadata.insert("session_id".to_string(), session_id.to_string());
    crate::event_persistence::append_event(
        store,
        &TaskId("managed-artifact-lifecycle-test".to_string()),
        EventKind::ToolCallFinished,
        "artifact fixture",
        metadata,
    )
    .expect("fixture event should append");
}

fn metadata(key: &str, value: impl Into<String>) -> Metadata {
    [(key.to_string(), value.into())].into_iter().collect()
}

fn managed_file(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("fixture should have a parent"))
        .expect("fixture parent should exist");
    fs::write(&path, b"artifact").expect("fixture should write");
    path
}

#[test]
fn missing_workspace_has_nothing_to_retire() {
    let workspace = tempdir().expect("workspace parent should exist");
    let missing = workspace.path().join("removed-workspace");
    let store = SqliteStore::in_memory().expect("store should open");

    let plan = plan_managed_artifact_retirement(&store, &missing, &["deleted".to_string()])
        .expect("missing workspace should be an empty retirement");

    assert!(plan.candidates.is_empty());
    assert_eq!(plan.preserved_live_paths, 0);
}

#[test]
fn shared_fork_reference_preserves_artifact_until_its_last_session_is_retired() {
    let workspace = tempdir().expect("workspace should exist");
    let artifact = managed_file(
        workspace.path(),
        ".cindx/output-history/session/version/result.md",
    );
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        "source",
        metadata("result_artifact_path", artifact.display().to_string()),
    );
    append_event(
        &mut store,
        "fork",
        metadata("result_artifact_path", artifact.display().to_string()),
    );

    let source =
        plan_managed_artifact_retirement(&store, workspace.path(), &["source".to_string()])
            .expect("source retirement should plan");
    assert!(source.candidates.is_empty());
    assert_eq!(source.preserved_live_paths, 1);
    assert!(artifact.is_file());

    let all = plan_managed_artifact_retirement(
        &store,
        workspace.path(),
        &["source".to_string(), "fork".to_string()],
    )
    .expect("last-reference retirement should plan");
    let stats = apply_managed_artifact_retirement(&all).expect("artifact should retire");
    assert_eq!(stats.planned_paths, 1);
    assert_eq!(stats.removed_files, 1);
    assert!(!artifact.exists());
}

#[test]
fn scalar_json_queue_and_multiline_references_are_retired_in_deterministic_order() {
    let workspace = tempdir().expect("workspace should exist");
    let scalar = managed_file(workspace.path(), ".cindx/artifacts/call/structured.json");
    let stdout = managed_file(workspace.path(), ".cindx/tool-output/call/stdout.log");
    let image = managed_file(workspace.path(), ".cindx/attachments/session/image.png");
    let queued = managed_file(workspace.path(), ".cindx/attachments/session/queued.txt");
    let mut values = Metadata::new();
    values.insert(
        "result_structured_output_path".to_string(),
        scalar.display().to_string(),
    );
    values.insert(
        "result_artifacts_json".to_string(),
        serde_json::json!([{ "path": stdout }]).to_string(),
    );
    values.insert("image_paths".to_string(), image.display().to_string());
    values.insert(
        "queue_payload".to_string(),
        serde_json::json!({ "attachments": [{ "path": queued }] }).to_string(),
    );
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(&mut store, "deleted", values);

    let plan = plan_managed_artifact_retirement(&store, workspace.path(), &["deleted".to_string()])
        .expect("retirement should plan");
    let ordered = plan
        .candidates
        .iter()
        .map(|candidate| candidate.path.clone())
        .collect::<Vec<_>>();
    let mut expected = ordered.clone();
    expected.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });
    assert_eq!(ordered, expected);

    let stats = apply_managed_artifact_retirement(&plan).expect("artifacts should retire");
    assert_eq!(stats.planned_paths, 4);
    assert_eq!(stats.removed_files, 4);
    assert_eq!(stats.preserved_live_paths, 0);
}

#[test]
fn malformed_json_custom_workspace_and_unknown_cindx_paths_are_ignored() {
    let workspace = tempdir().expect("workspace should exist");
    let deliverable = managed_file(workspace.path(), "deliverable.md");
    let custom = managed_file(workspace.path(), ".cindx/custom/keep.txt");
    let outside = tempdir().expect("outside directory should exist");
    let outside_file = managed_file(outside.path(), "keep.txt");
    let mut values = Metadata::new();
    values.insert("result_artifacts_json".to_string(), "[{broken".to_string());
    values.insert(
        "result_artifact_path".to_string(),
        deliverable.display().to_string(),
    );
    values.insert(
        "result_source_path".to_string(),
        custom.display().to_string(),
    );
    values.insert(
        "result_text_path".to_string(),
        outside_file.display().to_string(),
    );
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(&mut store, "deleted", values);

    let plan = plan_managed_artifact_retirement(&store, workspace.path(), &["deleted".to_string()])
        .expect("unknown references should be ignored");
    assert!(plan.candidates.is_empty());
    assert!(deliverable.is_file());
    assert!(custom.is_file());
    assert!(outside_file.is_file());
}

#[cfg(unix)]
#[test]
fn symlinked_managed_root_fails_closed_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().expect("workspace should exist");
    let outside = tempdir().expect("outside directory should exist");
    let outside_file = managed_file(outside.path(), "keep.txt");
    fs::create_dir_all(workspace.path().join(".cindx")).expect("cindx root should exist");
    symlink(
        outside.path(),
        workspace.path().join(".cindx/output-history"),
    )
    .expect("managed root symlink should write");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        "deleted",
        metadata("result_artifact_path", ".cindx/output-history/keep.txt"),
    );

    let error =
        plan_managed_artifact_retirement(&store, workspace.path(), &["deleted".to_string()])
            .expect_err("symlinked root must fail closed");
    assert!(error.contains("symbolic link"));
    assert!(outside_file.is_file());
}

#[test]
fn active_browser_session_blocks_cleanup_and_closed_cleanup_is_idempotent() {
    let workspace = tempdir().expect("workspace should exist");
    let session = workspace.path().join(".cindx/browser-sessions/session-abc");
    fs::create_dir_all(session.join("profile")).expect("session profile should exist");
    fs::write(session.join("profile/data"), b"profile").expect("profile should write");
    fs::write(session.join("session-state.json"), b"{}").expect("state should write");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        "deleted",
        metadata("result_session_path", ".cindx/browser-sessions/session-abc"),
    );
    let plan = plan_managed_artifact_retirement(&store, workspace.path(), &["deleted".to_string()])
        .expect("browser retirement should plan");

    let error = apply_managed_artifact_retirement(&plan)
        .expect_err("active browser session must block retirement");
    assert!(error.contains("active or locked"));
    assert!(session.is_dir());

    fs::remove_file(session.join("session-state.json")).expect("state should remove");
    let first = apply_managed_artifact_retirement(&plan).expect("closed session should retire");
    assert_eq!(first.removed_directories, 1);
    assert!(!session.exists());
    let second = apply_managed_artifact_retirement(&plan).expect("cleanup should be idempotent");
    assert_eq!(second.planned_paths, 1);
    assert_eq!(second.already_absent, 1);
    assert_eq!(second.removed_directories, 0);
}

#[test]
fn parent_traversal_is_rejected_without_removing_anything() {
    let workspace = tempdir().expect("workspace should exist");
    let keep = managed_file(workspace.path(), ".cindx/output-history/keep.txt");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        "deleted",
        metadata(
            "result_artifact_path",
            ".cindx/output-history/../output-history/keep.txt",
        ),
    );

    let error =
        plan_managed_artifact_retirement(&store, workspace.path(), &["deleted".to_string()])
            .expect_err("parent traversal must be rejected");
    assert!(error.contains("parent traversal"));
    assert!(keep.is_file());
}
