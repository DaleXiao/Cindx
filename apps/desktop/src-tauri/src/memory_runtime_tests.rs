use super::*;
use crate::{
    runtime_constants::AGENT_MEMORY_READ_MODEL_NAMESPACE, runtime_values::phase16_task_id,
};
use agent_memory::{extract_durable_memories, MEMORY_LEDGER_SCHEMA};

fn prepared_recall(project_id: &str) -> (MemoryLedger, PreparedMemoryRecall) {
    let content = "Always keep preparation persistence atomic";
    let source = Event {
        id: EventId("event-preparation-atomic".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: durable_requirement_metadata(
            project_id,
            "session-preparation-atomic",
            "run-preparation-atomic",
            content,
        ),
    };
    let mut record =
        extract_durable_memories(&[source], project_id, "session-preparation-atomic").remove(0);
    record.importance = 5;
    let mut ledger = MemoryLedger::new(project_id);
    ledger.records.push(record.clone());
    ledger.revision = 1;
    ledger.event_count = 1;
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

fn append_prepared_recall_source(store: &mut SqliteStore, project_id: &str) {
    append_durable_memory_event_at(
        store,
        "event-preparation-atomic",
        1,
        EventKind::MessageAdded,
        "user message",
        durable_requirement_metadata(
            project_id,
            "session-preparation-atomic",
            "run-preparation-atomic",
            "Always keep preparation persistence atomic",
        ),
    );
}

#[test]
fn memory_rag_index_excludes_unverified_and_invalid_trust_records() {
    let (mut ledger, _) = prepared_recall("project-memory-rag-trust");
    let valid = ledger.records[0].clone();
    let mut missing_evidence = valid.clone();
    missing_evidence.id = "memory-missing-evidence".to_string();
    missing_evidence.fingerprint = "fingerprint-missing-evidence".to_string();
    missing_evidence.user_requirement_evidence.clear();
    let mut task_local = valid.clone();
    task_local.id = "memory-task-local".to_string();
    task_local.fingerprint = "fingerprint-task-local".to_string();
    task_local.user_requirement_evidence[0].scope = agent_memory::MemoryRequirementScope::TaskLocal;
    let mut paraphrased = valid.clone();
    paraphrased.id = "memory-model-paraphrase".to_string();
    paraphrased.fingerprint = "fingerprint-model-paraphrase".to_string();
    paraphrased.user_requirement_evidence[0].origin =
        agent_memory::MemoryClaimOrigin::ModelParaphrased;
    let mut invalid_kind = valid.clone();
    invalid_kind.id = "memory-invalid-kind".to_string();
    invalid_kind.fingerprint = "fingerprint-invalid-kind".to_string();
    invalid_kind.kind = MemoryKind::Evidence;
    ledger.records = vec![
        valid.clone(),
        missing_evidence,
        task_local,
        paraphrased,
        invalid_kind,
    ];

    let index = memory_rag_index(&ledger);
    assert_eq!(index.stats.chunks_indexed, 1);
    assert_eq!(index.chunks.len(), 1);
    assert_eq!(index.chunks[0].id, valid.id);
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
    append_prepared_recall_source(&mut store, project_id);
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
    assert_eq!(
        store
            .list_by_task(&task_id)
            .expect("events should load")
            .len(),
        1
    );
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
    assert_eq!(events.len(), 3);
    assert_eq!(events[1].summary, "Starting execution");
    assert_eq!(events[2].summary, "Project memory recalled");
    let committed = load_project_memory_ledger(&mut store, project_id)
        .expect("committed memory ledger should load");
    assert_eq!(committed.revision, events[2].sequence);
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
    append_prepared_recall_source(&mut store, project_id);
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

    let newer_content = "Always keep the newer preparation requirement active";
    append_durable_requirement_run(
        &mut store,
        project_id,
        "session-preparation-newer",
        "run-preparation-newer",
        newer_content,
    );
    ledger = load_project_memory_ledger(&mut store, project_id)
        .expect("newer authoritative ledger should persist");

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
        .iter()
        .all(|event| event.summary != "Starting execution"
            && event.summary != "Project memory recalled"));
    assert_eq!(
        load_project_memory_ledger(&mut store, project_id)
            .expect("newer ledger should remain")
            .records
            .iter()
            .map(|record| record.recall_count)
            .sum::<u64>(),
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
        .any(|message| { message.content == newer_content }));
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
    let requirement_event_id = format!("event-memory-requirement-{run_id}");
    let completed_event_id = format!("event-memory-completed-{run_id}");
    append_durable_memory_event_at(
        store,
        &requirement_event_id,
        100,
        EventKind::MessageAdded,
        "user message",
        durable_requirement_metadata(project_id, session_id, run_id, content),
    );
    append_durable_memory_event_at(
        store,
        &completed_event_id,
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
fn semantic_checkpoint_cannot_reintroduce_task_local_memory() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_id = "project-task-local-memory";
    let session_id = "session-task-local-memory";
    let run_id = "run-task-local-memory";
    let content = "本次只做评审，不要修改任何代码";
    append_durable_memory_event_at(
        &mut store,
        "event-task-local-user",
        100,
        EventKind::MessageAdded,
        "user message",
        durable_requirement_metadata(project_id, session_id, run_id, content),
    );
    append_durable_memory_event_at(
        &mut store,
        "event-task-local-completed",
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

    let terminal_ledger = load_project_memory_ledger(&mut store, project_id)
        .expect("terminal memory projection should load");
    assert!(terminal_ledger.records.is_empty());

    let candidates = serde_json::json!({
        "schema": agent_memory::SEMANTIC_MEMORY_BATCH_SCHEMA,
        "candidates": [{
            "kind": "requirement",
            "content": content,
            "importance": 100,
            "source_event_ids": ["event-task-local-user"],
        }],
    })
    .to_string();
    append_durable_memory_event_at(
        &mut store,
        "event-task-local-semantic",
        300,
        EventKind::TaskStatusChanged,
        "Semantic memory candidates accepted",
        [
            ("project_id".to_string(), project_id.to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
            ("memory_candidates_json".to_string(), candidates),
        ]
        .into_iter()
        .collect(),
    );

    let semantic_ledger = load_project_memory_ledger(&mut store, project_id)
        .expect("semantic memory projection should load");
    assert!(semantic_ledger.records.is_empty());
}

#[test]
fn legacy_v4_read_model_rebuilds_without_unverified_requirements() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_id = "project-memory-v4-trust-rebuild";
    let session_id = "session-v4-unverified";
    let durable_content = "Always preserve explicit deletion confirmation";
    append_durable_requirement_run(
        &mut store,
        project_id,
        session_id,
        "run-v4-durable",
        durable_content,
    );
    append_durable_requirement_run(
        &mut store,
        project_id,
        session_id,
        "run-v4-task-local",
        "For this task, do not modify files",
    );
    let mut legacy = load_project_memory_ledger(&mut store, project_id)
        .expect("current ledger should seed from authoritative events");
    assert_eq!(legacy.records.len(), 1);
    legacy.schema = "cindx.memory-ledger.v4".to_string();
    legacy.records[0].user_requirement_evidence.clear();
    save_project_memory_ledger(&mut store, &legacy).expect("legacy read model should seed");

    let rebuilt = load_project_memory_ledger_inner(&mut store, project_id)
        .expect("legacy read model should rebuild safely");
    assert!(rebuilt.needs_persist);
    assert_eq!(rebuilt.ledger.schema, MEMORY_LEDGER_SCHEMA);
    assert_eq!(rebuilt.ledger.records.len(), 1);
    assert_eq!(rebuilt.ledger.records[0].content, durable_content);
    assert!(rebuilt.ledger.records[0].has_verified_user_requirement());
    assert!(
        persist_project_memory_snapshot_if_current(&mut store, &rebuilt.ledger)
            .expect("migrated ledger should persist")
    );

    let cached = load_project_memory_ledger_inner(&mut store, project_id)
        .expect("migrated read model should load from cache");
    assert!(!cached.needs_persist);
    assert_eq!(cached.ledger.records.len(), 1);
}

#[test]
fn cached_requirement_sources_cannot_cross_projects_or_trust_kinds() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_durable_requirement_run(
        &mut store,
        "project-source-a",
        "session-source-a",
        "run-source-a",
        "Always keep project A exports local",
    );
    append_durable_requirement_run(
        &mut store,
        "project-source-b",
        "session-source-b",
        "run-source-b",
        "Always keep project B exports encrypted",
    );
    let source = load_project_memory_ledger(&mut store, "project-source-a")
        .expect("source ledger should load");
    let expected = load_project_memory_ledger(&mut store, "project-source-b")
        .expect("target ledger should load");

    let mut copied = expected.clone();
    copied.records = source.records.clone();
    save_project_memory_ledger(&mut store, &copied).expect("copied cache should seed");
    let rebuilt = load_project_memory_ledger(&mut store, "project-source-b")
        .expect("cross-project cache should rebuild");
    assert_eq!(rebuilt.records.len(), 1);
    assert_eq!(
        rebuilt.records[0].content,
        "Always keep project B exports encrypted"
    );

    let mut invalid_kind = rebuilt.clone();
    invalid_kind.records[0].kind = MemoryKind::Evidence;
    save_project_memory_ledger(&mut store, &invalid_kind).expect("invalid cache should seed");
    let repaired = load_project_memory_ledger(&mut store, "project-source-b")
        .expect("invalid kind/trust cache should rebuild");
    assert_eq!(repaired.records.len(), 1);
    assert_eq!(repaired.records[0].kind, MemoryKind::Requirement);
    assert_eq!(
        repaired.records[0].trust,
        agent_memory::MemoryTrust::UserStated
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
