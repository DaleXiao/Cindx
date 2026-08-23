use super::*;

#[test]
fn queued_agent_message_ids_accept_only_bounded_client_ids() {
    assert_eq!(
        queued_agent_message_id(Some(" agent-queue-client-abc-123 ")),
        "agent-queue-client-abc-123"
    );
    assert!(!queued_agent_message_id(Some("queue-client-abc")).starts_with("queue-client-"));
    assert!(!queued_agent_message_id(Some("agent-queue-client-bad/id")).contains("bad/id"));
}

#[test]
fn queued_agent_messages_are_durable_ordered_and_session_scoped() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context_a = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let context_b = [("session_id".to_string(), "session-b".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = |prompt: &str| QueuedAgentMessagePayload {
        prompt: prompt.to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
        plan_mode: false,
    };
    let first = payload("first");
    let second = payload("second");
    let other = payload("other session");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "enqueue",
        "queue-a-1",
        "queue",
        10,
        Some(&first),
    )
    .expect("first message should queue");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "enqueue",
        "queue-a-2",
        "queue",
        20,
        Some(&second),
    )
    .expect("second message should queue");
    append_agent_queue_event(
        &mut store,
        &context_b,
        "enqueue",
        "queue-b-1",
        "queue",
        5,
        Some(&other),
    )
    .expect("other session message should queue");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "steer",
        "queue-a-2",
        "steer",
        20,
        None,
    )
    .expect("second message should steer");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    let pending = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].view.id, "queue-a-2");
    assert_eq!(pending[0].view.mode, "steer");
    assert_eq!(pending[1].view.id, "queue-a-1");

    let state =
        agent_state_for_session(&store, None, Some("session-a")).expect("queued state should load");
    assert_eq!(state.session_id.as_deref(), Some("session-a"));
    assert_eq!(state.status, "idle");
    assert_eq!(state.queued_messages.len(), 2);
    assert!(state.timeline.is_empty());
    let edited = payload("first edited");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "edit",
        "queue-a-1",
        "queue",
        10,
        Some(&edited),
    )
    .expect("first message should edit");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "delete",
        "queue-a-2",
        "steer",
        20,
        None,
    )
    .expect("steered message should delete");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("edited queue events should load");
    let remaining = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].view.prompt, "first edited");
    assert_eq!(
        agent_state_for_session(&store, None, Some("session-b"))
            .expect("other queued state should load")
            .queued_messages
            .len(),
        1
    );
}

#[test]
fn queue_events_do_not_change_a_terminal_agent_status() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "first task".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context.clone(),
    )
    .expect("run should complete");
    let queued = QueuedAgentMessagePayload {
        prompt: "next task".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
        plan_mode: false,
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-next",
        "queue",
        20,
        Some(&queued),
    )
    .expect("next task should queue");

    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("terminal queue state should load");
    assert_eq!(state.status, "completed");
    assert_eq!(state.queued_messages.len(), 1);
    assert_eq!(state.timeline.len(), 2);
}

#[test]
fn queued_messages_preserve_a_permission_waiting_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "inspect files".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task waiting for permission",
        context.clone(),
    )
    .expect("run should wait");
    let queued = QueuedAgentMessagePayload {
        prompt: "follow-up".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
        plan_mode: false,
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-next",
        "queue",
        20,
        Some(&queued),
    )
    .expect("follow-up should queue");

    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("waiting queue state should load");
    assert_eq!(state.status, "waiting_for_permission");
    assert!(state.can_cancel);
    assert_eq!(state.queued_messages.len(), 1);
}

#[test]
fn queued_agent_message_start_and_restore_are_replay_safe() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = QueuedAgentMessagePayload {
        prompt: "continue safely".to_string(),
        attachments: Vec::new(),
        effort: "pro".to_string(),
        current_time: "now".to_string(),
        plan_mode: false,
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    append_agent_queue_event(&mut store, &context, "start", "queue-a", "queue", 10, None)
        .expect("message should start");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    assert!(pending_queued_agent_messages(&events, "session-a").is_empty());

    append_agent_queue_event(
        &mut store,
        &context,
        "restore",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should restore");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("restored queue events should load");
    let restored = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].view.prompt, "continue safely");
    assert_eq!(restored[0].view.effort, "high");
}

#[test]
fn queued_run_failure_restores_only_before_agent_start() {
    let session_id = "session-queue-failure";
    let context = [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = QueuedAgentMessagePayload {
        prompt: "continue safely".to_string(),
        attachments: Vec::new(),
        effort: "pro".to_string(),
        current_time: "now".to_string(),
        plan_mode: false,
    };
    let pending = PendingQueuedAgentMessage {
        view: QueuedAgentMessageView {
            id: "queue-a".to_string(),
            session_id: session_id.to_string(),
            prompt: payload.prompt.clone(),
            attachments: Vec::new(),
            effort: payload.effort.clone(),
            mode: "queue".to_string(),
            plan_mode: false,
            created_at_ms: 10,
            updated_at_ms: 10,
        },
        payload: payload.clone(),
        priority_sequence: 1,
    };

    let mut pre_start_store = SqliteStore::in_memory().expect("store should open");
    append_agent_queue_event(
        &mut pre_start_store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    append_agent_queue_event(
        &mut pre_start_store,
        &context,
        "start",
        "queue-a",
        "queue",
        10,
        None,
    )
    .expect("dispatch should reserve the message");
    assert!(restore_queued_agent_message_before_run_start(
        &mut pre_start_store,
        &context,
        session_id,
        &pending,
    )
    .expect("a pre-start failure should restore"));
    let pre_start_events = pre_start_store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    assert_eq!(
        pending_queued_agent_messages(&pre_start_events, session_id).len(),
        1
    );

    let mut post_start_store = SqliteStore::in_memory().expect("store should open");
    append_agent_queue_event(
        &mut post_start_store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    append_agent_queue_event(
        &mut post_start_store,
        &context,
        "start",
        "queue-a",
        "queue",
        10,
        None,
    )
    .expect("dispatch should reserve the message");
    let run_context = [
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("queue_id".to_string(), "queue-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut post_start_store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("agent start should persist");
    append_message_event_with_metadata(
        &mut post_start_store,
        &phase16_task_id(),
        MessageRole::User,
        &payload.prompt,
        run_context,
    )
    .expect("user message should persist");

    assert!(!restore_queued_agent_message_before_run_start(
        &mut post_start_store,
        &context,
        session_id,
        &pending,
    )
    .expect("a post-start failure should not restore"));
    let post_start_events = post_start_store
        .list_by_task(&phase16_task_id())
        .expect("run events should load");
    assert!(pending_queued_agent_messages(&post_start_events, session_id).is_empty());
    assert!(!post_start_events.iter().any(|event| {
        event.metadata.get("queue_action").map(String::as_str) == Some("restore")
    }));
}

#[test]
fn queued_steer_commit_revalidates_the_current_queue_item() {
    let session_id = "session-steer-commit";
    let context = [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = QueuedAgentMessagePayload {
        prompt: "apply this guidance".to_string(),
        attachments: Vec::new(),
        effort: "pro".to_string(),
        current_time: "now".to_string(),
        plan_mode: false,
    };
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-live",
        "queue",
        10,
        Some(&payload),
    )
    .expect("live message should queue");

    let committed = commit_queued_agent_steer(&mut store, &context, session_id, "queue-live")
        .expect("a current queue item should commit");
    assert_eq!(committed.mode, "steer");

    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-deleted",
        "queue",
        20,
        Some(&payload),
    )
    .expect("second message should queue");
    append_agent_queue_event(
        &mut store,
        &context,
        "delete",
        "queue-deleted",
        "queue",
        20,
        None,
    )
    .expect("second message should delete");

    assert_eq!(
        commit_queued_agent_steer(&mut store, &context, session_id, "queue-deleted")
            .expect_err("a stale queue item must not commit"),
        "queued message not found"
    );
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.metadata.get("queue_action").map(String::as_str) == Some("steer")
            })
            .count(),
        1
    );
}

#[test]
fn queued_agent_messages_update_the_incremental_session_read_model() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-queue-read-model";
    let context = [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect::<Metadata>();
    let mut payload = QueuedAgentMessagePayload {
        prompt: "first version".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
        plan_mode: false,
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    let initial = load_agent_session_read_model(&mut store, session_id)
        .expect("initial queue read model should build");
    assert_eq!(initial.state.queued_messages.len(), 1);
    assert_eq!(
        initial
            .queued_payloads
            .get("queue-a")
            .map(|payload| payload.prompt.as_str()),
        Some("first version")
    );

    payload.prompt = "edited version".to_string();
    append_agent_queue_event(
        &mut store,
        &context,
        "edit",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should edit");
    let edited = load_agent_session_read_model(&mut store, session_id)
        .expect("queue edit should apply incrementally");
    assert_eq!(edited.state.queued_messages[0].prompt, "edited version");
    let next = next_queued_agent_message_from_read_model(&mut store, session_id)
        .expect("next queue item should use the read model")
        .expect("next queue item should exist");
    assert_eq!(next.payload.prompt, "edited version");
    let (queued, can_cancel) =
        queued_agent_message_from_read_model(&mut store, session_id, "queue-a")
            .expect("queue action lookup should use the read model");
    let receipt = queued_agent_message_action_receipt(
        &store, session_id, "queue-a", queued, can_cancel, false,
    )
    .expect("queue action receipt should use the compact revision");
    assert_eq!(
        receipt
            .message
            .as_ref()
            .map(|message| message.prompt.as_str()),
        Some("edited version")
    );
    assert!(!receipt.cancelled_active_run);
    assert!(!receipt.steer_committed);
    assert_eq!(
        serde_json::to_value(&receipt)
            .expect("receipt should serialize")
            .get("steerCommitted")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );

    append_agent_queue_event(&mut store, &context, "start", "queue-a", "queue", 10, None)
        .expect("message should start");
    let started = load_agent_session_read_model(&mut store, session_id)
        .expect("queue start should apply incrementally");
    assert!(started.state.queued_messages.is_empty());
    assert!(started.queued_payloads.is_empty());

    let run_context = [
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("queue_id".to_string(), "queue-a".to_string()),
    ]
    .into_iter()
    .collect();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context,
    )
    .expect("queued run should start");
    let running = load_agent_session_read_model(&mut store, session_id)
        .expect("run identity should apply incrementally");
    assert_eq!(running.latest_run_queue_id.as_deref(), Some("queue-a"));
}
