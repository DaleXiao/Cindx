use super::*;

#[test]
fn error_terminalization_rolls_back_event_and_snapshot_cleanup_together() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = [
        ("project_id".to_string(), "project-error".to_string()),
        ("session_id".to_string(), "session-error".to_string()),
        ("agent_run_id".to_string(), "run-error".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    for namespace in [
        AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
        AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
    ] {
        store
            .save_read_model(namespace, "session-error", 0, "snapshot-before")
            .expect("snapshot should persist");
    }
    let failed_ledger = agent_runtime::AgentTaskContract::default().failed_outcome_ledger(
        0,
        &agent_runtime::AgentFailure::new(
            "provider_timeout",
            "SENSITIVE_PROVIDER_FAILURE_SENTINEL",
            agent_runtime::AgentFailureClass::ProviderTransient,
            true,
        ),
    );
    let mut terminal_metadata = Metadata::new();
    assert!(failed_ledger.insert_metadata(&mut terminal_metadata));
    terminal_metadata.insert("outcome_ledger_status".to_string(), "recorded".to_string());

    let error = persist_agent_error_terminalization_with(
        &mut store,
        &run_context,
        "provider failed",
        terminal_metadata.clone(),
        |store, session_id| {
            store.delete_read_model(
                AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
                session_id.expect("session should be present"),
            )?;
            Err(StorageError::new("injected snapshot cleanup failure"))
        },
    )
    .expect_err("injected cleanup failure should abort terminalization");
    assert!(error.contains("injected snapshot cleanup failure"));

    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-error")
        .expect("events should load");
    assert!(events.is_empty(), "failed terminalization leaked an event");
    for namespace in [
        AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
        AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
    ] {
        assert_eq!(
            store
                .load_read_model(namespace, "session-error")
                .expect("snapshot should load")
                .map(|model| model.payload),
            Some("snapshot-before".to_string())
        );
    }

    let projected_state = persist_agent_error_terminalization_with_metadata(
        &mut store,
        &run_context,
        "provider failed",
        terminal_metadata,
    )
    .expect("terminalization retry should succeed");
    assert_eq!(projected_state.status, "failed");
    assert_eq!(
        projected_state.last_error.as_deref(),
        Some("provider failed")
    );
    let events = store
        .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-error")
        .expect("events should reload");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Error);
    assert_eq!(
        events[0].metadata.get("error").map(String::as_str),
        Some("provider failed")
    );
    assert_eq!(
        events[0]
            .metadata
            .get("outcome_ledger_status")
            .map(String::as_str),
        Some("recorded")
    );
    let restored_ledger = agent_runtime::OutcomeLedgerShadow::from_terminal_metadata(
        &events[0].metadata,
        agent_runtime::OutcomeLedgerPhase::Failed,
    )
    .expect("failed outcome ledger should replay");
    assert_eq!(restored_ledger, failed_ledger);
    assert_eq!(
        restored_ledger.phase,
        agent_runtime::OutcomeLedgerPhase::Failed
    );
    assert!(
        !events[0].metadata[agent_runtime::OUTCOME_LEDGER_METADATA_KEY]
            .contains("SENSITIVE_PROVIDER_FAILURE_SENTINEL")
    );
    for namespace in [
        AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
        AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
    ] {
        assert!(store
            .load_read_model(namespace, "session-error")
            .expect("snapshot absence should load")
            .is_none());
    }
}
