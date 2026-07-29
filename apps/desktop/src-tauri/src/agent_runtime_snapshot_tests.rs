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
use agent_core::{EventKind, MessageRole};
use agent_runtime::{start_agent_loop, AgentRunControl, AgentRuntimeConfig, RunStageClass};

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
