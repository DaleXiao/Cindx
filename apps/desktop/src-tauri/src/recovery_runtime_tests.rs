use super::*;
use crate::agent_resource_snapshot::persist_agent_resource_snapshot;
use tools::encode_input;

#[test]
fn recovery_identity_stays_on_root_prompt_after_steer() {
    let run_start = Event {
        id: EventId("run-start".to_string()),
        task_id: phase16_task_id(),
        sequence: 20,
        timestamp_ms: 100,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task started".to_string(),
        metadata: [
            (
                "agent_run_identity_schema".to_string(),
                "cindx.agent-run-identity.v1".to_string(),
            ),
            ("agent_run_id".to_string(), "run-a".to_string()),
            (
                "logical_agent_run_id".to_string(),
                "logical-run-a".to_string(),
            ),
            ("prompt".to_string(), "Review the report".to_string()),
            (
                "model_prompt".to_string(),
                "Review the report\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
            (
                "recovery_prompt".to_string(),
                "Review the report\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let root_turn = Event {
        id: EventId("root-turn".to_string()),
        task_id: phase16_task_id(),
        sequence: 21,
        timestamp_ms: 101,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), "Review the report".to_string()),
            (
                "model_content".to_string(),
                "Review the report\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let steer_turn = Event {
        id: EventId("steer-turn".to_string()),
        task_id: phase16_task_id(),
        sequence: 22,
        timestamp_ms: 102,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("role".to_string(), "user".to_string()),
            (
                "content".to_string(),
                "Focus on security findings".to_string(),
            ),
            (
                "display_content".to_string(),
                "Focus on security findings".to_string(),
            ),
            ("steer".to_string(), "true".to_string()),
            ("queue_mode".to_string(), "steer".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let events = vec![run_start, root_turn, steer_turn];

    assert_eq!(
        latest_agent_prompt_from_active_events(&events).as_deref(),
        Some("Focus on security findings")
    );
    assert_eq!(
        latest_agent_display_prompt_from_active_events(&events).as_deref(),
        Some("Focus on security findings")
    );
    assert_eq!(
        agent_recovery_prompt_from_active_events(&events).as_deref(),
        Some("Review the report\n\nAttached files: /workspace/report.pdf")
    );
    assert_eq!(
        primary_agent_user_turn_event(&events).map(|event| event.sequence),
        Some(21)
    );
    let recovery_context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
    ]
    .into_iter()
    .collect();
    let recovery =
        crate::agent_recovery_identity::resolve_agent_recovery_identity(&events, &recovery_context)
            .expect("recovery identity should resolve");
    assert_eq!(recovery.identity.source_run_id, "run-a");
    assert_eq!(
        recovery.identity.logical_run_id.as_deref(),
        Some("logical-run-a")
    );
    assert_eq!(recovery.identity.user_turn_sequence, 21);
    assert_eq!(
        recovery.identity.prompt_fingerprint,
        sha256_hex("Review the report\n\nAttached files: /workspace/report.pdf".as_bytes())
    );
    assert_eq!(
        recovery.prompt,
        "Review the report\n\nAttached files: /workspace/report.pdf"
    );
}

#[test]
fn startup_recovery_preserves_unfinished_agent_runs_as_continuations() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, run_id) in [("session-a", "run-a"), ("session-b", "run-b")] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [
                ("session_id".to_string(), session_id.to_string()),
                ("agent_run_id".to_string(), run_id.to_string()),
                ("prompt".to_string(), "finish the task".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("run start should append");
    }
    let recovery_control = AgentRunControl::new("fast");
    let _attempt = recovery_control
        .begin_physical_model_attempt("model-a", 12, 8, RunStageClass::Worker)
        .expect("resource attempt should reserve");
    persist_agent_resource_snapshot(
        &mut store,
        &[
            ("session_id".to_string(), "session-b".to_string()),
            ("agent_run_id".to_string(), "run-b".to_string()),
        ]
        .into_iter()
        .collect(),
        &recovery_control.resource_usage(),
    )
    .expect("resource checkpoint should persist");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("completion should append");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let completed = agent_state_for_session(&store, None, Some("session-a"))
        .expect("completed state should load");
    let interrupted = agent_state_for_session(&store, None, Some("session-b"))
        .expect("interrupted state should load");

    assert_eq!(completed.status, "completed");
    assert_eq!(interrupted.status, "paused");
    assert!(interrupted.can_retry);
    assert!(interrupted.can_continue);
    assert!(!interrupted.can_cancel);
    assert!(interrupted.last_error.is_none());
    let interrupted_events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-b")
        .expect("interrupted events should load");
    let envelope = latest_agent_recovery_envelope(&interrupted_events)
        .expect("recovery envelope should persist");
    assert_eq!(envelope.schema, AGENT_RECOVERY_SCHEMA);
    assert_eq!(envelope.state, AgentRecoveryState::Paused);
    assert_eq!(envelope.reason, AgentRecoveryReason::AppRestarted);
    assert_eq!(envelope.identity.source_run_id, "run-b");
    let resources = envelope
        .resource_snapshot
        .expect("resource checkpoint should transfer to recovery envelope");
    assert_eq!(resources.segment.physical_attempts, 1);
    assert_eq!(resources.segment.reserved_tokens, 20);
    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should be idempotent"),
        0
    );
}

#[test]
fn goal2_startup_recovery_preserves_the_latest_durable_steer_epoch() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let base = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-steered".to_string()),
        ("agent_run_id".to_string(), "run-steered".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [
                ("prompt".to_string(), "Keep every safety check".to_string()),
                (
                    "initial_prompt_objective".to_string(),
                    "Keep every safety check".to_string(),
                ),
                ("steer_epoch".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Keep every safety check",
        metadata_with_context(
            [("steer_epoch".to_string(), "0".to_string())]
                .into_iter()
                .collect(),
            &base,
        ),
    )
    .expect("initial prompt should persist");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Continue after fixing the race",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "1".to_string()),
                ("queue_mode".to_string(), "steer".to_string()),
                ("queue_id".to_string(), "queue-steer-1".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("steer should persist");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-steered")
        .expect("events should load");
    let pause = events
        .iter()
        .rev()
        .find(|event| event.summary == "Agent task paused")
        .expect("restart should persist a pause");
    assert_eq!(
        pause.metadata.get("steer_epoch").map(String::as_str),
        Some("1")
    );
    assert_eq!(latest_applied_agent_steer_epoch(&events), 1);
    assert_eq!(
        pause
            .metadata
            .get("initial_prompt_objective")
            .map(String::as_str),
        Some("Keep every safety check")
    );

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task retry started",
        metadata_with_context(
            [
                (
                    "prompt".to_string(),
                    "Continue after fixing the race".to_string(),
                ),
                ("steer_epoch".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("retry should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Continue after fixing the race",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "1".to_string()),
                ("continuation_replay".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("retry continuation should persist");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("second recovery should succeed"),
        1
    );
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-steered")
        .expect("recovered retry events should load");
    let active_events = active_agent_events_for_session(&events, Some("session-steered"));
    assert_eq!(latest_applied_agent_steer_epoch(&active_events), 1);
    let second_pause = active_events
        .iter()
        .rev()
        .find(|event| event.summary == "Agent task paused")
        .expect("second restart should persist a pause");
    assert_eq!(
        second_pause.metadata.get("steer_epoch").map(String::as_str),
        Some("1")
    );
}

#[test]
fn startup_recovery_preserves_pending_permission_as_blocked() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "pro".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut start = context.clone();
    start.insert("prompt".to_string(), "write the report".to_string());
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        start,
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "write the report",
        context.clone(),
    )
    .expect("user message should append");

    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("call-write".to_string()),
        task_id: phase16_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", "report.md"), ("content", "draft")]),
        proposed_by_model: "agent-loop".to_string(),
        metadata: context.clone(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("file.write")
        .expect("write tool should exist")
        .permission_request(&invocation)
        .expect("write should require permission");
    request.id = PermissionRequestId("permission-write".to_string());
    request.task_id = phase16_task_id();
    request.metadata.extend(context.clone());
    request
        .metadata
        .insert("tool_call_id".to_string(), "call-write".to_string());
    request
        .metadata
        .insert("tool_name".to_string(), "file.write".to_string());
    store
        .save_permission_request(request, current_time_millis())
        .expect("permission should persist");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("blocked state should load");
    assert_eq!(state.status, "waiting_for_permission");
    assert_eq!(state.pending_approvals.len(), 1);
    assert!(!state.can_continue);
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should load");
    let envelope =
        latest_agent_recovery_envelope(&events).expect("blocked recovery envelope should persist");
    assert_eq!(envelope.state, AgentRecoveryState::Blocked);
    assert_eq!(envelope.effort, "pro");
    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should be idempotent"),
        0
    );
    let claimed = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Blocked],
        AgentRecoveryReason::PermissionResolved,
    )
    .expect("blocked recovery claim should succeed")
    .expect("blocked checkpoint should exist");
    assert_eq!(claimed.attempts, 1);
    let duplicate = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Blocked],
        AgentRecoveryReason::PermissionResolved,
    )
    .expect_err("a blocked recovery must not be claimed twice");
    assert!(duplicate.contains("already claimed"));
}

#[test]
fn recovery_envelope_is_bound_to_the_latest_external_user_turn() {
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "auto".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut events = vec![
        Event {
            id: EventId("start".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata_with_context(
                [("prompt".to_string(), "finish alpha".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("user-alpha".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: metadata_with_context(
                [
                    ("role".to_string(), "user".to_string()),
                    ("content".to_string(), "finish alpha".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    ];
    let envelope = build_agent_recovery_envelope_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        30,
        None,
        None,
    )
    .expect("envelope should build");
    assert!(recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));

    events.push(Event {
        id: EventId("replay".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 30,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "finish alpha".to_string()),
                ("continuation_replay".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    });
    assert!(recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));

    events.push(Event {
        id: EventId("user-beta".to_string()),
        task_id: phase16_task_id(),
        sequence: 4,
        timestamp_ms: 40,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "start beta".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    });
    assert!(!recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));
}

#[test]
fn recovery_envelope_round_trips_the_kernel_task_checkpoint() {
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = vec![
        Event {
            id: EventId("start".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata_with_context(
                [("prompt".to_string(), "finish alpha".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("user-alpha".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: metadata_with_context(
                [
                    ("role".to_string(), "user".to_string()),
                    ("content".to_string(), "finish alpha".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    ];
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "finish alpha",
        AgentRuntimeConfig::default(),
    );
    runtime.turn = 3;
    record_tool_outcome_with_risk(
        &mut runtime,
        "file.write",
        r#"{"path":"report.md"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::WritesWorkspace),
    );
    let checkpoint = AgentTaskStateSnapshot::capture(&runtime);
    let envelope = build_agent_recovery_envelope_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        30,
        Some(&checkpoint),
        None,
    )
    .expect("envelope should build");
    let encoded = serde_json::to_string(&envelope).expect("envelope encodes");
    let encoded_value =
        serde_json::from_str::<serde_json::Value>(&encoded).expect("envelope JSON decodes");
    assert_eq!(encoded_value["resumeKey"], envelope.identity.resume_key);
    assert_eq!(encoded_value["sessionId"], "session-a");
    assert_eq!(encoded_value["state"], "paused");
    assert_eq!(encoded_value["reason"], "deadline_exceeded");
    assert!(encoded_value.get("identity").is_none());
    let decoded =
        serde_json::from_str::<AgentRecoveryEnvelope>(&encoded).expect("envelope decodes");
    let restored = decoded
        .task_state
        .expect("task checkpoint persists")
        .restore("finish alpha", runtime.messages.clone())
        .expect("checkpoint restores");

    assert_eq!(restored.turn, 3);
    assert_eq!(restored.successful_mutations(), 1);

    let mut invalid = encoded_value;
    invalid["taskState"]["schema"] = "cindx.agent.task-state.v999".into();
    assert!(serde_json::from_value::<AgentRecoveryEnvelope>(invalid).is_err());
}

#[test]
fn recovery_claim_is_single_use_and_recovered_if_restart_interrupts_claim() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "pro".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "finish alpha".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "finish alpha",
        context.clone(),
    )
    .expect("user message should append");
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should load");
    let recovery_metadata = agent_recovery_metadata_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        [("completion".to_string(), "partial".to_string())]
            .into_iter()
            .collect(),
        None,
        None,
    )
    .expect("recovery metadata should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        recovery_metadata,
    )
    .expect("pause should persist");

    let claimed = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Paused],
        AgentRecoveryReason::UserContinued,
    )
    .expect("claim should succeed")
    .expect("checkpoint should exist");
    assert_eq!(claimed.attempts, 1);
    let duplicate = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Paused],
        AgentRecoveryReason::UserContinued,
    )
    .expect_err("a claimed recovery must not be claimed twice");
    assert!(duplicate.contains("already claimed"));

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store)
            .expect("a restart should pause an interrupted claim"),
        1
    );
    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("recovered state should load");
    assert_eq!(state.status, "paused");
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should reload");
    let recovered =
        latest_agent_recovery_envelope(&events).expect("recovered checkpoint should persist");
    assert_eq!(recovered.state, AgentRecoveryState::Paused);
    assert_eq!(recovered.attempts, 1);
}

#[test]
fn recovery_transcript_marks_unfinished_tool_calls_unknown() {
    let mut events = vec![
        Event {
            id: EventId("user".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "update report".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("assistant-tool".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "assistant message".to_string(),
            metadata: [
                ("role".to_string(), "assistant".to_string()),
                ("content".to_string(), String::new()),
                ("tool_call_ids".to_string(), "call-write".to_string()),
                ("raw_tool_calls_json".to_string(), "[]".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ];
    let interrupted = recovery_safe_transcript(&events);
    assert_eq!(interrupted.len(), 3);
    assert_eq!(interrupted[2].role, MessageRole::Tool);
    assert_eq!(
        interrupted[2].metadata.get("status").map(String::as_str),
        Some("interrupted")
    );
    assert!(interrupted[2].content.contains("outcome as unknown"));

    events.push(Event {
        id: EventId("tool-result".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 30,
        kind: EventKind::MessageAdded,
        summary: "tool message".to_string(),
        metadata: [
            ("role".to_string(), "tool".to_string()),
            ("content".to_string(), "write completed".to_string()),
            ("tool_call_id".to_string(), "call-write".to_string()),
            ("status".to_string(), "succeeded".to_string()),
        ]
        .into_iter()
        .collect(),
    });
    let resolved = recovery_safe_transcript(&events);
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved[2].content, "write completed");
}
