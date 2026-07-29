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

fn append_durable_memory_event_at(
    store: &mut SqliteStore,
    event_id: &str,
    timestamp_ms: u64,
    kind: EventKind,
    summary: &str,
    metadata: Metadata,
) {
    store
        .append_next_event(
            EventId(event_id.to_string()),
            phase16_task_id(),
            timestamp_ms,
            kind,
            summary.to_string(),
            metadata,
        )
        .expect("durable memory event should append");
}

fn durable_requirement_metadata(
    project_id: &str,
    session_id: &str,
    run_id: &str,
    content: &str,
) -> Metadata {
    [
        ("project_id".to_string(), project_id.to_string()),
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
        ("role".to_string(), "user".to_string()),
        ("content".to_string(), content.to_string()),
    ]
    .into_iter()
    .collect()
}

fn durable_requirement_id(
    project_id: &str,
    session_id: &str,
    run_id: &str,
    content: &str,
) -> String {
    extract_durable_memories(
        &[Event {
            id: EventId("event-memory-id-projection".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: durable_requirement_metadata(project_id, session_id, run_id, content),
        }],
        project_id,
        session_id,
    )[0]
    .id
    .clone()
}

fn append_durable_requirement_run(
    store: &mut SqliteStore,
    project_id: &str,
    session_id: &str,
    run_id: &str,
    content: &str,
) {
    append_durable_memory_event_at(
        store,
        "event-memory-requirement",
        100,
        EventKind::MessageAdded,
        "user message",
        durable_requirement_metadata(project_id, session_id, run_id, content),
    );
    append_durable_memory_event_at(
        store,
        "event-memory-completed",
        200,
        EventKind::TaskStatusChanged,
        "Agent task completed",
        [
            ("project_id".to_string(), project_id.to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
        ]
        .into_iter()
        .collect(),
    );
}

#[test]
fn v2_rebuild_replays_only_well_formed_measurements_in_durable_order() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_id = "project-memory-v2-migration";
    let session_id = "session-memory-v2-migration";
    let run_id = "run-memory-v2-migration";
    let content = "Always preserve project memory feedback during projection rebuilds";
    let memory_id = durable_requirement_id(project_id, session_id, run_id, content);

    append_durable_memory_event_at(
        &mut store,
        "event-memory-recall-before-record",
        50,
        EventKind::RetrievalPerformed,
        "Project memory recalled",
        [
            ("project_id".to_string(), project_id.to_string()),
            ("action".to_string(), "memory_recall".to_string()),
            ("selected_count".to_string(), "1".to_string()),
            ("memory_ids".to_string(), memory_id.clone()),
        ]
        .into_iter()
        .collect(),
    );
    append_durable_requirement_run(&mut store, project_id, session_id, run_id, content);
    append_durable_memory_event_at(
        &mut store,
        "event-memory-recall-missing-count",
        250,
        EventKind::RetrievalPerformed,
        "Project memory recalled",
        [
            ("project_id".to_string(), project_id.to_string()),
            ("action".to_string(), "memory_recall".to_string()),
            ("memory_ids".to_string(), memory_id.clone()),
        ]
        .into_iter()
        .collect(),
    );
    append_durable_memory_event_at(
        &mut store,
        "event-memory-use-invalid-count",
        275,
        EventKind::RetrievalPerformed,
        "Project memory utilization measured",
        [
            ("project_id".to_string(), project_id.to_string()),
            ("action".to_string(), "memory_use".to_string()),
            ("selected_count".to_string(), "1".to_string()),
            ("used_count".to_string(), "invalid".to_string()),
            ("memory_ids".to_string(), memory_id.clone()),
            ("used_memory_ids".to_string(), memory_id.clone()),
        ]
        .into_iter()
        .collect(),
    );
    append_durable_memory_event_at(
        &mut store,
        "event-memory-recall-legacy",
        300,
        EventKind::RetrievalPerformed,
        "Project memory recalled",
        [
            ("project_id".to_string(), project_id.to_string()),
            ("action".to_string(), "memory_recall".to_string()),
            ("selected_count".to_string(), "1".to_string()),
            ("memory_ids".to_string(), memory_id.clone()),
        ]
        .into_iter()
        .collect(),
    );
    append_durable_memory_event_at(
        &mut store,
        "event-memory-use-legacy",
        400,
        EventKind::RetrievalPerformed,
        "Project memory utilization measured",
        [
            ("project_id".to_string(), project_id.to_string()),
            ("action".to_string(), "memory_use".to_string()),
            ("selected_count".to_string(), "1".to_string()),
            ("used_count".to_string(), "1".to_string()),
            ("memory_ids".to_string(), memory_id.clone()),
            ("used_memory_ids".to_string(), memory_id.clone()),
        ]
        .into_iter()
        .collect(),
    );

    let ledger = load_project_memory_ledger(&mut store, project_id)
        .expect("v2 memory projection should rebuild");
    let record = ledger
        .records
        .iter()
        .find(|record| record.id == memory_id)
        .expect("durable requirement should rebuild");
    assert_eq!(record.recall_count, 1);
    assert_eq!(record.last_recalled_at_ms, Some(300));
    assert_eq!(record.observed_use_count, 1);
    assert_eq!(record.last_observed_use_at_ms, Some(400));
}

#[test]
fn rebuild_preserves_runtime_measurement_counts_and_exact_timestamps() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_id = "project-memory-runtime-rebuild";
    let session_id = "session-memory-runtime-rebuild";
    let run_id = "run-memory-runtime-rebuild";
    let content = "Always keep project memory rebuilds lossless";
    append_durable_requirement_run(&mut store, project_id, session_id, run_id, content);
    let ledger = load_project_memory_ledger(&mut store, project_id)
        .expect("initial memory projection should build");
    let record = ledger.records[0].clone();
    let run_context = [
        ("project_id".to_string(), project_id.to_string()),
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
        ("memory_ids".to_string(), record.id.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let prepared = PreparedMemoryRecall {
        project_id: project_id.to_string(),
        ledger_projection_sha256: memory_vector_projection_sha256(&ledger),
        recalled_at_ms: 225,
        recalls: vec![agent_memory::MemoryRecall {
            record: record.clone(),
            score: 1.0,
            reasons: vec!["test".to_string()],
        }],
        event_metadata: [("action".to_string(), "memory_recall".to_string())]
            .into_iter()
            .collect(),
        message: Message {
            role: MessageRole::System,
            content: content.to_string(),
            metadata: Metadata::new(),
        },
    };
    commit_prepared_memory_recall(
        &mut store,
        &phase16_task_id(),
        &run_context,
        Some(&prepared),
    )
    .expect("runtime recall should persist");
    assert_eq!(
        record_project_memory_observed_use(&mut store, &phase16_task_id(), &run_context, content,)
            .expect("runtime memory use should persist"),
        1
    );
    let before = load_project_memory_ledger(&mut store, project_id)
        .expect("runtime-updated projection should load");
    let before_record = before
        .records
        .iter()
        .find(|candidate| candidate.id == record.id)
        .expect("updated requirement should exist")
        .clone();
    assert_eq!(before_record.last_recalled_at_ms, Some(225));

    store
        .delete_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, project_id)
        .expect("v2 projection should delete");
    let rebuilt = load_project_memory_ledger(&mut store, project_id)
        .expect("v2 memory projection should rebuild from durable events");
    let rebuilt_record = rebuilt
        .records
        .iter()
        .find(|candidate| candidate.id == record.id)
        .expect("rebuilt requirement should exist");
    assert_eq!(rebuilt_record.recall_count, before_record.recall_count);
    assert_eq!(
        rebuilt_record.last_recalled_at_ms,
        before_record.last_recalled_at_ms
    );
    assert_eq!(
        rebuilt_record.observed_use_count,
        before_record.observed_use_count
    );
    assert_eq!(
        rebuilt_record.last_observed_use_at_ms,
        before_record.last_observed_use_at_ms
    );
}
