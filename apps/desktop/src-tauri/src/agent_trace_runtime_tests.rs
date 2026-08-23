use super::test_support::temp_test_root;
use super::*;

#[test]
fn agent_transcript_restores_assistant_tool_and_tool_messages() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "read README".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "read README",
    )
    .expect("user message should append");
    append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "",
            [
                (
                    "raw_tool_calls_json".to_string(),
                    r#"[{"id":"call-1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]"#.to_string(),
                ),
                ("tool_call_count".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("assistant tool call should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Tool,
        "tool=file.read\nstatus=succeeded\noutput=hello",
        [
            ("kind".to_string(), "tool_observation".to_string()),
            ("tool_call_id".to_string(), "call-1".to_string()),
            ("tool".to_string(), "file.read".to_string()),
            ("status".to_string(), "succeeded".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("tool message should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let transcript = agent_transcript_from_active_events(&active_agent_events(&events));

    assert_eq!(transcript.len(), 3);
    assert!(matches!(transcript[1].role, MessageRole::Assistant));
    assert!(transcript[1].metadata.contains_key("raw_tool_calls_json"));
    assert!(matches!(transcript[2].role, MessageRole::Tool));
    assert_eq!(
        transcript[2]
            .metadata
            .get("tool_call_id")
            .map(String::as_str),
        Some("call-1")
    );

    let state = agent_state(&store, None).expect("agent state should load");
    assert!(state.run_started_at_ms > 0);
    assert_eq!(state.messages.len(), 3);
    assert_eq!(state.messages[0].role, "user");
    assert_eq!(state.messages[2].role, "tool");
}

#[test]
fn agent_trace_groups_steps_by_turn_and_exposes_details() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "read README".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "read README",
    )
    .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestStarted,
        "Agent model turn started",
        [
            ("request_id".to_string(), "agent-model-1".to_string()),
            ("turn".to_string(), "0".to_string()),
            ("model".to_string(), "model-a".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("model start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        [
            ("request_id".to_string(), "agent-model-1".to_string()),
            ("latency_ms".to_string(), "42".to_string()),
            ("tool_calls".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("model finish should append");
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-1",
        "file.write",
        "succeeded",
        "written ok",
        [("path".to_string(), "notes/result.md".to_string())]
            .into_iter()
            .collect(),
        None,
    )
    .expect("tool finish should append");

    let trace =
        agent_trace_state_for_session(&store, None, None, None).expect("trace should build");

    assert_eq!(trace.turn_count, 1);
    assert!(trace.step_count >= 5);
    assert!(trace.tool_call_count >= 1);
    assert!(trace.turns.iter().any(|turn| turn.index == 1
        && turn
            .steps
            .iter()
            .any(|step| { step.kind == "model" && step.latency_ms == Some(42) })));
    assert!(trace
        .turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .any(|step| step.tool_name.as_deref() == Some("file.write")
            && step.output_preview.as_deref() == Some("written ok")
            && step.artifact_path.as_deref() == Some("notes/result.md")));
}

#[test]
fn agent_trace_reports_actual_collaboration_role_activity() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-role-trace".to_string()),
        ("project_id".to_string(), "project-role-trace".to_string()),
        ("agent_run_id".to_string(), "run-role-trace".to_string()),
        (
            "collaboration_id".to_string(),
            "collab-role-trace".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "compare approaches".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("start should append");
    for (role, model, status, latency, first_token, tokens, evidence) in [
        (
            "planner",
            "model-planner",
            "completed",
            "120",
            "30",
            "80",
            "2",
        ),
        (
            "reviewer",
            "model-reviewer",
            "degraded",
            "90",
            "25",
            "40",
            "1",
        ),
        (
            "planner",
            "model-planner",
            "interrupted",
            "31",
            "",
            "0",
            "0",
        ),
    ] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            format!("Collaboration {role} finished"),
            metadata_with_context(
                [
                    ("role".to_string(), role.to_string()),
                    ("model".to_string(), model.to_string()),
                    ("status".to_string(), status.to_string()),
                    ("latency_ms".to_string(), latency.to_string()),
                    (
                        "first_token_latency_ms".to_string(),
                        first_token.to_string(),
                    ),
                    ("total_tokens".to_string(), tokens.to_string()),
                    ("evidence_count".to_string(), evidence.to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        )
        .expect("role event should append");
    }
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("completion should append");

    let trace = agent_trace_state_for_session(&store, None, None, Some("session-role-trace"))
        .expect("trace should build");

    assert_eq!(trace.role_summaries.len(), 2);
    assert_eq!(trace.role_summaries[0].role, "planner");
    assert_eq!(trace.role_summaries[0].models, vec!["model-planner"]);
    assert_eq!(trace.role_summaries[0].calls, 2);
    assert_eq!(trace.role_summaries[0].completed, 1);
    assert_eq!(trace.role_summaries[0].interrupted, 1);
    assert_eq!(trace.role_summaries[0].degraded, 0);
    assert_eq!(trace.role_summaries[0].latency_ms, 151);
    assert_eq!(trace.role_summaries[0].first_token_latency_ms, Some(30));
    assert_eq!(trace.role_summaries[1].role, "reviewer");
    assert_eq!(trace.role_summaries[1].degraded, 1);
    assert_eq!(trace.role_summaries[1].evidence_count, 1);
}

#[test]
fn agent_trace_export_writes_jsonl() {
    let root = temp_test_root("phase18-trace");
    fs::create_dir_all(&root).expect("temp root should exist");
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

    let path = write_agent_trace_jsonl(&root, &store, None).expect("trace should export");
    let text = fs::read_to_string(path).expect("trace export should be readable");

    assert!(text.contains("\"trace_id\""));
    assert!(text.contains("\"task_id\":\"phase-16-agent-loop\""));
    assert!(text.contains("\"kind\":\"message\""));
}
