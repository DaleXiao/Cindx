use super::*;
use crate::{EventId, EventKind, TaskId};

fn event(
    event_id: &str,
    task_id: &str,
    attempt_run_id: &str,
    project_id: &str,
    session_id: &str,
    source_attempt_run_id: Option<&str>,
    explicit_logical_run_id: Option<&str>,
) -> Event {
    let mut metadata = [
        ("project_id".to_string(), project_id.to_string()),
        ("session_id".to_string(), session_id.to_string()),
        (
            AGENT_RUN_ID_METADATA_KEY.to_string(),
            attempt_run_id.to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(source_attempt_run_id) = source_attempt_run_id {
        metadata.insert(
            SOURCE_AGENT_RUN_ID_METADATA_KEY.to_string(),
            source_attempt_run_id.to_string(),
        );
    }
    if let Some(explicit_logical_run_id) = explicit_logical_run_id {
        let identity = match source_attempt_run_id {
            Some(source_attempt_run_id) => AgentRunIdentity::continuation(
                explicit_logical_run_id,
                attempt_run_id,
                source_attempt_run_id,
            ),
            None => AgentRunIdentity::new(explicit_logical_run_id, attempt_run_id),
        };
        identity
            .expect("identity should build")
            .insert_into(&mut metadata)
            .expect("identity should insert");
    }
    Event {
        id: EventId(event_id.to_string()),
        task_id: TaskId(task_id.to_string()),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "run event".to_string(),
        metadata,
    }
}

#[test]
fn versioned_identity_inserts_existing_physical_and_logical_ids_without_attempt_alias() {
    let identity = AgentRunIdentity::continuation("run-a", "attempt-b", "attempt-a")
        .expect("identity should build");
    let mut metadata = Metadata::new();
    identity
        .insert_into(&mut metadata)
        .expect("identity should insert");

    assert_eq!(agent_run_id(&metadata), Some("attempt-b"));
    assert_eq!(logical_agent_run_id(&metadata), Some("run-a"));
    assert_eq!(source_agent_run_id(&metadata), Some("attempt-a"));
    assert_eq!(
        metadata
            .get(AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY)
            .map(String::as_str),
        Some(AGENT_RUN_IDENTITY_V1_SCHEMA)
    );
    assert!(!metadata.contains_key("attempt_id"));
    assert_eq!(
        AgentRunIdentity::from_metadata(&metadata).expect("identity should decode"),
        Some(identity)
    );
}

#[test]
fn identity_insertion_rejects_reserved_metadata_conflicts() {
    let mut metadata: Metadata = [(
        AGENT_RUN_ID_METADATA_KEY.to_string(),
        "other-attempt".to_string(),
    )]
    .into_iter()
    .collect();
    let before = metadata.clone();
    assert_eq!(
        AgentRunIdentity::new("logical", "attempt")
            .unwrap()
            .insert_into(&mut metadata),
        Err(AgentRunIdentityError::ReservedMetadataConflict(
            AGENT_RUN_ID_METADATA_KEY
        ))
    );
    assert_eq!(metadata, before);
}

#[test]
fn legacy_lineage_resolves_three_attempts_to_one_logical_run() {
    let events = vec![
        event("a", "task", "A", "project", "session", None, None),
        event("b", "task", "B", "project", "session", Some("A"), None),
        event("c", "task", "C", "project", "session", Some("B"), None),
    ];
    let lineage = AgentRunLineage::from_events(&events).expect("lineage should resolve");

    assert_eq!(lineage.logical_run_id_for_attempt("A"), Some("A"));
    assert_eq!(lineage.logical_run_id_for_attempt("B"), Some("A"));
    assert_eq!(lineage.logical_run_id_for_attempt("C"), Some("A"));
    assert_eq!(
        lineage
            .logical_run_id_for_event(&events[2])
            .expect("event should resolve"),
        Some("A")
    );
}

#[test]
fn legacy_same_attempt_recovery_does_not_replace_the_continuation_predecessor() {
    let events = vec![
        event("a", "task", "A", "project", "session", None, None),
        event(
            "b-start",
            "task",
            "B",
            "project",
            "session",
            Some("A"),
            None,
        ),
        event(
            "b-recovered",
            "task",
            "B",
            "project",
            "session",
            Some("B"),
            None,
        ),
    ];
    let lineage = AgentRunLineage::from_events(&events).expect("lineage should resolve");

    assert_eq!(lineage.logical_run_id_for_attempt("A"), Some("A"));
    assert_eq!(lineage.logical_run_id_for_attempt("B"), Some("A"));
}

#[test]
fn legacy_same_attempt_recovery_without_a_predecessor_stays_its_own_root() {
    let events = vec![event(
        "recovered",
        "task",
        "B",
        "project",
        "session",
        Some("B"),
        None,
    )];
    let lineage = AgentRunLineage::from_events(&events).expect("lineage should resolve");

    assert_eq!(lineage.logical_run_id_for_attempt("B"), Some("B"));
}

#[test]
fn legacy_different_non_self_predecessors_still_fail_closed() {
    let events = vec![
        event("a", "task", "A", "project", "session", None, None),
        event("c", "task", "C", "project", "session", None, None),
        event(
            "b-from-a",
            "task",
            "B",
            "project",
            "session",
            Some("A"),
            None,
        ),
        event(
            "b-from-c",
            "task",
            "B",
            "project",
            "session",
            Some("C"),
            None,
        ),
    ];

    assert_eq!(
        AgentRunLineage::from_events(&events),
        Err(AgentRunIdentityError::ConflictingSourceAttempt(
            "B".to_string()
        ))
    );
}

#[test]
fn explicit_logical_identity_survives_a_partial_event_window() {
    let event = event(
        "continued",
        "task",
        "B",
        "project",
        "session",
        Some("A"),
        Some("logical-A"),
    );
    let lineage = AgentRunLineage::from_events(&[event]).expect("explicit identity is enough");
    assert_eq!(lineage.logical_run_id_for_attempt("B"), Some("logical-A"));
}

#[test]
fn legacy_lineage_fails_closed_on_missing_source_cycle_and_cross_scope() {
    let missing = event(
        "missing",
        "task",
        "B",
        "project",
        "session",
        Some("A"),
        None,
    );
    assert!(matches!(
        AgentRunLineage::from_events(&[missing]),
        Err(AgentRunIdentityError::MissingSourceAttempt { .. })
    ));

    let cycle = [
        event("a", "task", "A", "project", "session", Some("B"), None),
        event("b", "task", "B", "project", "session", Some("A"), None),
    ];
    assert!(matches!(
        AgentRunLineage::from_events(&cycle),
        Err(AgentRunIdentityError::LineageCycle(_))
    ));

    let cross_scope = [
        event("a", "task", "A", "project", "first", None, None),
        event("b", "task", "B", "project", "second", Some("A"), None),
    ];
    assert!(matches!(
        AgentRunLineage::from_events(&cross_scope),
        Err(AgentRunIdentityError::SourceCrossesScope { .. })
    ));
}

#[test]
fn explicit_identity_is_not_reinferred_from_legacy_source_metadata() {
    let events = [
        event("a", "task", "A", "project", "session", None, None),
        event(
            "b",
            "task",
            "B",
            "project",
            "session",
            Some("A"),
            Some("different-logical-run"),
        ),
    ];
    let lineage = AgentRunLineage::from_events(&events)
        .expect("versioned logical identity must be authoritative");
    assert_eq!(
        lineage.logical_run_id_for_attempt("B"),
        Some("different-logical-run")
    );
}

#[test]
fn explicit_logical_identity_cannot_cross_run_scope() {
    let events = [
        event(
            "session-a",
            "task",
            "attempt-a",
            "project",
            "session-a",
            None,
            Some("logical-run"),
        ),
        event(
            "session-b",
            "task",
            "attempt-b",
            "project",
            "session-b",
            None,
            Some("logical-run"),
        ),
    ];
    assert_eq!(
        AgentRunLineage::from_events(&events),
        Err(AgentRunIdentityError::LogicalRunCrossesScope(
            "logical-run".to_string()
        ))
    );
}

#[test]
fn agent_run_lineage_contract() {
    let initial = event(
        "initial",
        "task",
        "A",
        "project",
        "session",
        None,
        Some("A"),
    );
    let mut steered = initial.clone();
    steered.id = EventId("steered-same-attempt".to_string());
    steered
        .metadata
        .insert("steer_epoch".to_string(), "1".to_string());
    steered.metadata.insert(
        SOURCE_AGENT_RUN_ID_METADATA_KEY.to_string(),
        "A".to_string(),
    );
    let explicit = AgentRunLineage::from_events(&[initial, steered])
        .expect("same-attempt steer metadata must keep one logical run");
    assert_eq!(explicit.logical_run_id_for_attempt("A"), Some("A"));

    let legacy = [
        event("legacy-a", "task", "A", "project", "session", None, None),
        event(
            "legacy-b",
            "task",
            "B",
            "project",
            "session",
            Some("A"),
            None,
        ),
        event(
            "legacy-c",
            "task",
            "C",
            "project",
            "session",
            Some("B"),
            None,
        ),
    ];
    let legacy = AgentRunLineage::from_events(&legacy)
        .expect("A <- B <- C must resolve in one legacy scope");
    assert_eq!(legacy.logical_run_id_for_attempt("C"), Some("A"));

    let cycle = [
        event(
            "cycle-a",
            "task",
            "A",
            "project",
            "session",
            Some("B"),
            None,
        ),
        event(
            "cycle-b",
            "task",
            "B",
            "project",
            "session",
            Some("A"),
            None,
        ),
    ];
    assert!(matches!(
        AgentRunLineage::from_events(&cycle),
        Err(AgentRunIdentityError::LineageCycle(_))
    ));

    let cross_scope = [
        event("scope-a", "task", "A", "project", "session-a", None, None),
        event(
            "scope-b",
            "task",
            "B",
            "project",
            "session-b",
            Some("A"),
            None,
        ),
    ];
    assert!(matches!(
        AgentRunLineage::from_events(&cross_scope),
        Err(AgentRunIdentityError::SourceCrossesScope { .. })
    ));

    println!("{AGENT_RUN_IDENTITY_V1_SCHEMA}");
}
