use super::test_support::temp_test_root;
use super::*;

#[test]
fn project_session_state_defaults_to_active_workspace() {
    let root = temp_test_root("phase20-projects");
    let config = ProjectSessionConfig::default_for_root(&root);
    let session_id = config.active_session_id.clone();
    let state = project_session_state(&config, None);

    assert_eq!(state.projects.len(), 1);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.projects[0].root, root.display().to_string());
    assert!(state.projects[0].active);
    assert!(state.sessions[0].active);
    assert_eq!(state.sessions[0].effort, "default");
    assert_eq!(state.active_project_id, "project-cindx");
    assert_eq!(state.active_session_id, session_id);
    assert!(state.active_session_id.starts_with("sess_"));
}

#[test]
fn new_session_ids_are_unique_uuid_v7_values() {
    let first = new_session_id();
    let second = new_session_id();

    assert_ne!(first, second);
    for session_id in [first, second] {
        let value = session_id
            .strip_prefix("sess_")
            .expect("session id should use the opaque prefix");
        let uuid = uuid::Uuid::parse_str(value).expect("session id should contain a UUID");
        assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
    }
}

#[test]
fn session_effort_updates_only_the_selected_session() {
    let root = temp_test_root("session-effort");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.sessions.push(SessionRecord {
        id: "session-second".to_string(),
        project_id: config.projects[0].id.clone(),
        name: "Second".to_string(),
        detail: "timeline + chat".to_string(),
        effort: default_agent_effort(),

        agent_model: String::new(),        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    assert!(update_session_effort(
        &mut config,
        &initial_session_id,
        "pro"
    ));
    assert_eq!(config.sessions[0].effort, "pro");
    assert_eq!(config.sessions[1].effort, "default");
    assert!(!update_session_effort(&mut config, "missing", "high"));
}

#[test]
fn deleting_a_project_removes_its_sessions_and_selects_a_neighbor() {
    let root = temp_test_root("delete-project");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.projects.push(ProjectRecord {
        id: "project-next".to_string(),
        name: "Next".to_string(),
        root: root.join("next").display().to_string(),
        detail: "workspace project".to_string(),
        created_at_ms: 2,
        updated_at_ms: 2,
    });
    config.sessions.push(SessionRecord {
        id: "session-next".to_string(),
        project_id: "project-next".to_string(),
        name: "Next Session".to_string(),
        detail: "timeline + chat".to_string(),
        effort: "pro".to_string(),

        agent_model: String::new(),        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    let (_, deleted_session_ids) = remove_project_from_config(&mut config, "project-cindx")
        .expect("project should be removed");

    assert_eq!(deleted_session_ids, vec![initial_session_id]);
    assert_eq!(config.projects.len(), 1);
    assert_eq!(config.sessions.len(), 1);
    assert_eq!(config.active_project_id, "project-next");
    assert_eq!(config.active_session_id, "session-next");
    assert_eq!(config.sessions[0].effort, "pro");
}

#[test]
fn deleting_the_last_project_leaves_an_empty_workspace() {
    let root = temp_test_root("delete-last-project");
    let mut config = ProjectSessionConfig::default_for_root(&root);

    remove_project_from_config(&mut config, "project-cindx").expect("project should be removed");

    assert!(config.projects.is_empty());
    assert!(config.sessions.is_empty());
    assert!(config.active_project_id.is_empty());
    assert!(config.active_session_id.is_empty());
}

#[test]
fn archived_active_session_gets_a_visible_replacement_and_can_be_listed() {
    let root = temp_test_root("archived-session");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    config.sessions[0].archived_at_ms = Some(42);
    config.ensure_consistent(&root);

    let state = project_session_state(&config, None);
    let archived = state
        .sessions
        .iter()
        .find(|session| session.archived)
        .expect("archived session should remain recoverable");
    let active = state
        .sessions
        .iter()
        .find(|session| session.active)
        .expect("replacement session should be active");

    assert_eq!(archived.archived_at_ms, Some(42));
    assert_ne!(archived.id, active.id);
    assert!(!active.archived);
}

#[test]
fn session_activity_acknowledgement_stops_at_the_last_visible_sequence() {
    assert_eq!(acknowledged_event_sequence(12, None), 12);
    assert_eq!(acknowledged_event_sequence(12, Some(8)), 8);
    assert_eq!(acknowledged_event_sequence(12, Some(20)), 12);
}

#[test]
fn archived_session_restore_does_not_revive_seen_activity() {
    let root = temp_test_root("archived-session-seen-activity");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let session_id = config.sessions[0].id.clone();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let metadata = [("session_id".to_string(), session_id.clone())]
        .into_iter()
        .collect();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata,
    )
    .expect("completion should append");
    let latest_sequence = load_agent_session_read_model_snapshot(&store, &session_id)
        .expect("session model should load")
        .revision;

    config.sessions[0].seen_event_sequence = latest_sequence;
    config.sessions[0].archived_at_ms = Some(42);
    let archived = project_session_state_from_store(&config, &store, None)
        .expect("archived state should project");
    assert_eq!(archived.sessions[0].activity, "idle");
    assert!(!archived.sessions[0].unseen_result);

    config.sessions[0].archived_at_ms = None;
    let restored = project_session_state_from_store(&config, &store, None)
        .expect("restored state should project");
    assert_eq!(restored.sessions[0].activity, "idle");
    assert!(!restored.sessions[0].unseen_result);
}

#[test]
fn fork_names_are_unique_within_a_project() {
    let root = temp_test_root("fork-name");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let source = config.sessions[0].clone();
    assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork");
    config.sessions.push(SessionRecord {
        id: "fork-one".to_string(),
        project_id: source.project_id.clone(),
        name: "Runtime Session Fork".to_string(),
        detail: "fork".to_string(),
        effort: default_agent_effort(),

        agent_model: String::new(),        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 1,
        updated_at_ms: 1,
        archived_at_ms: None,
    });

    assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork 2");
}
