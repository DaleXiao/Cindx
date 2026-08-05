use super::*;
use crate::append_message_event_with_metadata;
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
fn startup_recovery_replays_denial_after_a_pre_denial_snapshot() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let prompt = "protect the workspace";
    let run_context = [
        ("project_id".to_string(), "project-denial".to_string()),
        ("session_id".to_string(), "session-denial".to_string()),
        ("agent_run_id".to_string(), "run-denial".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
        ("prompt_contract_epoch".to_string(), "0".to_string()),
        ("effective_prompt_objective".to_string(), prompt.to_string()),
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
                ("prompt".to_string(), prompt.to_string()),
                ("initial_prompt_objective".to_string(), prompt.to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        prompt,
        run_context.clone(),
    )
    .expect("prompt should persist");
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        prompt,
        agent_runtime::AgentRuntimeConfig::default(),
    );
    runtime.task_contract.require_tool_success("shell.run");
    let task_state = agent_runtime::AgentTaskStateSnapshot::capture(&runtime);
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-denial")
        .expect("events should load");
    let recovery_metadata = agent_recovery_metadata_with_task_state(
        &events,
        &run_context,
        AgentRecoveryState::Blocked,
        AgentRecoveryReason::WaitingForPermission,
        Metadata::new(),
        Some(&task_state),
        None,
    )
    .expect("blocked recovery should encode");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task waiting for permission",
        recovery_metadata,
    )
    .expect("blocked checkpoint should persist");
    crate::agent_runtime_snapshot::persist_agent_runtime_snapshot(
        &mut store,
        &runtime,
        &run_context,
    )
    .expect("pre-denial snapshot should persist");

    let permission_id = agent_core::PermissionRequestId("permission-denial".to_string());
    let tool_input = r#"{"command":"touch protected"}"#;
    store
        .save_permission_request(
            agent_core::PermissionRequest {
                id: permission_id.clone(),
                task_id: phase16_task_id(),
                risk: agent_core::PermissionRisk::Execute,
                action: "shell.run".to_string(),
                reason: "requires approval".to_string(),
                scope: "workspace".to_string(),
                metadata: run_context.clone(),
            },
            10,
        )
        .expect("permission should persist");
    let resolution = agent_core::PermissionResolution {
        request_id: permission_id.clone(),
        decision: agent_core::PermissionDecision::Deny,
        resolved_at_ms: 20,
        resolved_by: "local-user".to_string(),
    };
    let input_fingerprint = agent_runtime::tool_input_fingerprint("shell.run", tool_input);
    store
        .with_immediate_transaction(|transaction| {
            transaction.resolve_permission_in_transaction(&resolution)?;
            append_event(
                transaction,
                &phase16_task_id(),
                EventKind::PermissionResolved,
                "Permission denied",
                metadata_with_context(
                    [
                        ("permission_id".to_string(), permission_id.0.clone()),
                        ("decision".to_string(), "deny".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    &run_context,
                ),
            )?;
            append_message_event_with_metadata(
                transaction,
                &phase16_task_id(),
                MessageRole::Tool,
                "The user denied this tool call.",
                metadata_with_context(
                    [
                        ("kind".to_string(), "tool_observation".to_string()),
                        ("tool_call_id".to_string(), "call-denial".to_string()),
                        ("tool".to_string(), "shell.run".to_string()),
                        ("status".to_string(), "denied".to_string()),
                        ("permission_id".to_string(), permission_id.0.clone()),
                        (
                            "permission_observation_schema".to_string(),
                            "cindx.permission-tool-observation.v1".to_string(),
                        ),
                        (
                            "permission_observation_provenance".to_string(),
                            "runtime_permission_resolution".to_string(),
                        ),
                        ("tool_input_fingerprint".to_string(), input_fingerprint.clone()),
                        ("action_denial_schema".to_string(), agent_runtime::ACTION_DENIAL_SCHEMA.to_string()),
                        ("action_denial_kind".to_string(), "user_permission".to_string()),
                        ("action_denial_code".to_string(), "user_permission_denied".to_string()),
                        ("action_denial_recovery".to_string(), "finalize_blocked".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    &run_context,
                ),
            )
        })
        .expect("resolution and denial observation should commit");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("startup recovery should reconcile"),
        1
    );
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-denial")
        .expect("recovered events should load");
    let recovered = latest_agent_recovery_envelope(&events)
        .unwrap_or_else(|| panic!("a paused recovery envelope should be present: {events:#?}"));
    assert_eq!(recovered.state, AgentRecoveryState::Paused);
    let recovered_task = recovered
        .task_state
        .expect("startup replay must preserve the typed task state");
    let ledger = recovered_task.task_contract.outcome_ledger_shadow(0);
    assert!(ledger.obligations.iter().any(|obligation| {
        obligation.satisfaction == agent_runtime::OutcomeSatisfaction::Blocked
            && obligation
                .blocker
                .as_ref()
                .is_some_and(|blocker| blocker.code == "user_permission_denied")
    }));
}

#[test]
fn recovery_claim_rolls_back_with_its_enclosing_transaction() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = run_metadata("session-claim-rollback", "run-claim-rollback", true);
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        context.clone(),
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "finish the task",
        context.clone(),
    )
    .expect("prompt should persist");
    let events = store
        .list_by_task_and_metadata(
            &phase16_task_id(),
            "session_id",
            "session-claim-rollback",
        )
        .expect("events should load");
    let paused = agent_recovery_metadata_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        Metadata::new(),
        None,
        None,
    )
    .expect("pause checkpoint should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        paused,
    )
    .expect("pause should persist");

    let injected = store.with_immediate_transaction(|store| {
        claim_agent_recovery_envelope_in_transaction(
            store,
            &context,
            &[AgentRecoveryState::Paused],
            AgentRecoveryReason::UserContinued,
        )
        .map_err(StorageError::new)?
        .expect("checkpoint should be claimable inside the transaction");
        Err::<(), _>(StorageError::new("injected post-claim failure"))
    });
    assert!(injected.is_err());

    let claimed = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Paused],
        AgentRecoveryReason::UserContinued,
    )
    .expect("rolled-back claim should remain available")
    .expect("paused checkpoint should still exist");
    assert_eq!(claimed.attempts, 1);
    assert!(pause_permission_recovery_after_handoff_error(&mut store, &context)
        .expect("a failed permission handoff should release the claim"));
    let events = store
        .list_by_task_and_metadata(
            &phase16_task_id(),
            "session_id",
            "session-claim-rollback",
        )
        .expect("released recovery events should load");
    let released = latest_agent_recovery_envelope(&events)
        .expect("the released claim should remain recoverable");
    assert_eq!(released.state, AgentRecoveryState::Paused);
    assert_eq!(released.reason.label(), "permission_handoff_failed");
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
        peek_agent_recovery_envelope(&store, &run_context, &[AgentRecoveryState::Paused])
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
        peek_agent_recovery_envelope(&store, &run_context, &[AgentRecoveryState::Paused])
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
        peek_agent_recovery_envelope(&store, &run_context, &[AgentRecoveryState::Paused])
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

#[test]
fn malformed_latest_recovery_envelope_does_not_revive_an_older_checkpoint() {
    let context = run_metadata("session-strict", "run-strict", true);
    let mut events = vec![Event {
        id: EventId("start-strict".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task started".to_string(),
        metadata: context.clone(),
    }];
    let envelope = build_agent_recovery_envelope_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::AppRestarted,
        20,
        None,
        None,
    )
    .expect("valid envelope should build");
    events.push(Event {
        id: EventId("valid-recovery".to_string()),
        task_id: phase16_task_id(),
        sequence: 2,
        timestamp_ms: 20,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task paused".to_string(),
        metadata: [(
            "recovery_envelope".to_string(),
            serde_json::to_string(&envelope).expect("valid envelope should encode"),
        )]
        .into_iter()
        .collect(),
    });
    let mut malformed = serde_json::to_value(&envelope).expect("valid envelope JSON");
    malformed["sessionId"] = "".into();
    events.push(Event {
        id: EventId("malformed-recovery".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 30,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task paused".to_string(),
        metadata: [(
            "recovery_envelope".to_string(),
            serde_json::to_string(&malformed).expect("malformed fixture should encode"),
        )]
        .into_iter()
        .collect(),
    });

    assert!(latest_agent_recovery_envelope(&events).is_none());
}
