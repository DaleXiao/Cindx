use super::*;
use agent_core::{AgentRunIdentity, EventId, TaskId};
use agent_storage::SqliteStore;
use agent_core::{TaskClass, WorkspaceRetrievalChannel};

fn run_context(decision: AgentRunDecision) -> Metadata {
    [(
        "run_decision".to_string(),
        serde_json::to_string(&decision).expect("decision should serialize"),
    )]
    .into_iter()
    .collect()
}

fn successful_tool(tool: &str, path: &str) -> Event {
    Event {
        id: EventId("tool-event".to_string()),
        task_id: TaskId("task".to_string()),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::ToolCallFinished,
        summary: "Tool finished".to_string(),
        metadata: [
            ("tool".to_string(), tool.to_string()),
            ("status".to_string(), "succeeded".to_string()),
            ("result_path".to_string(), path.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

fn append_test_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    sequence: u64,
    summary: &str,
    metadata: Metadata,
) {
    store
        .append_next_event(
            EventId(format!("semantic-lineage-{sequence}")),
            task_id.clone(),
            sequence,
            EventKind::TaskStatusChanged,
            summary.to_string(),
            metadata,
        )
        .expect("semantic lineage event should append");
}

fn scoped_identity_metadata(
    logical_run_id: &str,
    attempt_run_id: &str,
    source_attempt_run_id: Option<&str>,
) -> Metadata {
    let mut metadata = [
        ("project_id".to_string(), "project-lineage".to_string()),
        ("session_id".to_string(), "session-lineage".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect();
    let identity = match source_attempt_run_id {
        Some(source_attempt_run_id) => {
            AgentRunIdentity::continuation(logical_run_id, attempt_run_id, source_attempt_run_id)
        }
        None => AgentRunIdentity::new(logical_run_id, attempt_run_id),
    }
    .expect("test identity should be valid");
    identity
        .insert_into(&mut metadata)
        .expect("test identity should attach");
    metadata
}

#[test]
fn direct_general_text_run_skips_model_curation() {
    let context = run_context(AgentRunDecision::direct("model"));

    assert!(!semantic_memory_model_is_warranted(&context, &[]));
    assert!(!semantic_memory_model_is_warranted(&Metadata::new(), &[]));
}

#[test]
fn workflow_or_retrieval_run_admits_model_curation() {
    let mut research = AgentRunDecision::direct("model");
    research.task_class = TaskClass::Research;
    assert!(!semantic_memory_model_is_warranted(
        &run_context(research.clone()),
        &[]
    ));
    research.execution = AgentExecutionMode::Workflow;
    assert!(semantic_memory_model_is_warranted(
        &run_context(research),
        &[]
    ));

    let mut retrieval = AgentRunDecision::direct("model");
    retrieval.retrieval.query = "project decision".to_string();
    retrieval
        .retrieval
        .channels
        .insert(WorkspaceRetrievalChannel::FileSearch);
    assert!(semantic_memory_model_is_warranted(
        &run_context(retrieval),
        &[]
    ));
}

#[test]
fn durable_effect_admits_curation_but_transient_read_does_not() {
    let context = run_context(AgentRunDecision::direct("model"));
    let mut planned_effect = AgentRunDecision::direct("model");
    planned_effect.tool_requirement = AgentToolRequirement::Effects;

    assert!(semantic_memory_model_is_warranted(
        &run_context(planned_effect),
        &[]
    ));
    assert!(semantic_memory_model_is_warranted(
        &context,
        &[successful_tool("file.write", "src/lib.rs")]
    ));
    assert!(!semantic_memory_model_is_warranted(
        &context,
        &[successful_tool("shell.run", "README.md")]
    ));
}

#[test]
fn semantic_memory_loads_every_attempt_in_one_logical_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let task_id = TaskId("semantic-logical-run".to_string());
    let initial = scoped_identity_metadata("logical-run", "attempt-a", None);
    let continuation = scoped_identity_metadata("logical-run", "attempt-b", Some("attempt-a"));
    append_test_event(&mut store, &task_id, 1, "Agent task started", initial);
    append_test_event(
        &mut store,
        &task_id,
        2,
        "Agent task completed",
        continuation.clone(),
    );

    let events = semantic_memory_events_for_run(&store, &task_id, &continuation)
        .expect("logical semantic events should load");
    assert_eq!(events.len(), 2);
    assert_eq!(
        events
            .iter()
            .filter_map(|event| event.metadata.get("agent_run_id"))
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["attempt-a", "attempt-b"]
    );
}

#[test]
fn semantic_memory_preserves_legacy_multihop_recovery_lineage() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let task_id = TaskId("semantic-legacy-lineage".to_string());
    let metadata = |attempt_run_id: &str, source_attempt_run_id: Option<&str>| {
        let mut metadata = [
            ("project_id".to_string(), "project-legacy".to_string()),
            ("session_id".to_string(), "session-legacy".to_string()),
            ("agent_run_id".to_string(), attempt_run_id.to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        if let Some(source_attempt_run_id) = source_attempt_run_id {
            metadata.insert(
                "source_agent_run_id".to_string(),
                source_attempt_run_id.to_string(),
            );
        }
        metadata
    };
    append_test_event(
        &mut store,
        &task_id,
        1,
        "Agent task started",
        metadata("attempt-a", None),
    );
    append_test_event(
        &mut store,
        &task_id,
        2,
        "Agent task retry started",
        metadata("attempt-b", Some("attempt-a")),
    );
    let current = metadata("attempt-c", Some("attempt-b"));
    append_test_event(
        &mut store,
        &task_id,
        3,
        "Agent task completed",
        current.clone(),
    );

    let events = semantic_memory_events_for_run(&store, &task_id, &current)
        .expect("legacy semantic events should load");
    assert_eq!(events.len(), 3);
}

#[test]
fn semantic_memory_physical_fallback_stays_in_run_scope() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let task_id = TaskId("semantic-physical-scope".to_string());
    let mut current = [
        ("project_id".to_string(), "project-scope-a".to_string()),
        ("session_id".to_string(), "session-scope-a".to_string()),
        (
            "agent_run_id".to_string(),
            "attempt-scope-collision".to_string(),
        ),
        (
            "source_agent_run_id".to_string(),
            "missing-source".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_test_event(
        &mut store,
        &task_id,
        1,
        "Agent task completed",
        current.clone(),
    );
    current.insert("session_id".to_string(), "session-scope-b".to_string());
    current.insert("project_id".to_string(), "project-scope-b".to_string());
    append_test_event(&mut store, &task_id, 2, "Agent task completed", current);
    let run_context = [
        ("project_id".to_string(), "project-scope-a".to_string()),
        ("session_id".to_string(), "session-scope-a".to_string()),
        (
            "agent_run_id".to_string(),
            "attempt-scope-collision".to_string(),
        ),
        (
            "source_agent_run_id".to_string(),
            "missing-source".to_string(),
        ),
    ]
    .into_iter()
    .collect();

    let events = semantic_memory_events_for_run(&store, &task_id, &run_context)
        .expect("physical fallback should load");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].metadata.get("session_id").map(String::as_str),
        Some("session-scope-a")
    );
}

#[test]
fn semantic_memory_bridges_legacy_lineage_into_an_explicit_continuation() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let task_id = TaskId("semantic-hybrid-lineage".to_string());
    let legacy = |attempt_run_id: &str, source_attempt_run_id: Option<&str>| {
        let mut metadata = [
            ("project_id".to_string(), "project-hybrid".to_string()),
            ("session_id".to_string(), "session-hybrid".to_string()),
            ("agent_run_id".to_string(), attempt_run_id.to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        if let Some(source_attempt_run_id) = source_attempt_run_id {
            metadata.insert(
                "source_agent_run_id".to_string(),
                source_attempt_run_id.to_string(),
            );
        }
        metadata
    };
    append_test_event(
        &mut store,
        &task_id,
        1,
        "Agent task started",
        legacy("attempt-a", None),
    );
    append_test_event(
        &mut store,
        &task_id,
        2,
        "Agent task retry started",
        legacy("attempt-b", Some("attempt-a")),
    );
    append_test_event(
        &mut store,
        &task_id,
        3,
        "Agent task retry started",
        legacy("attempt-c", Some("attempt-b")),
    );
    let mut current = [
        ("project_id".to_string(), "project-hybrid".to_string()),
        ("session_id".to_string(), "session-hybrid".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect();
    AgentRunIdentity::continuation("attempt-a", "attempt-d", "attempt-c")
        .expect("hybrid continuation identity should be valid")
        .insert_into(&mut current)
        .expect("hybrid continuation identity should attach");
    append_test_event(
        &mut store,
        &task_id,
        4,
        "Agent task completed",
        current.clone(),
    );

    let events = semantic_memory_events_for_run(&store, &task_id, &current)
        .expect("hybrid semantic events should load");
    assert_eq!(events.len(), 4);
}
