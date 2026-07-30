use super::*;
use crate::{
    agent_read_model::{active_agent_events_for_session, agent_events_for_session},
    agent_recovery_service::agent_recovery_identity,
    agent_resource_snapshot::{
        load_matching_agent_resource_snapshot, persist_agent_resource_snapshot,
    },
    event_persistence::{append_event, append_message_event_with_metadata},
    project_session_persistence::metadata_with_context,
    runtime_constants::AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
};
use agent_core::{EventKind, Message, MessageRole};
use agent_runtime::{
    start_agent_loop, AgentLoopAppendTransaction, AgentRunControl, AgentRuntimeConfig,
    RunEpochLeaseOutcome, RunExecutionStepCommit, RunStageClass, RunSteerBatchCommit,
};
use agent_storage::EventStore;
use std::{
    sync::{mpsc, Arc},
    thread,
};

fn run_context(session_id: &str, run_id: &str) -> Metadata {
    [
        ("session_id".to_string(), session_id.to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
        ("agent_effort".to_string(), "auto".to_string()),
    ]
    .into_iter()
    .collect()
}

#[test]
fn runtime_snapshot_is_overwritten_and_bound_to_the_active_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = run_context("session-a", "run-a");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "inspect workspace".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run start should persist");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "inspect workspace",
        context.clone(),
    )
    .expect("user message should persist");
    let runtime = start_agent_loop(
        phase16_task_id(),
        "inspect workspace".to_string(),
        AgentRuntimeConfig::default(),
    );

    persist_agent_runtime_snapshot(&mut store, &runtime, &context)
        .expect("runtime snapshot should persist");
    let control = AgentRunControl::new("fast");
    let _attempt = control
        .begin_physical_model_attempt("model-a", 12, 8, RunStageClass::Worker)
        .expect("attempt should reserve");
    persist_agent_resource_snapshot(&mut store, &context, &control.resource_usage())
        .expect("resource snapshot should persist");
    let events = agent_events_for_session(&store, &phase16_task_id(), Some("session-a"))
        .expect("events should load");
    let active = active_agent_events_for_session(&events, Some("session-a"));
    let (_, source_run_id, _, prompt_fingerprint, _) =
        agent_recovery_identity(&active, &context).expect("identity should resolve");
    let latest_revision = active
        .last()
        .map(|event| event.sequence)
        .unwrap_or_default();
    assert!(load_matching_agent_runtime_snapshot(
        &store,
        &context,
        &source_run_id,
        &prompt_fingerprint,
        latest_revision,
    )
    .expect("snapshot should load")
    .is_some());
    let resources =
        load_matching_agent_resource_snapshot(&store, &context, &source_run_id, latest_revision)
            .expect("resource snapshot should load")
            .expect("resource snapshot should match");
    assert_eq!(resources.segment.physical_attempts, 1);
    assert_eq!(resources.segment.reserved_tokens, 20);

    let stale_context = run_context("session-a", "run-b");
    assert!(load_matching_agent_runtime_snapshot(
        &store,
        &stale_context,
        "run-b",
        &prompt_fingerprint,
        latest_revision,
    )
    .expect("stale snapshot lookup should succeed")
    .is_none());

    delete_persisted_agent_runtime_snapshot(&mut store, Some("session-a"))
        .expect("snapshot should delete");
    assert!(store
        .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
        .expect("read model lookup should succeed")
        .is_none());
    assert!(store
        .load_read_model(AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
        .expect("resource read model lookup should succeed")
        .is_none());
}

#[test]
fn resource_checkpoint_rejects_an_out_of_order_older_snapshot() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = run_context("session-a", "run-a");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "inspect workspace".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run start should persist");
    let control = AgentRunControl::new("fast");
    let attempt = control
        .begin_physical_model_attempt("model-a", 12, 8, RunStageClass::Worker)
        .expect("resource attempt should reserve");
    let older = control.resource_usage();
    assert!(control.finish_physical_model_attempt(attempt, None));
    let newer = control.resource_usage();
    assert!(newer.mutation_revision() > older.mutation_revision());

    persist_agent_resource_snapshot(&mut store, &context, &newer)
        .expect("newer resource checkpoint should persist");
    persist_agent_resource_snapshot(&mut store, &context, &older)
        .expect("older resource checkpoint should be ignored");

    let restored = load_matching_agent_resource_snapshot(&store, &context, "run-a", u64::MAX)
        .expect("resource checkpoint should load")
        .expect("newer resource checkpoint should remain");
    assert_eq!(restored.mutation_revision(), newer.mutation_revision());
    assert_eq!(restored.segment.reserved_tokens, 0);
    assert_eq!(restored.segment.total_tokens, 20);
}

fn runtime_with_appended_message() -> (
    agent_runtime::AgentLoopState,
    Metadata,
    usize,
    PreparedAgentRuntimeSnapshot,
) {
    let context = run_context("session-a", "run-a");
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "inspect workspace".to_string(),
        AgentRuntimeConfig::default(),
    );
    let cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    let previous_message_count = runtime.messages.len();
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: "inspection complete".to_string(),
        metadata: Metadata::new(),
    });
    let (prepared, _) = cursor.prepare_after_append(&runtime, previous_message_count, &context);
    (runtime, context, previous_message_count, prepared)
}

#[test]
fn runtime_append_and_snapshot_commit_together() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let (runtime, context, previous_message_count, prepared) = runtime_with_appended_message();

    persist_runtime_append_and_snapshot(
        &mut store,
        &runtime,
        previous_message_count,
        &context,
        &prepared,
    )
    .expect("runtime append and checkpoint should persist");

    let events = store
        .list_by_task(&runtime.task_id)
        .expect("runtime events should load");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::MessageAdded);
    let stored = store
        .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
        .expect("runtime checkpoint should load")
        .expect("runtime checkpoint should exist");
    assert_eq!(stored.revision, events[0].sequence);
}

#[test]
fn runtime_append_and_snapshot_roll_back_together_after_sqlite_failure() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let (runtime, context, previous_message_count, prepared) = runtime_with_appended_message();
    store
        .execute_batch_for_testing(
            "
            create trigger fail_agent_runtime_snapshot
            before insert on read_models
            when new.namespace = 'agent-runtime-snapshot-v1'
            begin
              select raise(abort, 'injected runtime snapshot failure');
            end;
            ",
        )
        .expect("failure trigger should install");

    let result = persist_runtime_append_and_snapshot(
        &mut store,
        &runtime,
        previous_message_count,
        &context,
        &prepared,
    );

    assert!(result.is_err());
    assert!(store
        .list_by_task(&runtime.task_id)
        .expect("runtime events should load")
        .is_empty());
    assert!(store
        .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
        .expect("runtime checkpoint lookup should succeed")
        .is_none());

    store
        .execute_batch_for_testing("drop trigger fail_agent_runtime_snapshot;")
        .expect("failure trigger should uninstall");
    persist_runtime_append_and_snapshot(
        &mut store,
        &runtime,
        previous_message_count,
        &context,
        &prepared,
    )
    .expect("retry should commit after the SQLite failure is removed");
    assert_eq!(
        store
            .list_by_task(&runtime.task_id)
            .expect("runtime events should load")
            .len(),
        1
    );
    assert!(store
        .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
        .expect("runtime checkpoint lookup should succeed")
        .is_some());
}

fn append_runtime_message_and_checkpoint(
    store: &mut SqliteStore,
    runtime: &mut agent_runtime::AgentLoopState,
    context: &Metadata,
    cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<(), String> {
    let mut transaction = AgentLoopAppendTransaction::begin(runtime);
    let previous_message_count = transaction.original_message_count();
    transaction.with_append_only_mutation(|runtime| {
        runtime.messages.push(Message {
            role: MessageRole::System,
            content: "reserve terminal delivery".to_string(),
            metadata: Metadata::new(),
        });
    });
    let (prepared, next_cursor) =
        cursor.prepare_after_append(transaction.state(), previous_message_count, context);
    persist_runtime_append_and_snapshot(
        store,
        transaction.state(),
        previous_message_count,
        context,
        &prepared,
    )?;
    transaction.commit();
    *cursor = next_cursor;
    Ok(())
}

#[test]
fn terminal_instruction_commit_is_inert_when_steer_wins_the_epoch() {
    let control = AgentRunControl::new("pro");
    let stale_lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    assert_eq!(control.request_steer("new objective"), Ok(true));
    assert!(matches!(
        control
            .commit_pending_steers_with(|_| Ok::<_, ()>(()))
            .expect("steer should apply"),
        RunSteerBatchCommit::Committed { .. }
    ));
    let context = run_context("session-a", "run-a");
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "inspect workspace".to_string(),
        AgentRuntimeConfig::default(),
    );
    let runtime_before = runtime.clone();
    let mut cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    let cursor_visits_before = cursor.message_visits();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let mut persistence_ran = false;

    let outcome = control
        .commit_execution_step_with(stale_lease, || {
            persistence_ran = true;
            append_runtime_message_and_checkpoint(&mut store, &mut runtime, &context, &mut cursor)
        })
        .expect("terminal instruction arbitration should succeed");

    assert_eq!(outcome, RunExecutionStepCommit::RestartAfterSteer);
    assert!(!persistence_ran);
    assert_eq!(runtime, runtime_before);
    assert_eq!(cursor.message_visits(), cursor_visits_before);
    assert!(store
        .list_by_task(&runtime.task_id)
        .expect("runtime events should load")
        .is_empty());
    assert!(store
        .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
        .expect("runtime checkpoint lookup should succeed")
        .is_none());
}

#[test]
fn terminal_instruction_commit_precedes_a_later_concurrent_steer() {
    let control = Arc::new(AgentRunControl::new("pro"));
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let context = run_context("session-a", "run-a");
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "inspect workspace".to_string(),
        AgentRuntimeConfig::default(),
    );
    let original_message_count = runtime.messages.len();
    let mut cursor = AgentRuntimeSnapshotCursor::rebuild(&runtime, &context);
    let cursor_visits_before = cursor.message_visits();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let (start_steer_tx, start_steer_rx) = mpsc::channel();
    let (attempting_tx, attempting_rx) = mpsc::channel();
    let steer_control = Arc::clone(&control);
    let steer = thread::spawn(move || {
        start_steer_rx.recv().expect("steer should be released");
        attempting_tx.send(()).expect("attempt signal should send");
        steer_control.request_steer("later objective")
    });

    let outcome = control
        .commit_execution_step_with(lease, || {
            start_steer_tx.send(()).expect("steer should start");
            attempting_rx.recv().expect("steer should attempt");
            append_runtime_message_and_checkpoint(&mut store, &mut runtime, &context, &mut cursor)
        })
        .expect("terminal instruction arbitration should succeed");

    assert_eq!(outcome, RunExecutionStepCommit::Committed(()));
    assert_eq!(steer.join().expect("steer should join"), Ok(true));
    assert_eq!(runtime.messages.len(), original_message_count + 1);
    assert_eq!(cursor.message_visits(), cursor_visits_before + 1);
    assert_eq!(
        store
            .list_by_task(&runtime.task_id)
            .expect("runtime events should load")
            .len(),
        1
    );
    assert!(store
        .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
        .expect("runtime checkpoint lookup should succeed")
        .is_some());
    assert_eq!(
        control.execution_epoch_lease(),
        RunEpochLeaseOutcome::RestartAfterSteer
    );
}
