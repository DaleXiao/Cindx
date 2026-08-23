use super::*;
use tools::encode_input;

#[test]
fn agent_state_reports_context_usage_and_hides_internal_drafts() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("prompt".to_string(), "inspect".to_string()),
            ("context_window_tokens".to_string(), "100000".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        [("prompt_tokens".to_string(), "25000".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("usage should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "internal draft",
        [("internal".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("draft should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "final answer",
    )
    .expect("final should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.context_tokens_used, 25_000);
    assert_eq!(state.context_window_tokens, 100_000);
    assert_eq!(state.context_remaining_percent, 75.0);
    assert!(!state.context_usage_estimated);
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[1].content, "final answer");
}

#[test]
fn phase4_state_includes_provider_config_and_messages() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let config = ProviderConfig {
        api_key: "secret".to_string(),
        ..ProviderConfig::default()
    };
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::User,
        "hello model",
    )
    .expect("message should append");

    let state = phase4_state(&mut store, &config, None).expect("state should load");

    assert!(state.provider.ready);
    assert!(state.provider.api_key_set);
    assert!(!provider_config_state(&ProviderConfig::default()).ready);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].content, "hello model");
}

#[test]
fn phase5_state_lists_tool_results() {
    let store = Mutex::new(SqliteStore::in_memory().expect("store should open"));
    let execution_gate = Mutex::new(());
    let root = workspace_root();
    let registry = ToolRegistry::with_workspace_tools(root.clone());
    execute_manual_tool_invocation(
        &execution_gate,
        &store,
        &registry,
        ToolInvocation {
            id: agent_core::ToolCallId("tool-1".to_string()),
            task_id: phase5_task_id(),
            tool_name: "file.list".to_string(),
            input_json: encode_input(&[("path", ".")]),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        },
        &root,
        None,
    )
    .expect("tool should execute");

    let store = store.lock().expect("store should lock");
    let state = phase5_state(&store, None, &root).expect("state should load");

    assert!(state.tools.iter().any(|tool| tool.name == "file.write"));
    assert_eq!(state.results.len(), 1);
    assert_eq!(state.results[0].tool_name, "file.list");
}

#[test]
fn phase5_state_lists_pending_tool_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let root = workspace_root();
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("tool-2".to_string()),
        task_id: phase5_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", ".cindx/phase5-test.txt"), ("content", "ok")]),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(root.clone());
    let mut request = registry
        .get("file.write")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("write should request permission");
    request.id = PermissionRequestId("perm-phase5".to_string());
    request
        .metadata
        .insert("phase".to_string(), "5".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    store
        .save_permission_request(request, 123)
        .expect("request should save");

    let state = phase5_state(&store, None, &root).expect("state should load");

    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "file.write");
}

#[test]
fn phase8_state_lists_browser_observations() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase8_task_id(),
        EventKind::ToolCallFinished,
        "Browser text extracted",
        [
            ("tool_call_id".to_string(), "browser-1".to_string()),
            ("tool".to_string(), "browser.extract_text".to_string()),
            ("status".to_string(), "succeeded".to_string()),
            ("output".to_string(), "Example Domain".to_string()),
            ("result_url".to_string(), "https://example.com".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase8_state(&store, None).expect("state should load");

    assert_eq!(state.observations.len(), 1);
    assert_eq!(state.observations[0].tool_name, "browser.extract_text");
    assert_eq!(
        state.observations[0].url.as_deref(),
        Some("https://example.com")
    );
}

#[test]
fn phase8_state_lists_pending_browser_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("browser-2".to_string()),
        task_id: phase8_task_id(),
        tool_name: "browser.capture".to_string(),
        input_json: encode_input(&[("url", "https://example.com")]),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("browser.capture")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("browser capture should request permission");
    request.id = PermissionRequestId("perm-phase8".to_string());
    request
        .metadata
        .insert("phase".to_string(), "8".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    store
        .save_permission_request(request, 456)
        .expect("request should save");

    let state = phase8_state(&store, None).expect("state should load");

    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "browser.capture");
}

#[test]
fn agent_state_lists_pending_agent_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "write a file".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("agent-tool-1".to_string()),
        task_id: phase16_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", ".cindx/agent-loop.txt"), ("content", "ok")]),
        proposed_by_model: "agent-loop".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("file.write")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("write should request permission");
    request.id = PermissionRequestId("perm-agent".to_string());
    request
        .metadata
        .insert("phase".to_string(), "16".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    request
        .metadata
        .insert("agent_prompt".to_string(), "write a file".to_string());
    store
        .save_permission_request(request, current_time_millis())
        .expect("request should save");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "waiting_for_permission");
    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "file.write");

    store
        .resolve_permission(PermissionResolution {
            request_id: PermissionRequestId("perm-agent".to_string()),
            decision: PermissionDecision::AllowOnce,
            resolved_at_ms: current_time_millis(),
            resolved_by: "local-user".to_string(),
        })
        .expect("permission should resolve");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task resumed after permission",
        Metadata::new(),
    )
    .expect("resume should append");

    let resumed = agent_state(&store, None).expect("resumed state should load");
    assert_eq!(resumed.status, "running");
    assert!(resumed.pending_approvals.is_empty());
}

#[test]
fn agent_state_ignores_old_pending_approvals_after_new_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let mut request = PermissionRequest {
        id: PermissionRequestId("old-agent-perm".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Write,
        action: "file.write".to_string(),
        reason: "old run".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("tool_call_id".to_string(), "old-call".to_string()),
            ("tool_name".to_string(), "file.write".to_string()),
            ("tool_input".to_string(), "path=old.txt".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    request
        .metadata
        .insert("phase".to_string(), "16".to_string());
    store
        .save_permission_request(request, 1)
        .expect("old request should save");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "new task".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "new task",
    )
    .expect("user message should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "running");
    assert!(state.pending_approvals.is_empty());
    assert_eq!(state.transcript_messages, 1);
}

#[test]
fn agent_state_reports_cancelled_and_retryable() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "inspect".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        Metadata::new(),
    )
    .expect("cancel should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "cancelled");
    assert!(state.can_retry);
    assert!(!state.can_cancel);
}

#[test]
fn agent_state_and_trace_expose_project_session_context() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("prompt".to_string(), "inspect".to_string()),
            ("project_id".to_string(), "project-alpha".to_string()),
            ("project_name".to_string(), "Alpha".to_string()),
            ("session_id".to_string(), "session-alpha".to_string()),
            ("session_name".to_string(), "Alpha Session".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");

    let state = agent_state(&store, None).expect("state should load");
    let trace =
        agent_trace_state_for_session(&store, None, None, None).expect("trace should build");

    assert_eq!(state.project_id.as_deref(), Some("project-alpha"));
    assert_eq!(state.session_name.as_deref(), Some("Alpha Session"));
    assert_eq!(trace.project_name.as_deref(), Some("Alpha"));
    assert_eq!(trace.session_id.as_deref(), Some("session-alpha"));
}
