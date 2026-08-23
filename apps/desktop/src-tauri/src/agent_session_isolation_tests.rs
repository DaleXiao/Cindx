use super::*;

#[test]
fn agent_state_and_trace_are_isolated_by_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, prompt, answer) in [
        ("session-a", "inspect alpha", "alpha answer"),
        ("session-b", "inspect beta", "beta answer"),
        ("session-a", "follow up alpha", "second alpha answer"),
    ] {
        let context = [
            ("project_id".to_string(), "project-cindx".to_string()),
            ("project_name".to_string(), "Cindx".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("session_name".to_string(), session_id.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut start_metadata = context.clone();
        start_metadata.insert("prompt".to_string(), prompt.to_string());
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            start_metadata,
        )
        .expect("start should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            prompt,
            context.clone(),
        )
        .expect("user message should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            answer,
            context,
        )
        .expect("assistant message should append");
    }

    let alpha =
        agent_state_for_session(&store, None, Some("session-a")).expect("alpha state should load");
    let beta =
        agent_state_for_session(&store, None, Some("session-b")).expect("beta state should load");
    let alpha_trace = agent_trace_state_for_session(&store, None, None, Some("session-a"))
        .expect("alpha trace should load");

    assert_eq!(alpha.session_id.as_deref(), Some("session-a"));
    assert_eq!(alpha.messages.len(), 4);
    assert_eq!(alpha.messages[1].content, "alpha answer");
    assert_eq!(alpha.messages[3].content, "second alpha answer");
    assert_eq!(beta.session_id.as_deref(), Some("session-b"));
    assert_eq!(beta.messages.len(), 2);
    assert_eq!(beta.messages[1].content, "beta answer");
    assert_eq!(alpha_trace.session_id.as_deref(), Some("session-a"));
    assert!(alpha_trace
        .turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .all(|step| !step.detail.contains("beta")));
}

#[test]
fn interleaved_agent_runs_remain_isolated_by_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let alpha = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let beta = [("session_id".to_string(), "session-b".to_string())]
        .into_iter()
        .collect::<Metadata>();

    for (summary, context) in [
        ("Agent task started", alpha.clone()),
        ("Agent task started", beta.clone()),
    ] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            summary,
            context,
        )
        .expect("run start should append");
    }
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "alpha finished after beta started",
        alpha.clone(),
    )
    .expect("alpha answer should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "beta answer",
        beta.clone(),
    )
    .expect("beta answer should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        alpha,
    )
    .expect("alpha completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        beta,
    )
    .expect("beta completion should append");

    let alpha_state =
        agent_state_for_session(&store, None, Some("session-a")).expect("alpha state should load");
    let beta_state =
        agent_state_for_session(&store, None, Some("session-b")).expect("beta state should load");

    assert_eq!(alpha_state.status, "completed");
    assert_eq!(alpha_state.messages.len(), 1);
    assert_eq!(
        alpha_state.messages[0].content,
        "alpha finished after beta started"
    );
    assert_eq!(beta_state.status, "completed");
    assert_eq!(beta_state.messages.len(), 1);
    assert_eq!(beta_state.messages[0].content, "beta answer");
}

#[test]
fn cancelling_one_session_does_not_cancel_another() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for session_id in ["session-a", "session-b"] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("session_id".to_string(), session_id.to_string())]
                .into_iter()
                .collect(),
        )
        .expect("run start should append");
    }
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("cancellation should append");

    assert!(agent_task_is_cancelled(&mut store, Some("session-a"))
        .expect("alpha cancellation should load"));
    assert!(!agent_task_is_cancelled(&mut store, Some("session-b"))
        .expect("beta cancellation should load"));
}
