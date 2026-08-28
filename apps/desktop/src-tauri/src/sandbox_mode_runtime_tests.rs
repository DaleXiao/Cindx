use super::*;
use crate::test_support::temp_test_root;
use agent_core::{Event, EventId, EventKind, SandboxMode};
use agent_storage::SqliteStore;

fn run_context(session_id: &str) -> Metadata {
    [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect()
}

fn mode_event(session_id: &str, sequence: u64, mode_label: &str) -> Event {
    let mut metadata = sandbox_mode_changed_event_metadata(match mode_label {
        "read-only" => SandboxMode::ReadOnly,
        "workspace-write" => SandboxMode::WorkspaceWrite,
        _ => SandboxMode::FullAccess,
    });
    metadata.insert("session_id".to_string(), session_id.to_string());
    Event {
        id: EventId(format!("event-{sequence}")),
        task_id: crate::runtime_values::phase16_task_id(),
        sequence,
        timestamp_ms: sequence,
        kind: EventKind::TaskStatusChanged,
        summary: SANDBOX_MODE_CHANGED_SUMMARY.to_string(),
        metadata,
    }
}

#[test]
fn effective_mode_defaults_to_full_access_without_mode_events() {
    assert_eq!(
        effective_sandbox_mode_from_events(&[], "session-a"),
        SandboxMode::FullAccess
    );
    let unrelated = Event {
        id: EventId("event-1".to_string()),
        task_id: crate::runtime_values::phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect(),
    };
    assert_eq!(
        effective_sandbox_mode_from_events(&[unrelated], "session-a"),
        SandboxMode::FullAccess
    );
}

#[test]
fn effective_mode_fold_takes_the_latest_valid_change() {
    let events = vec![
        mode_event("session-a", 1, "read-only"),
        mode_event("session-a", 2, "workspace-write"),
    ];
    assert_eq!(
        effective_sandbox_mode_from_events(&events, "session-a"),
        SandboxMode::WorkspaceWrite
    );
    let events = vec![
        mode_event("session-a", 1, "workspace-write"),
        mode_event("session-a", 2, "full"),
    ];
    assert_eq!(
        effective_sandbox_mode_from_events(&events, "session-a"),
        SandboxMode::FullAccess
    );
}

#[test]
fn malformed_mode_events_are_ignored_instead_of_failing_open_or_closed() {
    let mut malformed = mode_event("session-a", 1, "read-only");
    malformed
        .metadata
        .insert(SANDBOX_MODE_METADATA_KEY.to_string(), "bogus".to_string());
    let events = vec![malformed.clone()];
    assert_eq!(
        effective_sandbox_mode_from_events(&events, "session-a"),
        SandboxMode::FullAccess
    );
    let events = vec![malformed, mode_event("session-a", 2, "read-only")];
    assert_eq!(
        effective_sandbox_mode_from_events(&events, "session-a"),
        SandboxMode::ReadOnly
    );
}

#[test]
fn mode_events_from_other_sessions_do_not_leak_across_the_fold() {
    let events = vec![
        mode_event("session-a", 1, "read-only"),
        mode_event("session-b", 2, "workspace-write"),
    ];
    assert_eq!(
        effective_sandbox_mode_from_events(&events, "session-a"),
        SandboxMode::ReadOnly
    );
    assert_eq!(
        effective_sandbox_mode_from_events(&events, "session-b"),
        SandboxMode::WorkspaceWrite
    );
    assert_eq!(
        effective_sandbox_mode_from_events(&events, "session-c"),
        SandboxMode::FullAccess
    );
}

#[test]
fn mode_change_event_metadata_carries_the_replay_contract() {
    let metadata = sandbox_mode_changed_event_metadata(SandboxMode::WorkspaceWrite);
    assert_eq!(
        metadata.get(SANDBOX_MODE_EVENT_KEY).map(String::as_str),
        Some(SANDBOX_MODE_EVENT_MARKER)
    );
    assert_eq!(
        metadata.get("sandbox_mode_schema").map(String::as_str),
        Some(SANDBOX_MODE_SCHEMA)
    );
    assert_eq!(
        metadata.get(SANDBOX_MODE_METADATA_KEY).map(String::as_str),
        Some("workspace-write")
    );
}

#[test]
fn sandbox_mode_replays_from_the_durable_event_log_after_restart() {
    let database = temp_test_root("cindx-sandbox-mode-replay").join("events.db");
    std::fs::create_dir_all(database.parent().expect("database parent")).expect("dir");
    {
        let mut store = SqliteStore::open(&database).expect("store should open");
        append_sandbox_mode_event(&mut store, &run_context("session-a"), SandboxMode::ReadOnly)
            .expect("mode event should append");
        append_sandbox_mode_event(
            &mut store,
            &run_context("session-a"),
            SandboxMode::WorkspaceWrite,
        )
        .expect("mode event should append");
    }
    // Simulated restart: a brand-new store handle replays the durable events.
    let reopened = SqliteStore::open(&database).expect("store should reopen");
    assert_eq!(
        effective_sandbox_mode_for_session(&reopened, "session-a"),
        Ok(SandboxMode::WorkspaceWrite)
    );
    assert_eq!(
        effective_sandbox_mode_for_session(&reopened, "session-never-touched"),
        Ok(SandboxMode::FullAccess)
    );
}

#[test]
fn sandbox_mode_sessions_stay_isolated_in_the_shared_store() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_sandbox_mode_event(&mut store, &run_context("session-a"), SandboxMode::ReadOnly)
        .expect("mode event should append");
    append_sandbox_mode_event(
        &mut store,
        &run_context("session-b"),
        SandboxMode::WorkspaceWrite,
    )
    .expect("mode event should append");

    assert_eq!(
        effective_sandbox_mode_for_session(&store, "session-a"),
        Ok(SandboxMode::ReadOnly)
    );
    assert_eq!(
        effective_sandbox_mode_for_session(&store, "session-b"),
        Ok(SandboxMode::WorkspaceWrite)
    );
    assert_eq!(
        effective_sandbox_mode_for_session(&store, "session-c"),
        Ok(SandboxMode::FullAccess)
    );
}
