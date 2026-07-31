use super::*;
use crate::{
    agent_read_model::{active_agent_events_for_session, agent_state_for_session},
    configuration_models::ProjectSessionConfig,
    queue_service::{pending_queued_agent_messages, QueuedAgentMessagePayload},
    view_models::AgentAttachmentView,
};
use agent_core::{
    decode_event_type, DecodedEventType, EventKind, EventTypeV1, PermissionRequest,
    PermissionRequestId, PermissionRisk,
};
use agent_storage::PermissionStore;
use std::fs;
use tempfile::tempdir;

fn session_config(root: &Path, session_ids: &[&str]) -> ProjectSessionConfig {
    let mut config = ProjectSessionConfig::default_for_root(root);
    let project_id = config.projects[0].id.clone();
    config.sessions = session_ids
        .iter()
        .map(|session_id| crate::configuration_models::SessionRecord {
            id: (*session_id).to_string(),
            project_id: project_id.clone(),
            name: format!("Session {session_id}"),
            title_state: agent_application::SessionTitleState::Manual,
            detail: "timeline + chat".to_string(),
            effort: "auto".to_string(),
            seen_event_sequence: 0,
            created_at_ms: 1,
            updated_at_ms: 1,
            archived_at_ms: None,
        })
        .collect();
    config.active_session_id = session_ids.first().copied().unwrap_or_default().to_string();
    config
}

fn append_test_event(store: &mut SqliteStore, kind: EventKind, summary: &str, metadata: Metadata) {
    append_event(store, &phase16_task_id(), kind, summary, metadata)
        .expect("test event should append");
}

fn session_metadata(session_id: &str) -> Metadata {
    [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect()
}

fn append_active_fork_source(store: &mut SqliteStore, final_status: Option<&str>) {
    let mut start_metadata = session_metadata("source");
    start_metadata.insert("agent_run_id".to_string(), "run-source".to_string());
    for key in FORK_RECOVERY_METADATA_KEYS {
        start_metadata.insert(key.to_string(), format!("source-{key}"));
    }
    append_test_event(
        store,
        EventKind::TaskStatusChanged,
        "Agent task started",
        start_metadata,
    );
    append_test_event(
        store,
        EventKind::MessageAdded,
        "user message",
        [
            ("session_id".to_string(), "source".to_string()),
            ("agent_run_id".to_string(), "run-source".to_string()),
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), "inspect this".to_string()),
            ("model_content".to_string(), "inspect this".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    append_test_event(
        store,
        EventKind::ToolCallStarted,
        "tool started",
        [
            ("session_id".to_string(), "source".to_string()),
            ("agent_run_id".to_string(), "run-source".to_string()),
            ("tool_call_id".to_string(), "tool-source".to_string()),
            ("tool".to_string(), "workspace.read".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    append_test_event(
        store,
        EventKind::PermissionRequested,
        "permission requested",
        [
            ("session_id".to_string(), "source".to_string()),
            ("agent_run_id".to_string(), "run-source".to_string()),
            ("permission_id".to_string(), "permission-source".to_string()),
            ("tool_call_id".to_string(), "tool-source".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    store
        .save_permission_request(
            PermissionRequest {
                id: PermissionRequestId("permission-source".to_string()),
                task_id: phase16_task_id(),
                risk: PermissionRisk::Read,
                action: "workspace.read".to_string(),
                reason: "fixture".to_string(),
                scope: "source".to_string(),
                metadata: [
                    ("session_id".to_string(), "source".to_string()),
                    ("agent_run_id".to_string(), "run-source".to_string()),
                    ("tool_call_id".to_string(), "tool-source".to_string()),
                    ("tool".to_string(), "workspace.read".to_string()),
                    ("tool_input".to_string(), "{}".to_string()),
                ]
                .into_iter()
                .collect(),
            },
            10,
        )
        .expect("source permission should persist");
    let payload = QueuedAgentMessagePayload {
        prompt: "queued follow-up".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: String::new(),
    };
    append_test_event(
        store,
        EventKind::TaskStatusChanged,
        "Agent message queued",
        [
            ("session_id".to_string(), "source".to_string()),
            ("agent_run_id".to_string(), "run-source".to_string()),
            ("queue_action".to_string(), "enqueue".to_string()),
            ("queue_id".to_string(), "queue-source".to_string()),
            ("queue_mode".to_string(), "queue".to_string()),
            ("queue_created_at_ms".to_string(), "10".to_string()),
            (
                "queue_payload".to_string(),
                serde_json::to_string(&payload).expect("queue payload should encode"),
            ),
        ]
        .into_iter()
        .collect(),
    );
    if let Some(summary) = final_status {
        append_test_event(
            store,
            EventKind::TaskStatusChanged,
            summary,
            [
                ("session_id".to_string(), "source".to_string()),
                ("agent_run_id".to_string(), "run-source".to_string()),
            ]
            .into_iter()
            .collect(),
        );
    }
}

#[test]
fn fork_attribution_uses_the_complete_phase16_stream_before_session_filtering() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_test_event(
        &mut store,
        EventKind::TaskStatusChanged,
        "Agent task started",
        session_metadata("session-a"),
    );
    append_test_event(
        &mut store,
        EventKind::ModelRequestStarted,
        "A unscoped",
        Metadata::new(),
    );
    append_test_event(
        &mut store,
        EventKind::TaskStatusChanged,
        "Agent task started",
        session_metadata("session-b"),
    );
    append_test_event(
        &mut store,
        EventKind::ModelRequestStarted,
        "B unscoped",
        Metadata::new(),
    );
    append_test_event(
        &mut store,
        EventKind::MessageAdded,
        "A explicit",
        session_metadata("session-a"),
    );

    let events = load_forkable_session_events(&store, "session-a").expect("events should load");
    let summaries = events
        .iter()
        .map(|event| event.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        summaries,
        ["Agent task started", "A unscoped", "A explicit"]
    );
}

#[test]
fn fork_event_transaction_rolls_back_every_append_on_failure() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for summary in ["first", "second"] {
        append_test_event(
            &mut store,
            EventKind::MessageAdded,
            summary,
            session_metadata("source"),
        );
    }
    let events = load_forkable_session_events(&store, "source").expect("source should load");
    let fork_metadata = session_metadata("target");
    let error =
        persist_fork_events_with(&mut store, &events, &fork_metadata, "source", |index, _| {
            if index == 0 {
                Err(StorageError::new("injected fork failure"))
            } else {
                Ok(())
            }
        })
        .expect_err("fault should abort the transaction");
    assert!(error.to_string().contains("injected fork failure"));
    assert!(store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "target")
        .expect("target events should load")
        .is_empty());
    assert_eq!(
        store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "source")
            .expect("source events should load")
            .len(),
        2
    );
}

#[test]
fn active_waiting_and_paused_forks_close_queue_and_detach_recovery_without_mutating_source() {
    for (final_status, expected_source_status) in [
        (None, AgentRunStatus::Running),
        (
            Some("Agent task waiting for permission"),
            AgentRunStatus::WaitingForPermission,
        ),
        (Some("Agent task paused"), AgentRunStatus::Paused),
    ] {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_active_fork_source(&mut store, final_status);
        store
            .save_read_model(
                AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
                "source",
                1,
                "source runtime",
            )
            .expect("source runtime snapshot should persist");
        store
            .save_read_model(
                AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
                "source",
                1,
                "source resources",
            )
            .expect("source resource snapshot should persist");
        let source_before = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "source")
            .expect("source events should load");
        let forkable =
            load_forkable_session_events(&store, "source").expect("fork source events should load");

        persist_fork_events(&mut store, &forkable, &session_metadata("target"), "source")
            .expect("fork events should persist");

        let source_after = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "source")
            .expect("source events should reload");
        let target = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "target")
            .expect("target events should load");
        assert_eq!(source_after, source_before, "fork must not mutate source");
        assert_eq!(
            AgentRunStatus::from_events(
                &active_agent_events_for_session(&source_after, Some("source")),
                false,
                false,
            ),
            expected_source_status
        );
        assert_eq!(
            AgentRunStatus::from_events(
                &active_agent_events_for_session(&target, Some("target")),
                false,
                false,
            ),
            AgentRunStatus::Cancelled
        );

        let historical_target = &target[..source_before.len()];
        assert_eq!(
            historical_target
                .iter()
                .map(|event| (&event.kind, event.summary.as_str()))
                .collect::<Vec<_>>(),
            source_before
                .iter()
                .map(|event| (&event.kind, event.summary.as_str()))
                .collect::<Vec<_>>(),
            "fork history order should remain unchanged"
        );
        assert_eq!(
            historical_target
                .iter()
                .find(|event| event.kind == EventKind::ToolCallStarted)
                .and_then(|event| event.metadata.get("tool_call_id"))
                .map(String::as_str),
            Some("tool-source")
        );
        assert_eq!(
            historical_target
                .iter()
                .find(|event| event.kind == EventKind::PermissionRequested)
                .and_then(|event| event.metadata.get("permission_id"))
                .map(String::as_str),
            Some("permission-source")
        );
        for event in &target {
            for key in FORK_RECOVERY_METADATA_KEYS {
                assert!(
                    !event.metadata.contains_key(key),
                    "fork event retained recovery key {key}"
                );
            }
        }

        assert_eq!(
            target[target.len() - 2]
                .metadata
                .get("queue_action")
                .map(String::as_str),
            Some("delete")
        );
        assert_eq!(
            target[target.len() - 2]
                .metadata
                .get("queue_id")
                .map(String::as_str),
            Some("queue-source")
        );
        let boundary = target.last().expect("fork boundary should exist");
        assert_eq!(boundary.summary, "Fork snapshot detached");
        assert_eq!(
            boundary
                .metadata
                .get("fork_snapshot_reason")
                .map(String::as_str),
            Some("detached_source_run")
        );
        match decode_event_type(boundary) {
            DecodedEventType::V1(typed) => {
                assert_eq!(typed.event_type(), EventTypeV1::AgentRunCancelled)
            }
            decoded => panic!("fork boundary must be typed, got {decoded:?}"),
        }

        assert_eq!(
            pending_queued_agent_messages(&source_after, "source").len(),
            1
        );
        assert!(pending_queued_agent_messages(&target, "target").is_empty());
        let target_state = agent_state_for_session(&store, None, Some("target"))
            .expect("target state should project");
        assert_eq!(target_state.status, "cancelled");
        assert!(target_state.pending_approvals.is_empty());
        assert!(target_state.queued_messages.is_empty());
        assert_eq!(
            store
                .list_permission_audits_for_session(
                    &phase16_task_id(),
                    "target",
                    Some("run-source"),
                    0,
                )
                .expect("target audits should load")
                .len(),
            0,
            "historical permission IDs must not create target permissions"
        );
        assert_eq!(
            store
                .list_permission_audits_for_session(
                    &phase16_task_id(),
                    "source",
                    Some("run-source"),
                    0,
                )
                .expect("source audits should load")
                .len(),
            1
        );
        assert!(store
            .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "target")
            .expect("target runtime snapshot should load")
            .is_none());
        assert!(store
            .load_read_model(AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE, "target")
            .expect("target resource snapshot should load")
            .is_none());
    }
}

#[test]
fn fork_queue_closure_and_detachment_boundary_share_the_history_transaction() {
    for fault_index_from_history_end in [0usize, 1usize] {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_active_fork_source(&mut store, None);
        let source_before = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "source")
            .expect("source events should load");
        let forkable =
            load_forkable_session_events(&store, "source").expect("fork source events should load");
        let fault_index = forkable.len() + fault_index_from_history_end;

        let error = persist_fork_events_with(
            &mut store,
            &forkable,
            &session_metadata("target"),
            "source",
            |append_index, _| {
                if append_index == fault_index {
                    Err(StorageError::new("injected fork suffix failure"))
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("queue closure or boundary failure should abort the transaction");

        assert!(error.to_string().contains("injected fork suffix failure"));
        assert!(store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "target")
            .expect("target events should load")
            .is_empty());
        assert_eq!(
            store
                .list_by_task_and_metadata(&phase16_task_id(), "session_id", "source")
                .expect("source events should reload"),
            source_before
        );
    }
}

#[test]
fn fork_rewrites_all_managed_attachment_references_and_survives_source_delete() {
    let temp = tempdir().expect("tempdir should exist");
    let source_dir = temp.path().join("source");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&source_dir).expect("source dir should exist");
    let source = source_dir.join("image.png");
    fs::write(&source, b"image-bytes").expect("source should write");
    let source_text = source.display().to_string();
    let target_text = target_dir.join("image.png").display().to_string();
    let queue_payload = QueuedAgentMessagePayload {
        prompt: format!("inspect {source_text}"),
        attachments: vec![AgentAttachmentView {
            id: "attachment-a".to_string(),
            name: "image.png".to_string(),
            path: source_text.clone(),
            mime_type: "image/png".to_string(),
            size_bytes: 11,
        }],
        effort: "auto".to_string(),
        current_time: String::new(),
    };
    let mut events = vec![Event {
        id: agent_core::EventId("event-a".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("attachment_paths".to_string(), source_text.clone()),
            ("image_paths".to_string(), source_text.clone()),
            (
                "model_content".to_string(),
                format!("open {source_text}, not {source_text}-extra"),
            ),
            (
                "queue_payload".to_string(),
                serde_json::to_string(&queue_payload).expect("queue payload should encode"),
            ),
        ]
        .into_iter()
        .collect(),
    }];

    clone_fork_attachments_and_rewrite_events(&mut events, &source_dir, &target_dir)
        .expect("attachments should clone");
    assert_eq!(
        events[0].metadata.get("attachment_paths"),
        Some(&target_text)
    );
    assert_eq!(events[0].metadata.get("image_paths"), Some(&target_text));
    let model_content = events[0]
        .metadata
        .get("model_content")
        .expect("model content should remain");
    assert!(model_content.contains(&format!("open {target_text}")));
    assert!(model_content.contains(&format!("{source_text}-extra")));
    let rewritten_queue = serde_json::from_str::<QueuedAgentMessagePayload>(
        events[0]
            .metadata
            .get("queue_payload")
            .expect("queue payload should remain"),
    )
    .expect("queue payload should decode");
    assert_eq!(rewritten_queue.attachments[0].path, target_text);
    assert!(rewritten_queue.prompt.contains(&target_text));

    fs::write(&source, b"source-changed").expect("source should remain writable");
    assert_eq!(
        fs::read(target_dir.join("image.png")).expect("fork attachment should be independent"),
        b"image-bytes"
    );
    fs::remove_dir_all(&source_dir).expect("source should delete");
    assert_eq!(
        fs::read(target_dir.join("image.png")).expect("fork attachment should remain"),
        b"image-bytes"
    );
}

#[test]
fn queue_payload_only_attachment_is_cloned_and_rewritten() {
    let temp = tempdir().expect("tempdir should exist");
    let source_dir = temp.path().join("source");
    let target_dir = temp.path().join("target");
    fs::create_dir_all(&source_dir).expect("source dir should exist");
    let source = source_dir.join("queued.txt");
    fs::write(&source, b"queued").expect("source should write");
    let payload = QueuedAgentMessagePayload {
        prompt: "queued".to_string(),
        attachments: vec![AgentAttachmentView {
            id: "queued-a".to_string(),
            name: "queued.txt".to_string(),
            path: source.display().to_string(),
            mime_type: "text/plain".to_string(),
            size_bytes: 6,
        }],
        effort: "auto".to_string(),
        current_time: String::new(),
    };
    let mut events = vec![Event {
        id: agent_core::EventId("queue-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent message queued".to_string(),
        metadata: [(
            "queue_payload".to_string(),
            serde_json::to_string(&payload).expect("payload should encode"),
        )]
        .into_iter()
        .collect(),
    }];

    clone_fork_attachments_and_rewrite_events(&mut events, &source_dir, &target_dir)
        .expect("queued attachment should clone");
    let rewritten = serde_json::from_str::<QueuedAgentMessagePayload>(
        events[0]
            .metadata
            .get("queue_payload")
            .expect("payload should remain"),
    )
    .expect("payload should decode");
    assert_eq!(
        rewritten.attachments[0].path,
        target_dir.join("queued.txt").display().to_string()
    );
    assert_eq!(
        fs::read(target_dir.join("queued.txt")).expect("target should read"),
        b"queued"
    );
}

#[test]
fn fork_rejects_nested_managed_attachment_paths() {
    let temp = tempdir().expect("tempdir should exist");
    let source_dir = temp.path().join("source");
    let nested = source_dir.join("nested").join("file.txt");
    fs::create_dir_all(nested.parent().expect("nested parent should exist"))
        .expect("nested directory should exist");
    fs::write(&nested, b"nested").expect("nested file should write");
    let mut events = vec![Event {
        id: agent_core::EventId("nested-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::MessageAdded,
        summary: "nested attachment".to_string(),
        metadata: [("attachment_paths".to_string(), nested.display().to_string())]
            .into_iter()
            .collect(),
    }];

    let error = clone_fork_attachments_and_rewrite_events(
        &mut events,
        &source_dir,
        &temp.path().join("target"),
    )
    .expect_err("nested attachments must not be traversed");
    assert!(error.contains("not a direct managed file"));
}

#[cfg(unix)]
#[test]
fn fork_rejects_symlinked_managed_attachment_files() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().expect("tempdir should exist");
    let source_dir = temp.path().join("source");
    fs::create_dir_all(&source_dir).expect("source directory should exist");
    let outside = temp.path().join("outside.txt");
    fs::write(&outside, b"outside").expect("outside file should write");
    let linked = source_dir.join("linked.txt");
    symlink(&outside, &linked).expect("symlink should exist");
    let mut events = vec![Event {
        id: agent_core::EventId("symlink-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::MessageAdded,
        summary: "symlink attachment".to_string(),
        metadata: [("attachment_paths".to_string(), linked.display().to_string())]
            .into_iter()
            .collect(),
    }];

    let error = clone_fork_attachments_and_rewrite_events(
        &mut events,
        &source_dir,
        &temp.path().join("target"),
    )
    .expect_err("symlink attachments must not be followed");
    assert!(error.contains("not a regular file"));
}

#[test]
fn unpublished_fork_journal_recovery_removes_every_target_artifact_idempotently() {
    let data = tempdir().expect("data tempdir should exist");
    let workspace = tempdir().expect("workspace tempdir should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_test_event(
        &mut store,
        EventKind::MessageAdded,
        "fork target",
        session_metadata("target"),
    );
    for namespace in [
        AGENT_SESSION_READ_MODEL_NAMESPACE,
        AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
        AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
    ] {
        store
            .save_read_model(namespace, "target", 1, "snapshot")
            .expect("snapshot should persist");
    }
    let attachment = managed_attachment_dir(workspace.path(), "target").join("file.txt");
    fs::create_dir_all(attachment.parent().expect("attachment parent"))
        .expect("attachment dir should exist");
    fs::write(&attachment, b"target").expect("attachment should write");
    for path in [
        context_checkpoint_path_for_session(workspace.path(), Some("target")),
        context_checkpoint_manifest_path_for_session(workspace.path(), Some("target")),
    ] {
        fs::create_dir_all(path.parent().expect("context parent"))
            .expect("context dir should exist");
        fs::write(path, b"context").expect("context should write");
    }
    let config = session_config(workspace.path(), &["source"]);
    let journal = ProjectLifecycleJournal::fork(
        config.projects[0].id.clone(),
        workspace.path().to_path_buf(),
        "source".to_string(),
        "target".to_string(),
    );
    persist_project_lifecycle_journal_at(data.path(), &journal).expect("journal should persist");

    let refreshes = recover_project_lifecycle_operations_at(data.path(), &mut store, &config)
        .expect("recovery should succeed");
    assert!(refreshes.is_empty());
    assert!(store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "target")
        .expect("events should load")
        .is_empty());
    for namespace in [
        AGENT_SESSION_READ_MODEL_NAMESPACE,
        AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
        AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
    ] {
        assert!(store
            .load_read_model(namespace, "target")
            .expect("snapshot should load")
            .is_none());
    }
    assert!(!attachment.exists());
    assert!(load_project_lifecycle_journals_at(data.path())
        .expect("journal should load")
        .is_empty());
    assert!(
        recover_project_lifecycle_operations_at(data.path(), &mut store, &config)
            .expect("second recovery should succeed")
            .is_empty()
    );
}

#[test]
fn published_fork_recovery_keeps_history_and_only_clears_journal() {
    let data = tempdir().expect("data tempdir should exist");
    let workspace = tempdir().expect("workspace tempdir should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_test_event(
        &mut store,
        EventKind::MessageAdded,
        "fork target",
        session_metadata("target"),
    );
    let config = session_config(workspace.path(), &["source", "target"]);
    let journal = ProjectLifecycleJournal::fork(
        config.projects[0].id.clone(),
        workspace.path().to_path_buf(),
        "source".to_string(),
        "target".to_string(),
    );
    persist_project_lifecycle_journal_at(data.path(), &journal).expect("journal should persist");

    recover_project_lifecycle_operations_at(data.path(), &mut store, &config)
        .expect("recovery should succeed");
    assert_eq!(
        store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "target")
            .expect("target should load")
            .len(),
        1
    );
    assert!(load_project_lifecycle_journals_at(data.path())
        .expect("journal should load")
        .is_empty());
}

#[test]
fn published_session_delete_recovery_removes_context_manifest_and_resource_snapshot() {
    let data = tempdir().expect("data tempdir should exist");
    let workspace = tempdir().expect("workspace tempdir should exist");
    let historical_output = workspace
        .path()
        .join(".cindx/output-history/deleted/version/result.md");
    fs::create_dir_all(
        historical_output
            .parent()
            .expect("output parent should exist"),
    )
    .expect("output directory should exist");
    fs::write(&historical_output, b"retired output").expect("output should write");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_test_event(
        &mut store,
        EventKind::MessageAdded,
        "deleted session",
        [
            ("session_id".to_string(), "deleted".to_string()),
            ("project_id".to_string(), "project-cindx".to_string()),
            ("role".to_string(), "user".to_string()),
            (
                "result_artifact_path".to_string(),
                historical_output.display().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );
    store
        .save_read_model(
            AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
            "deleted",
            1,
            "resource",
        )
        .expect("resource snapshot should persist");
    let attachment = managed_attachment_dir(workspace.path(), "deleted").join("file.txt");
    fs::create_dir_all(attachment.parent().expect("attachment parent"))
        .expect("attachment dir should exist");
    fs::write(&attachment, b"deleted").expect("attachment should write");
    let context = context_checkpoint_path_for_session(workspace.path(), Some("deleted"));
    let manifest = context_checkpoint_manifest_path_for_session(workspace.path(), Some("deleted"));
    fs::create_dir_all(context.parent().expect("context parent"))
        .expect("context dir should exist");
    fs::write(&context, b"context").expect("context should write");
    fs::write(&manifest, b"manifest").expect("manifest should write");
    let config = session_config(workspace.path(), &["retained"]);
    let journal = ProjectLifecycleJournal::delete(
        config.projects[0].id.clone(),
        workspace.path().to_path_buf(),
        vec!["deleted".to_string()],
        false,
    );
    persist_project_lifecycle_journal_at(data.path(), &journal).expect("journal should persist");

    let refreshes = recover_project_lifecycle_operations_at(data.path(), &mut store, &config)
        .expect("delete recovery should succeed");
    assert_eq!(refreshes.len(), 1);
    assert!(store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "deleted")
        .expect("events should load")
        .is_empty());
    assert!(store
        .load_read_model(AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE, "deleted")
        .expect("resource snapshot should load")
        .is_none());
    assert!(!attachment.exists());
    assert!(!context.exists());
    assert!(!manifest.exists());
    assert!(!historical_output.exists());
    assert_eq!(
        load_project_lifecycle_journals_at(data.path())
            .expect("journal should load")
            .len(),
        1,
        "refresh journal must remain until a runtime can schedule the refresh"
    );
    complete_project_lifecycle_journal_at(data.path(), &refreshes[0].journal)
        .expect("scheduled refresh should complete the journal");
    assert!(load_project_lifecycle_journals_at(data.path())
        .expect("journal should load")
        .is_empty());
}

#[cfg(unix)]
#[test]
fn published_delete_recovery_keeps_journal_and_events_when_artifact_root_is_unsafe() {
    use std::os::unix::fs::symlink;

    let data = tempdir().expect("data tempdir should exist");
    let workspace = tempdir().expect("workspace tempdir should exist");
    let outside = tempdir().expect("outside tempdir should exist");
    fs::write(outside.path().join("keep.md"), b"keep").expect("outside file should write");
    fs::create_dir_all(workspace.path().join(".cindx")).expect("cindx root should exist");
    symlink(
        outside.path(),
        workspace.path().join(".cindx/output-history"),
    )
    .expect("unsafe managed root should link");

    let mut store = SqliteStore::in_memory().expect("store should open");
    append_test_event(
        &mut store,
        EventKind::MessageAdded,
        "deleted session",
        [
            ("session_id".to_string(), "deleted".to_string()),
            ("project_id".to_string(), "project-cindx".to_string()),
            (
                "result_artifact_path".to_string(),
                ".cindx/output-history/keep.md".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let config = session_config(workspace.path(), &["retained"]);
    let journal = ProjectLifecycleJournal::delete(
        config.projects[0].id.clone(),
        workspace.path().to_path_buf(),
        vec!["deleted".to_string()],
        false,
    );
    persist_project_lifecycle_journal_at(data.path(), &journal).expect("journal should persist");

    let error = recover_project_lifecycle_operations_at(data.path(), &mut store, &config)
        .expect_err("unsafe root must fail closed");

    assert!(error.contains("symbolic link"));
    assert_eq!(
        store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "deleted")
            .expect("deleted events should remain")
            .len(),
        1
    );
    assert_eq!(
        load_project_lifecycle_journals_at(data.path())
            .expect("journal should remain")
            .len(),
        1
    );
    assert_eq!(
        fs::read(outside.path().join("keep.md")).expect("outside file should remain"),
        b"keep"
    );
}

#[test]
fn unpublished_delete_journal_is_aborted_without_touching_persistent_state() {
    let data = tempdir().expect("data tempdir should exist");
    let workspace = tempdir().expect("workspace tempdir should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_test_event(
        &mut store,
        EventKind::MessageAdded,
        "retained",
        session_metadata("retained"),
    );
    let config = session_config(workspace.path(), &["retained"]);
    let journal = ProjectLifecycleJournal::delete(
        config.projects[0].id.clone(),
        workspace.path().to_path_buf(),
        vec!["retained".to_string()],
        false,
    );
    persist_project_lifecycle_journal_at(data.path(), &journal).expect("journal should persist");

    recover_project_lifecycle_operations_at(data.path(), &mut store, &config)
        .expect("recovery should abort unpublished delete");
    assert_eq!(
        store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "retained")
            .expect("events should load")
            .len(),
        1
    );
    assert!(load_project_lifecycle_journals_at(data.path())
        .expect("journal should load")
        .is_empty());
}

#[test]
fn malformed_lifecycle_journal_fails_closed_and_remains_for_diagnosis() {
    let data = tempdir().expect("data tempdir should exist");
    let workspace = tempdir().expect("workspace tempdir should exist");
    let directory = lifecycle_journal_directory(data.path());
    fs::create_dir_all(&directory).expect("journal dir should exist");
    let path = directory.join("broken.json");
    fs::write(&path, b"not-json").expect("broken journal should write");
    let mut store = SqliteStore::in_memory().expect("store should open");
    let config = session_config(workspace.path(), &["retained"]);

    let error = recover_project_lifecycle_operations_at(data.path(), &mut store, &config)
        .expect_err("invalid journal must fail closed");
    assert!(error.contains("invalid project lifecycle journal"));
    assert!(path.exists());
}
