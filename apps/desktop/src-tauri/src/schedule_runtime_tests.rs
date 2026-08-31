use super::test_support::temp_test_root;
use super::*;

#[test]
fn scheduled_queue_progress_tracks_permission_and_completion() {
    let queue_id = "schedule-queue";
    let event = |sequence: u64, summary: &str, queue_action: Option<&str>| Event {
        id: EventId(format!("schedule-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 100,
        kind: EventKind::TaskStatusChanged,
        summary: summary.to_string(),
        metadata: [
            ("queue_id".to_string(), queue_id.to_string()),
            ("session_id".to_string(), "session-a".to_string()),
        ]
        .into_iter()
        .chain(queue_action.map(|action| ("queue_action".to_string(), action.to_string())))
        .collect(),
    };
    let mut events = vec![
        event(1, "Agent message queued", Some("enqueue")),
        event(2, "Queued agent message started", Some("start")),
        event(3, "Agent task started", None),
        event(4, "Agent task waiting for permission", None),
    ];

    let waiting = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(waiting.status, "waiting_for_permission");
    assert_eq!(waiting.started_at_ms, Some(200));
    assert_eq!(
        latest_unfinished_agent_queue_id(&events).as_deref(),
        Some(queue_id)
    );

    events.push(event(5, "Agent task paused", None));
    let paused = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(paused.status, "paused");
    assert_eq!(paused.finished_at_ms, Some(500));
    assert!(latest_unfinished_agent_queue_id(&events).is_none());

    events.push(event(6, "Agent task retry started", None));
    events.push(event(7, "Agent task completed", None));
    let completed = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.finished_at_ms, Some(700));
    assert!(latest_unfinished_agent_queue_id(&events).is_none());
}

#[test]
fn restored_scheduled_queue_is_retryable_instead_of_running() {
    let queue_id = "schedule-queue";
    let events = vec![
        Event {
            id: EventId("schedule-enqueue".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent message queued".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "enqueue".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("schedule-start".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Queued agent message started".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "start".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("schedule-restore".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 300,
            kind: EventKind::TaskStatusChanged,
            summary: "Queued agent message restored".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "restore".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ];

    let progress = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(progress.status, "queued");
    assert_eq!(progress.started_at_ms, None);
    assert_eq!(progress.finished_at_ms, None);
}

#[test]
fn schedule_execution_sessions_stay_out_of_the_task_sidebar() {
    let root = temp_test_root("hidden-schedule-session");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.sessions.push(SessionRecord {
        id: "schedule-session-review".to_string(),
        project_id: "project-cindx".to_string(),
        name: "Review · Schedule".to_string(),
        detail: SCHEDULE_EXECUTION_SESSION_DETAIL.to_string(),
        effort: "auto".to_string(),

        agent_model: String::new(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    let state = project_session_state(&config, None);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].id, initial_session_id);

    config.sessions.retain(is_schedule_execution_session);
    let replacement = ensure_open_session_for_project(&mut config, "project-cindx");
    assert_ne!(replacement, "schedule-session-review");
    assert!(config
        .sessions
        .iter()
        .any(|session| { session.id == replacement && !is_schedule_execution_session(session) }));
}
