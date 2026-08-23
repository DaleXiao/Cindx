use super::*;

#[test]
fn session_read_model_advances_from_only_new_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-read-model";
    let run_id = "run-read-model";
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("project_name".to_string(), "Project A".to_string()),
        ("session_id".to_string(), session_id.to_string()),
        ("session_name".to_string(), "Read model".to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [
                ("prompt".to_string(), "Inspect the workspace".to_string()),
                ("context_window_tokens".to_string(), "128000".to_string()),
                ("run_budget_ms".to_string(), "987654".to_string()),
                ("run_model_call_budget".to_string(), "37".to_string()),
                ("run_tool_call_budget".to_string(), "83".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("run start should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Inspect the workspace",
        context.clone(),
    )
    .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("prompt_tokens".to_string(), "640".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("model turn should append");

    let initial = load_agent_session_read_model(&mut store, session_id)
        .expect("initial read model should build");
    assert_eq!(initial.event_count, 3);
    assert_eq!(initial.state.turn_count, 1);
    assert_eq!(initial.state.context_tokens_used, 640);
    assert_eq!(initial.state.status, "running");
    assert_eq!(initial.state.run_budget_ms, 987_654);
    assert_eq!(initial.state.run_model_call_budget, 37);
    assert_eq!(initial.state.run_tool_call_budget, 83);
    assert_eq!(initial.state.max_turns, 37);

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "Workspace inspected",
        context.clone(),
    )
    .expect("assistant message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context.clone(),
    )
    .expect("completion should append");

    let updated = load_agent_session_read_model(&mut store, session_id)
        .expect("read model should apply the delta");
    assert_eq!(updated.event_count, 5);
    assert_eq!(updated.revision, initial.revision + 2);
    assert_eq!(updated.state.status, "completed");
    assert_eq!(updated.state.run_budget_ms, 987_654);
    assert_eq!(updated.state.run_model_call_budget, 37);
    assert_eq!(updated.state.run_tool_call_budget, 83);
    assert_eq!(updated.state.max_turns, 37);
    assert_eq!(
        updated.state.latest_answer.as_deref(),
        Some("Workspace inspected")
    );
    assert!(updated.state.can_retry);

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [
                (
                    "agent_run_id".to_string(),
                    "run-read-model-fast".to_string(),
                ),
                ("agent_effort".to_string(), "fast".to_string()),
                ("prompt".to_string(), "Inspect another change".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("next run start should append");

    let restarted = load_agent_session_read_model(&mut store, session_id)
        .expect("read model should apply the next run delta");
    let fast_budget = RunBudget::for_effort("fast");
    assert_eq!(restarted.event_count, 6);
    assert_eq!(restarted.revision, updated.revision + 1);
    assert_eq!(restarted.state.status, "running");
    assert_eq!(
        restarted.state.run_budget_ms,
        u64::try_from(fast_budget.max_duration.as_millis()).unwrap()
    );
    assert_eq!(
        restarted.state.run_model_call_budget,
        fast_budget.max_model_calls
    );
    assert_eq!(restarted.state.max_turns, fast_budget.max_model_calls);
    assert_eq!(
        restarted.state.run_tool_call_budget,
        fast_budget.max_tool_calls
    );

    let persisted = store
        .load_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, session_id)
        .expect("persisted read model should load")
        .expect("persisted read model should exist");
    assert_eq!(persisted.revision, restarted.revision);
}

#[test]
fn session_read_model_normalizes_legacy_cached_max_turns() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-legacy-budget-cache";
    let mut legacy = load_agent_session_read_model(&mut store, session_id)
        .expect("empty read model should build");
    legacy.state.max_turns = RunBudget::for_effort("auto").max_model_calls;
    let payload = serde_json::to_string(&legacy).expect("legacy read model should serialize");
    store
        .save_read_model(
            AGENT_SESSION_READ_MODEL_NAMESPACE,
            session_id,
            legacy.revision,
            &payload,
        )
        .expect("legacy read model should save");

    let normalized = load_agent_session_read_model(&mut store, session_id)
        .expect("legacy read model should normalize");
    assert_eq!(normalized.state.run_model_call_budget, 0);
    assert_eq!(normalized.state.max_turns, 0);

    let persisted = store
        .load_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, session_id)
        .expect("normalized read model should load")
        .expect("normalized read model should exist");
    let persisted: serde_json::Value =
        serde_json::from_str(&persisted.payload).expect("normalized payload should deserialize");
    assert_eq!(persisted["state"]["maxTurns"], 0);
}

#[test]
fn session_read_model_rebuilds_missing_legacy_run_budgets_from_effort() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-legacy-running-budget-cache";
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), "run-legacy-fast".to_string()),
            ("agent_effort".to_string(), "FAST".to_string()),
            ("prompt".to_string(), "Inspect the workspace".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("legacy run start should append");

    let mut legacy = load_agent_session_read_model(&mut store, session_id)
        .expect("read model should initially build");
    legacy.state.run_budget_ms = 0;
    legacy.state.run_model_call_budget = 0;
    legacy.state.run_tool_call_budget = 0;
    legacy.state.max_turns = RunBudget::for_effort("auto").max_model_calls;
    let payload = serde_json::to_string(&legacy).expect("legacy read model should serialize");
    store
        .save_read_model(
            AGENT_SESSION_READ_MODEL_NAMESPACE,
            session_id,
            legacy.revision,
            &payload,
        )
        .expect("legacy read model should save");

    let rebuilt = load_agent_session_read_model(&mut store, session_id)
        .expect("legacy run budget should rebuild");
    let fast_budget = RunBudget::for_effort("fast");
    assert_eq!(
        rebuilt.state.run_budget_ms,
        u64::try_from(fast_budget.max_duration.as_millis()).unwrap()
    );
    assert_eq!(
        rebuilt.state.run_model_call_budget,
        fast_budget.max_model_calls
    );
    assert_eq!(rebuilt.state.max_turns, fast_budget.max_model_calls);
    assert_eq!(
        rebuilt.state.run_tool_call_budget,
        fast_budget.max_tool_calls
    );
}

#[test]
fn session_history_page_reports_a_stable_older_cursor() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-history-page";
    for index in 0..7 {
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            &format!("message-{index}"),
            [("session_id".to_string(), session_id.to_string())]
                .into_iter()
                .collect(),
        )
        .expect("message should append");
    }

    let latest = store
        .list_by_task_and_metadata_before(&phase16_task_id(), "session_id", session_id, u64::MAX, 3)
        .expect("latest page should load");
    assert_eq!(
        latest
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![5, 6, 7]
    );

    let older = store
        .list_by_task_and_metadata_before(
            &phase16_task_id(),
            "session_id",
            session_id,
            latest[0].sequence,
            3,
        )
        .expect("older page should load");
    assert_eq!(
        older.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert!(store
        .has_task_metadata_event_before(
            &phase16_task_id(),
            "session_id",
            session_id,
            older[0].sequence,
        )
        .expect("older cursor should be checked"));
}
