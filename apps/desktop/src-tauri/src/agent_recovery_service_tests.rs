use super::*;
use crate::agent_strategy_receipt_runtime::bind_strategy_receipt_from_events;
use crate::append_message_event_with_metadata;
use agent_application::{
    insert_strategy_not_selected, strategy_receipt_is_explicitly_not_selected,
    AgentStrategyDecisionReceipt,
};
use agent_core::{
    insert_event_type_v1, EventId, EventTypeV1, AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY,
    AGENT_RUN_IDENTITY_V1_SCHEMA, EVENT_TYPE_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
    SOURCE_AGENT_RUN_ID_METADATA_KEY,
};

fn run_metadata(session_id: &str, run_id: &str, with_prompt: bool) -> Metadata {
    let mut metadata = [
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
        (
            AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
            AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
        ),
        (
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            run_id.to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    if with_prompt {
        metadata.insert("prompt".to_string(), "finish the task".to_string());
    }
    metadata
}

#[test]
fn agent_strategy_lifecycle_contract_recovery_binds_or_explains() {
    let mut context = run_metadata("session-receipt", "run-receipt", true);
    context.insert("steer_epoch".to_string(), "3".to_string());
    let receipt =
        AgentStrategyDecisionReceipt::new(&phase16_task_id(), &context, &"a".repeat(64)).unwrap();
    let mut decision_metadata = context.clone();
    receipt.insert_into(&mut decision_metadata).unwrap();
    insert_event_type_v1(
        &EventKind::TaskStatusChanged,
        &mut decision_metadata,
        EventTypeV1::AgentRunDecisionSelected,
    )
    .unwrap();
    let decision = Event {
        id: EventId("decision-receipt".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "localized decision".to_string(),
        metadata: decision_metadata,
    };

    let mut recovered = Metadata::new();
    bind_strategy_receipt_from_events(std::slice::from_ref(&decision), &context, &mut recovered)
        .unwrap();
    let recovered = metadata_with_context(recovered, &context);
    assert_eq!(
        AgentStrategyDecisionReceipt::from_metadata(&recovered).unwrap(),
        Some(receipt.clone())
    );

    let mut stale_context = run_metadata("session-receipt", "run-prior", true);
    stale_context.insert("steer_epoch".to_string(), "9".to_string());
    let stale_receipt =
        AgentStrategyDecisionReceipt::new(&phase16_task_id(), &stale_context, &"c".repeat(64))
            .unwrap();
    stale_receipt.insert_into(&mut stale_context).unwrap();
    insert_event_type_v1(
        &EventKind::TaskStatusChanged,
        &mut stale_context,
        EventTypeV1::AgentRunDecisionSelected,
    )
    .unwrap();
    let stale_decision = Event {
        id: EventId("decision-prior-run".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "prior decision".to_string(),
        metadata: stale_context,
    };
    let mut start_metadata = run_metadata("session-receipt", "run-receipt", true);
    start_metadata.insert("steer_epoch".to_string(), "3".to_string());
    insert_event_type_v1(
        &EventKind::TaskStatusChanged,
        &mut start_metadata,
        EventTypeV1::AgentRunStarted,
    )
    .unwrap();
    let start = Event {
        id: EventId("start-current-run".to_string()),
        task_id: phase16_task_id(),
        sequence: 2,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "localized start".to_string(),
        metadata: start_metadata,
    };
    let mut retained_context = run_metadata("session-receipt", "run-receipt", true);
    retained_context.insert("steer_epoch".to_string(), "4".to_string());
    let retained_receipt =
        AgentStrategyDecisionReceipt::new(&phase16_task_id(), &retained_context, &"d".repeat(64))
            .unwrap();
    retained_receipt.insert_into(&mut retained_context).unwrap();
    insert_event_type_v1(
        &EventKind::TaskStatusChanged,
        &mut retained_context,
        EventTypeV1::AgentRunDecisionSelected,
    )
    .unwrap();
    let retained_decision = Event {
        id: EventId("decision-retained".to_string()),
        task_id: phase16_task_id(),
        sequence: 4,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "retained decision".to_string(),
        metadata: retained_context.clone(),
    };
    let mut malformed_metadata = run_metadata("session-receipt", "run-receipt", true);
    malformed_metadata.insert("steer_epoch".to_string(), "10".to_string());
    insert_event_type_v1(
        &EventKind::TaskStatusChanged,
        &mut malformed_metadata,
        EventTypeV1::AgentRunDecisionSelected,
    )
    .unwrap();
    let malformed_decision = Event {
        id: EventId("decision-malformed".to_string()),
        task_id: phase16_task_id(),
        sequence: 5,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "malformed decision".to_string(),
        metadata: malformed_metadata,
    };
    let active_events = vec![
        stale_decision,
        start,
        decision.clone(),
        retained_decision,
        malformed_decision,
    ];
    assert_eq!(latest_applied_agent_steer_epoch(&active_events), 4);
    let mut retained_recovery = Metadata::new();
    bind_strategy_receipt_from_events(&active_events, &retained_context, &mut retained_recovery)
        .unwrap();
    let retained_recovery = metadata_with_context(retained_recovery, &retained_context);
    assert_eq!(
        AgentStrategyDecisionReceipt::from_metadata(&retained_recovery).unwrap(),
        Some(retained_receipt)
    );

    let mut pre_decision = Metadata::new();
    bind_strategy_receipt_from_events(&[], &context, &mut pre_decision).unwrap();
    assert!(strategy_receipt_is_explicitly_not_selected(&pre_decision));

    let mut stale_not_selected = context.clone();
    insert_strategy_not_selected(&mut stale_not_selected, 2).unwrap();
    assert!(
        bind_strategy_receipt_from_events(&[], &stale_not_selected, &mut Metadata::new(),)
            .expect_err("stale not-selected epoch must fail closed")
            .contains("mismatched epoch")
    );

    let conflicting =
        AgentStrategyDecisionReceipt::new(&phase16_task_id(), &context, &"b".repeat(64)).unwrap();
    let mut conflicting_context = context.clone();
    conflicting.insert_into(&mut conflicting_context).unwrap();
    assert!(bind_strategy_receipt_from_events(
        std::slice::from_ref(&decision),
        &conflicting_context,
        &mut Metadata::new(),
    )
    .expect_err("mismatched context must fail closed")
    .contains("does not match"));
    assert!(bind_strategy_receipt_from_events(
        &[decision.clone(), decision],
        &context,
        &mut Metadata::new(),
    )
    .expect_err("duplicate decision must fail closed")
    .contains("duplicate"));
}

#[test]
fn recovery_checkpoint_keeps_physical_source_separate_from_attempt_predecessor() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let mut context = run_metadata("session-lineage", "attempt-b", true);
    context.insert(
        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
        "logical-root".to_string(),
    );
    context.insert(
        SOURCE_AGENT_RUN_ID_METADATA_KEY.to_string(),
        "attempt-a".to_string(),
    );
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task retry started",
        context.clone(),
    )
    .expect("continuation should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "finish the task",
        context.clone(),
    )
    .expect("prompt should persist");
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-lineage")
        .expect("events should load");

    let checkpoint = agent_recovery_metadata_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        Metadata::new(),
        None,
        None,
    )
    .expect("checkpoint should build");
    let envelope = serde_json::from_str::<AgentRecoveryEnvelope>(
        checkpoint
            .get("recovery_envelope")
            .expect("checkpoint should encode its envelope"),
    )
    .expect("recovery envelope should decode");

    assert_eq!(
        checkpoint
            .get(SOURCE_AGENT_RUN_ID_METADATA_KEY)
            .map(String::as_str),
        Some("attempt-a")
    );
    assert_eq!(envelope.identity.source_run_id, "attempt-b");
    assert_eq!(envelope.identity.logical_run_id(), "logical-root");
}

#[test]
fn legacy_three_attempt_recovery_inherits_the_root_logical_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let legacy_context = |attempt_run_id: &str, source_attempt_run_id: Option<&str>| {
        let mut metadata = [
            (
                "session_id".to_string(),
                "session-legacy-lineage".to_string(),
            ),
            ("agent_run_id".to_string(), attempt_run_id.to_string()),
            ("prompt".to_string(), "finish the task".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        if let Some(source_attempt_run_id) = source_attempt_run_id {
            metadata.insert(
                SOURCE_AGENT_RUN_ID_METADATA_KEY.to_string(),
                source_attempt_run_id.to_string(),
            );
        }
        metadata
    };
    let context_a = legacy_context("attempt-a", None);
    let context_b = legacy_context("attempt-b", Some("attempt-a"));
    let context_c = legacy_context("attempt-c", Some("attempt-b"));
    for (summary, context) in [
        ("Agent task started", &context_a),
        ("Agent task retry started", &context_b),
        ("Agent task retry started", &context_c),
    ] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            summary,
            context.clone(),
        )
        .expect("attempt should start");
    }
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "finish the task",
        context_c.clone(),
    )
    .expect("active prompt should persist");
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-legacy-lineage")
        .expect("legacy events should load");
    let active = active_agent_events_for_session(&events, Some("session-legacy-lineage"));
    let mut envelope = build_agent_recovery_envelope_with_task_state(
        &active,
        &context_c,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        10,
        None,
        None,
    )
    .expect("legacy checkpoint should build");
    envelope.identity.logical_run_id = None;
    let mut pause_metadata = context_c.clone();
    pause_metadata.insert("recovery_state".to_string(), "paused".to_string());
    pause_metadata.insert(
        "recovery_envelope".to_string(),
        serde_json::to_string(&envelope).expect("legacy envelope should encode"),
    );
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        pause_metadata,
    )
    .expect("legacy pause should persist");

    let recovered = peek_agent_recovery_envelope(&store, &context_c, &[AgentRecoveryState::Paused])
        .expect("legacy recovery should be readable")
        .expect("legacy recovery should remain available");

    assert_eq!(recovered.identity.source_run_id, "attempt-c");
    assert_eq!(
        recovered.identity.logical_run_id.as_deref(),
        Some("attempt-a")
    );
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
                        (
                            "tool_input_fingerprint".to_string(),
                            input_fingerprint.clone(),
                        ),
                        (
                            "action_denial_schema".to_string(),
                            agent_runtime::ACTION_DENIAL_SCHEMA.to_string(),
                        ),
                        (
                            "action_denial_kind".to_string(),
                            "user_permission".to_string(),
                        ),
                        (
                            "action_denial_code".to_string(),
                            "user_permission_denied".to_string(),
                        ),
                        (
                            "action_denial_recovery".to_string(),
                            "finalize_blocked".to_string(),
                        ),
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
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-claim-rollback")
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
    assert_eq!(
        paused
            .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
            .map(String::as_str),
        Some("run-claim-rollback")
    );
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
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            context.clone(),
        )?;
        let mut replay = context.clone();
        replay.insert("continuation_replay".to_string(), "true".to_string());
        append_message_event_with_metadata(
            store,
            &phase16_task_id(),
            MessageRole::User,
            "finish the task",
            replay,
        )?;
        Err::<(), _>(StorageError::new("injected post-replay failure"))
    });
    assert!(injected.is_err());

    let rolled_back = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-claim-rollback")
        .expect("rolled-back events should load");
    assert!(!rolled_back
        .iter()
        .any(|event| event.summary == "Recovery resume claimed"));
    assert!(!rolled_back
        .iter()
        .any(|event| event.summary == "Agent task retry started"));
    assert!(!rolled_back.iter().any(|event| {
        event
            .metadata
            .get("continuation_replay")
            .map(String::as_str)
            == Some("true")
    }));

    let claimed = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Paused],
        AgentRecoveryReason::UserContinued,
    )
    .expect("rolled-back claim should remain available")
    .expect("paused checkpoint should still exist");
    assert_eq!(claimed.attempts, 1);
    let claimed_events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-claim-rollback")
        .expect("claimed recovery events should load");
    let claim_event = claimed_events
        .iter()
        .rev()
        .find(|event| event.summary == "Recovery resume claimed")
        .expect("recovery claim should be recorded");
    assert_eq!(
        claim_event
            .metadata
            .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
            .map(String::as_str),
        Some("run-claim-rollback")
    );
    assert_eq!(
        claim_event.metadata.get("agent_run_id").map(String::as_str),
        Some("run-claim-rollback")
    );
    assert!(
        pause_permission_recovery_after_handoff_error(&mut store, &context)
            .expect("a failed permission handoff should release the claim")
    );
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-claim-rollback")
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
