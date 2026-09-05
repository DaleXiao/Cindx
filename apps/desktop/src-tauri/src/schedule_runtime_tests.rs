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

fn dispatch_test_schedule(run: ScheduleRunRecord) -> ScheduleRecord {
    ScheduleRecord {
        id: "schedule-1".to_string(),
        name: "Nightly report".to_string(),
        project_id: None,
        session_id: None,
        execution_session_id: "session-a".to_string(),
        prompt: "run the report".to_string(),
        effort: "default".to_string(),
        timezone: "UTC".to_string(),
        cadence: ScheduleCadence::Daily,
        anchor_at_ms: 0,
        weekly_days: Vec::new(),
        ends_at_ms: None,
        catch_up: true,
        enabled: true,
        next_run_at_ms: None,
        created_at_ms: 0,
        updated_at_ms: 0,
        runs: vec![run],
    }
}

fn queued_test_run(id: &str, queue_id: Option<&str>) -> ScheduleRunRecord {
    ScheduleRunRecord {
        id: id.to_string(),
        queue_id: queue_id.map(str::to_string),
        source: "scheduled".to_string(),
        scheduled_for_ms: 0,
        queued_at_ms: queue_id.map(|_| 1_000),
        dispatch_attempts: 0,
        last_dispatch_at_ms: None,
        started_at_ms: None,
        finished_at_ms: None,
        status: "queued".to_string(),
        error: None,
    }
}

#[test]
fn scheduled_dispatch_matches_the_recorded_queue_head() {
    let schedules = vec![dispatch_test_schedule(queued_test_run("run-1", Some("q1")))];

    match decide_scheduled_dispatch(&schedules, "session-a", "q1", 5_000) {
        ScheduledDispatchDecision::Dispatch {
            schedule_id,
            queue_id,
        } => {
            assert_eq!(schedule_id, "schedule-1");
            assert_eq!(queue_id, "q1");
        }
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn scheduled_dispatch_adopts_an_untouched_orphan_queue_head() {
    // Crash window: q1 was committed to SQLite by a trigger that died before
    // saving its run record; the next trigger recorded q2 while the real head
    // is still q1. The never-dispatched run adopts the head instead of
    // stalling forever on a queue item the config does not know.
    let schedules = vec![dispatch_test_schedule(queued_test_run("run-2", Some("q2")))];

    match decide_scheduled_dispatch(&schedules, "session-a", "q1", 5_000) {
        ScheduledDispatchDecision::AdoptOrphanHead {
            schedule_id,
            run_id,
            queue_id,
        } => {
            assert_eq!(schedule_id, "schedule-1");
            assert_eq!(run_id, "run-2");
            assert_eq!(queue_id, "q1");
        }
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn scheduled_dispatch_counts_bounded_attempts_for_a_touched_mismatched_run() {
    let mut run = queued_test_run("run-2", Some("q2"));
    run.dispatch_attempts = 1;
    let schedules = vec![dispatch_test_schedule(run)];

    match decide_scheduled_dispatch(&schedules, "session-a", "q1", 5_000) {
        ScheduledDispatchDecision::CountMismatchAttempt {
            schedule_id,
            run_id,
        } => {
            assert_eq!(schedule_id, "schedule-1");
            assert_eq!(run_id, "run-2");
        }
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn scheduled_dispatch_retires_capped_runs_and_honors_the_backoff_window() {
    // A mismatched run at the attempt cap retires through the existing bound
    // instead of rewriting the config on every poll forever.
    let mut capped = queued_test_run("run-2", Some("q2"));
    capped.dispatch_attempts = SCHEDULE_MAX_DISPATCH_ATTEMPTS;
    let schedules = vec![dispatch_test_schedule(capped)];
    assert!(matches!(
        decide_scheduled_dispatch(&schedules, "session-a", "q1", 5_000),
        ScheduledDispatchDecision::Skip
    ));

    // Inside the retry window even a matching head waits.
    let mut recent = queued_test_run("run-1", Some("q1"));
    recent.last_dispatch_at_ms = Some(4_500);
    let schedules = vec![dispatch_test_schedule(recent)];
    assert!(matches!(
        decide_scheduled_dispatch(&schedules, "session-a", "q1", 5_000),
        ScheduledDispatchDecision::Skip
    ));
}

#[test]
fn scheduled_dispatch_skips_sessions_without_a_queued_active_run() {
    let mut finished = queued_test_run("run-1", Some("q1"));
    finished.status = "completed".to_string();
    finished.finished_at_ms = Some(2_000);
    let schedules = vec![dispatch_test_schedule(finished)];

    assert!(matches!(
        decide_scheduled_dispatch(&schedules, "session-a", "q1", 5_000),
        ScheduledDispatchDecision::Skip
    ));
    assert!(matches!(
        decide_scheduled_dispatch(&[], "session-a", "q1", 5_000),
        ScheduledDispatchDecision::Skip
    ));
}
