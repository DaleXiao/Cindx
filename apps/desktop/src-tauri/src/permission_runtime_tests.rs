use super::*;

#[test]
fn phase3_mock_permission_round_trips_through_store() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let state = request_mock_permission_in_store(&mut store).expect("request should save");

    assert_eq!(state.permissions.len(), 1);
    assert_eq!(state.permissions[0].status, "pending");

    let request_id = state.permissions[0].id.clone();
    let resolved = resolve_permission_in_store(&mut store, &request_id, "deny")
        .expect("resolution should save");

    assert_eq!(resolved.permissions.len(), 1);
    assert_eq!(resolved.permissions[0].status, "resolved");
    assert_eq!(resolved.permissions[0].decision.as_deref(), Some("deny"));
    assert!(resolved
        .timeline
        .iter()
        .any(|entry| entry.label == "Permission resolved"));
}

#[test]
fn pending_review_state_identifies_the_related_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_sessions = ProjectSessionConfig::default_for_root(&workspace_root());
    let session = project_sessions
        .active_session()
        .expect("default session should exist");
    let project = project_sessions
        .active_project()
        .expect("default project should exist");
    let request = PermissionRequest {
        id: PermissionRequestId("agent-review-1".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "Run project checks.".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("phase".to_string(), "16".to_string()),
            ("session_id".to_string(), session.id.clone()),
            ("session_name".to_string(), session.name.clone()),
            ("project_id".to_string(), project.id.clone()),
            ("project_name".to_string(), project.name.clone()),
            ("tool_input".to_string(), "command=cargo test".to_string()),
            ("session_reusable".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    store
        .save_permission_request(request, current_time_millis())
        .expect("request should save");

    let state =
        permission_review_state(&store, &project_sessions).expect("review state should load");

    assert_eq!(state.pending.len(), 1);
    assert_eq!(state.pending[0].source, "agent");
    assert_eq!(
        state.pending[0].session_id.as_deref(),
        Some(session.id.as_str())
    );
    assert_eq!(
        state.pending[0].session_name.as_deref(),
        Some(session.name.as_str())
    );
    assert!(state.pending[0].can_allow_session);
    assert!(state.pending[0].input.contains("cargo test"));
}

#[test]
fn session_permission_grant_only_covers_the_same_capability() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let granted = PermissionRequest {
        id: PermissionRequestId("session-grant".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "run a command".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
            ("command".to_string(), "cargo test".to_string()),
            ("session_reusable".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    store
        .save_permission_request(granted.clone(), 1)
        .expect("grant request should save");
    store
        .resolve_permission(PermissionResolution {
            request_id: granted.id.clone(),
            decision: PermissionDecision::AllowForSession,
            resolved_at_ms: 2,
            resolved_by: "local-user".to_string(),
        })
        .expect("grant should resolve");

    let mut next = granted.clone();
    next.id = PermissionRequestId("next-request".to_string());
    assert!(
        agent_session_permission_granted(&store, &phase16_task_id(), &next, Some("session-a"),)
            .expect("matching grant should load")
    );
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-b"),
    )
    .expect("other session should load"));
    next.action = "file.write".to_string();
    next.scope = "crates/tools".to_string();
    next.risk = PermissionRisk::Write;
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("execute grant must not cover a write request"));
    next.action = "shell.run".to_string();
    next.risk = PermissionRisk::Execute;
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("shell grants must remain bound to their working directory"));
    next.scope = ".".to_string();
    assert!(
        agent_session_permission_granted(&store, &phase16_task_id(), &next, Some("session-a"),)
            .expect("the exact shell capability should reuse the session grant")
    );
    next.metadata
        .insert("command".to_string(), "cargo build".to_string());
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("another shell command should not reuse the grant"));
    next.metadata.remove("command");
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("legacy shell grants without a command should fail closed"));
    next.metadata
        .insert("command".to_string(), "cargo test".to_string());
    next.risk = PermissionRisk::Destructive;
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("destructive grant should not persist"));
}

#[test]
fn pending_permissions_are_isolated_by_agent_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (id, run_id) in [("pending-old", "run-old"), ("pending-new", "run-new")] {
        store
            .save_permission_request(
                PermissionRequest {
                    id: PermissionRequestId(id.to_string()),
                    task_id: phase16_task_id(),
                    risk: PermissionRisk::Execute,
                    action: "shell.run".to_string(),
                    reason: "run a command".to_string(),
                    scope: ".".to_string(),
                    metadata: [
                        ("session_id".to_string(), "session-a".to_string()),
                        ("agent_run_id".to_string(), run_id.to_string()),
                    ]
                    .into_iter()
                    .collect(),
                },
                if run_id == "run-old" { 1 } else { 2 },
            )
            .expect("pending request should save");
    }

    let pending = pending_agent_permissions_for_run(
        &store,
        &phase16_task_id(),
        Some("session-a"),
        Some("run-new"),
    )
    .expect("pending requests should load");

    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id.0, "pending-new");
}
