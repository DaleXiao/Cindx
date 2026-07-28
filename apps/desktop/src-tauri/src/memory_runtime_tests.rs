use super::*;

fn prepared_recall(project_id: &str) -> (MemoryLedger, PreparedMemoryRecall) {
    let record = agent_memory::MemoryRecord {
        id: "memory-preparation-atomic".to_string(),
        fingerprint: "memory-preparation-atomic-fingerprint".to_string(),
        kind: agent_memory::MemoryKind::Requirement,
        trust: agent_memory::MemoryTrust::UserStated,
        content: "Keep preparation persistence atomic".to_string(),
        importance: 5,
        provenance: agent_memory::MemoryProvenance {
            project_id: project_id.to_string(),
            session_id: "session-preparation-atomic".to_string(),
            event_id: "event-preparation-atomic".to_string(),
            agent_run_id: Some("run-preparation-atomic".to_string()),
            sequence: 1,
            timestamp_ms: 1,
        },
        source_event_ids: vec!["event-preparation-atomic".to_string()],
        source_session_ids: vec!["session-preparation-atomic".to_string()],
        created_at_ms: 1,
        updated_at_ms: 1,
        recall_count: 0,
        last_recalled_at_ms: None,
        observed_use_count: 0,
        last_observed_use_at_ms: None,
        superseded_by: None,
        superseded_at_ms: None,
    };
    let mut ledger = MemoryLedger::new(project_id);
    ledger.records.push(record.clone());
    let prepared = PreparedMemoryRecall {
        project_id: project_id.to_string(),
        ledger_projection_sha256: memory_vector_projection_sha256(&ledger),
        recalled_at_ms: 2,
        recalls: vec![agent_memory::MemoryRecall {
            record,
            score: 1.0,
            reasons: vec!["test".to_string()],
        }],
        event_metadata: [("action".to_string(), "memory_recall".to_string())]
            .into_iter()
            .collect(),
        message: Message {
            role: MessageRole::System,
            content: "test memory".to_string(),
            metadata: Metadata::new(),
        },
    };
    (ledger, prepared)
}

#[test]
fn preparation_transaction_commits_or_rolls_back_progress_recall_and_ledger_together() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_id = "project-preparation-atomic";
    let task_id = phase16_task_id();
    let run_context = [
        ("project_id".to_string(), project_id.to_string()),
        (
            "session_id".to_string(),
            "session-preparation-atomic".to_string(),
        ),
        (
            "agent_run_id".to_string(),
            "run-preparation-atomic".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let (ledger, prepared) = prepared_recall(project_id);
    save_project_memory_ledger(&mut store, &ledger).expect("memory ledger should seed");

    let failed = store.with_immediate_transaction(|transaction| {
        append_event(
            transaction,
            &task_id,
            EventKind::TaskStatusChanged,
            "Starting execution",
            run_context.clone(),
        )?;
        commit_prepared_memory_recall(transaction, &task_id, &run_context, Some(&prepared))?;
        Err::<(), _>(StorageError::new("injected preparation failure"))
    });
    assert!(failed.is_err());
    assert!(store
        .list_by_task(&task_id)
        .expect("events should load")
        .is_empty());
    assert_eq!(
        load_project_memory_ledger(&mut store, project_id)
            .expect("memory ledger should load")
            .records[0]
            .recall_count,
        0
    );

    store
        .with_immediate_transaction(|transaction| {
            append_event(
                transaction,
                &task_id,
                EventKind::TaskStatusChanged,
                "Starting execution",
                run_context.clone(),
            )?;
            commit_prepared_memory_recall(transaction, &task_id, &run_context, Some(&prepared))
        })
        .expect("preparation transaction should commit");
    let events = store
        .list_by_task(&task_id)
        .expect("events should load after commit");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].summary, "Starting execution");
    assert_eq!(events[1].summary, "Project memory recalled");
    let committed = load_project_memory_ledger(&mut store, project_id)
        .expect("committed memory ledger should load");
    assert_eq!(committed.revision, events[1].sequence);
    assert_eq!(
        committed
            .records
            .iter()
            .map(|record| record.recall_count)
            .sum::<u64>(),
        1
    );
}

#[test]
fn preparation_rejects_a_memory_snapshot_that_changed_before_handoff() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_id = "project-preparation-stale";
    let task_id = phase16_task_id();
    let run_context = [
        ("project_id".to_string(), project_id.to_string()),
        ("session_id".to_string(), "session-stale".to_string()),
        ("agent_run_id".to_string(), "run-stale".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let (mut ledger, prepared) = prepared_recall(project_id);
    save_project_memory_ledger(&mut store, &ledger).expect("memory ledger should seed");
    let mut runtime = start_agent_loop(
        task_id.clone(),
        "Use the current project requirement",
        AgentRuntimeConfig::default(),
    );
    let original_messages = runtime.messages.clone();
    let (active_user, base_history) = original_messages
        .split_last()
        .expect("runtime should contain an active user prompt");
    let mut stale_history = base_history.to_vec();
    stale_history.push(prepared.message.clone());
    let control = AgentRunControl::new("pro");
    assert!(control.begin_preparation());

    ledger.records[0].content = "A newer requirement replaced this memory".to_string();
    ledger.records[0].fingerprint = "newer-memory-fingerprint".to_string();
    save_project_memory_ledger(&mut store, &ledger).expect("newer ledger should persist");

    let result = control.commit_preparation_with(0, || {
        store.with_immediate_transaction(|transaction| {
            append_event(
                transaction,
                &task_id,
                EventKind::TaskStatusChanged,
                "Starting execution",
                run_context.clone(),
            )?;
            commit_prepared_memory_recall(transaction, &task_id, &run_context, Some(&prepared))
        })?;
        stale_history.push(active_user.clone());
        runtime.messages = stale_history;
        Ok::<(), StorageError>(())
    });

    assert_eq!(
        result.expect_err("stale preparation must restart").message,
        MEMORY_RECALL_STALE_ERROR
    );
    assert_eq!(runtime.messages, original_messages);
    assert!(store
        .list_by_task(&task_id)
        .expect("rolled back events should load")
        .is_empty());
    assert_eq!(
        load_project_memory_ledger(&mut store, project_id)
            .expect("newer ledger should remain")
            .records[0]
            .recall_count,
        0
    );

    let mut replayed = prepared.clone();
    replayed.ledger_projection_sha256 = memory_vector_projection_sha256(&ledger);
    replayed.recalls[0].record = ledger.records[0].clone();
    replayed.message.content = ledger.records[0].content.clone();
    let mut replay_history = base_history.to_vec();
    replay_history.push(replayed.message.clone());
    let replay = control
        .commit_preparation_with(0, || {
            store.with_immediate_transaction(|transaction| {
                append_event(
                    transaction,
                    &task_id,
                    EventKind::TaskStatusChanged,
                    "Starting execution",
                    run_context.clone(),
                )?;
                commit_prepared_memory_recall(transaction, &task_id, &run_context, Some(&replayed))
            })?;
            replay_history.push(active_user.clone());
            runtime.messages = replay_history;
            Ok::<(), StorageError>(())
        })
        .expect("fresh preparation should commit");
    assert!(matches!(
        replay,
        agent_runtime::RunPreparationCommit::Committed { .. }
    ));
    assert!(!runtime
        .messages
        .iter()
        .any(|message| message.content == "test memory"));
    assert!(runtime
        .messages
        .iter()
        .any(|message| { message.content == "A newer requirement replaced this memory" }));
}
