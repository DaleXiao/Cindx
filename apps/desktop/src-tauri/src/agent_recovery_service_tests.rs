use super::*;
use agent_core::{insert_event_type_v1, EventTypeV1, EVENT_TYPE_METADATA_KEY};

fn run_metadata(session_id: &str, run_id: &str, with_prompt: bool) -> Metadata {
    let mut metadata = [
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if with_prompt {
        metadata.insert("prompt".to_string(), "finish the task".to_string());
    }
    metadata
}

#[test]
fn recorded_error_keeps_interrupted_run_recoverable() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = run_metadata("session-recorded", "run-recorded", true);
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    let mut error_metadata = run_metadata("session-recorded", "run-recorded", false);
    error_metadata.insert("error".to_string(), "recoverable warning".to_string());
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::Error,
        "Background extraction unavailable",
        error_metadata,
    )
    .expect("recorded error should append");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-recorded")
        .expect("events should load");
    assert_eq!(
        events.last().map(|event| event.summary.as_str()),
        Some("Agent task paused")
    );
    let mut paused = events.last().cloned().expect("pause event should exist");
    assert_eq!(
        paused
            .metadata
            .get(EVENT_TYPE_METADATA_KEY)
            .map(String::as_str),
        Some(EventTypeV1::AgentRunPaused.id())
    );
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Preparing recovery",
        recovery_run_context(&events[0]),
    )
    .expect("recovery progress should append");
    let progress = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-recorded")
        .expect("progress should load")
        .pop()
        .expect("progress should exist");
    assert!(!progress.metadata.contains_key(EVENT_TYPE_METADATA_KEY));
    paused.summary = "运行已暂停".to_string();
    store
        .update_event_content(&paused)
        .expect("localized pause should persist");
    let localized_events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-recorded")
        .expect("localized events should load");
    let localized_active =
        active_agent_events_for_session(&localized_events, Some("session-recorded"));
    assert_eq!(
        latest_agent_run_event(&localized_active),
        Ok(Some(AgentRunEvent::Paused))
    );
    assert!(
        peek_agent_recovery_envelope(&store, &run_context, &["paused"])
            .expect("localized pause should remain resumable")
            .is_some()
    );

    let mut later_error = run_metadata("session-recorded", "run-recorded", false);
    insert_event_type_v1(
        &EventKind::Error,
        &mut later_error,
        EventTypeV1::ErrorRecorded,
    )
    .expect("recorded error tag should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::Error,
        "Background cleanup unavailable",
        later_error,
    )
    .expect("later recorded error should append");
    assert!(
        peek_agent_recovery_envelope(&store, &run_context, &["paused"])
            .expect("recorded error should not hide the pause")
            .is_some()
    );

    let mut invalid = run_metadata("session-recorded", "run-recorded", false);
    invalid.insert(
        EVENT_TYPE_METADATA_KEY.to_string(),
        "cindx.event.v2/agent.run.paused".to_string(),
    );
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        invalid,
    )
    .expect("future lifecycle event should append");
    assert!(
        peek_agent_recovery_envelope(&store, &run_context, &["paused"])
            .expect("invalid lifecycle event should fail closed")
            .is_none()
    );
}

#[test]
fn localized_typed_completion_is_not_recovered() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_metadata("session-complete", "run-complete", true),
    )
    .expect("start should append");
    let mut completed = run_metadata("session-complete", "run-complete", false);
    insert_event_type_v1(
        &EventKind::TaskStatusChanged,
        &mut completed,
        EventTypeV1::AgentRunCompleted,
    )
    .expect("completion tag should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "任务已完成",
        completed,
    )
    .expect("completion should append");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        0
    );
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-complete")
        .expect("events should load");
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].summary, "任务已完成");
}

#[test]
fn future_lifecycle_tag_fails_closed_during_recovery() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_metadata("session-future", "run-future", true),
    )
    .expect("start should append");
    let mut future = run_metadata("session-future", "run-future", false);
    future.insert(
        EVENT_TYPE_METADATA_KEY.to_string(),
        "cindx.event.v2/agent.run.completed".to_string(),
    );
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        future,
    )
    .expect("future lifecycle event should append");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should fail closed"),
        0
    );
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-future")
        .expect("events should load");
    assert_eq!(events.len(), 2);
}

#[test]
fn unscoped_cancel_check_uses_typed_lifecycle() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let mut started = run_metadata("unused", "run-unscoped", true);
    started.remove("session_id");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        started,
    )
    .expect("start should append");
    let mut cancelled = run_metadata("unused", "run-unscoped", false);
    cancelled.remove("session_id");
    insert_event_type_v1(
        &EventKind::TaskStatusChanged,
        &mut cancelled,
        EventTypeV1::AgentRunCancelled,
    )
    .expect("cancel tag should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "任务已取消",
        cancelled,
    )
    .expect("cancel should append");

    let mut later_error = run_metadata("unused", "run-unscoped", false);
    later_error.remove("session_id");
    insert_event_type_v1(
        &EventKind::Error,
        &mut later_error,
        EventTypeV1::ErrorRecorded,
    )
    .expect("recorded error tag should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::Error,
        "Background cleanup unavailable",
        later_error,
    )
    .expect("recorded error should append");

    assert!(agent_task_is_cancelled(&mut store, None).expect("cancel status should load"));
}
