use super::*;

#[test]
fn run_context_selects_the_primary_model_without_collapsing_to_executor() {
    let config = ProviderConfig {
        model: "default-a".to_string(),
        executor_model: "executor-b".to_string(),
        ..ProviderConfig::default()
    };

    assert_eq!(
        config.model_for_agent_policy(&OrchestrationPolicy::Single),
        "default-a"
    );
    assert_eq!(
        config.model_for_agent_policy(&OrchestrationPolicy::PlanExecuteReview),
        "executor-b"
    );

    let run_context = [
        ("collaboration_policy".to_string(), "single".to_string()),
        ("agent_model".to_string(), "routed-c".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(agent_model_for_run(&config, &run_context), "routed-c");
}

#[test]
fn agent_effort_keeps_auto_and_pro_under_dynamic_policy_selection() {
    assert_eq!(
        AgentPolicy::parse_ingress("fast").requested_policy(),
        OrchestrationPolicy::Single
    );
    assert_eq!(
        AgentPolicy::parse_ingress("auto").requested_policy(),
        OrchestrationPolicy::AutoRouter
    );
    assert_eq!(
        AgentPolicy::parse_ingress("pro").requested_policy(),
        OrchestrationPolicy::AutoRouter
    );
    assert_eq!(AgentPolicy::parse_ingress("unknown"), AgentPolicy::Default);
    assert_eq!(persisted_agent_policy(None), Ok(AgentPolicy::Default));
    assert_eq!(persisted_agent_policy(Some("pro")), Ok(AgentPolicy::High));
    assert!(persisted_agent_policy(Some("Pro")).is_err());
    assert!(persisted_agent_policy(Some("future")).is_err());
}

#[test]
fn retry_recovers_effort_from_the_active_run() {
    let mut event = Event {
        id: EventId("run-start".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task started".to_string(),
        metadata: [("agent_effort".to_string(), "pro".to_string())]
            .into_iter()
            .collect(),
    };

    assert_eq!(
        persisted_agent_policy_from_active_events(std::slice::from_ref(&event)),
        Ok(AgentPolicy::High)
    );

    event
        .metadata
        .insert("agent_effort".to_string(), "future".to_string());
    assert!(persisted_agent_policy_from_active_events(&[event]).is_err());
    assert_eq!(
        persisted_agent_policy_from_active_events(&[]),
        Ok(AgentPolicy::Default)
    );
}

#[test]
fn agent_runtime_turn_budget_tracks_effort_and_extends_on_resume() {
    let control = AgentRunControl::new("pro");
    let config = control.runtime_config();
    assert_eq!(config.max_turns, 384);

    let mut runtime = start_agent_loop(phase16_task_id(), "continue", config);
    runtime.turn = 23;
    control.extend_runtime_budget(&mut runtime);

    assert_eq!(runtime.max_turns, 407);
}

#[test]
fn agent_run_budget_extends_only_after_material_progress_and_stops_cycles() {
    let control = AgentRunControl::new("fast");
    for call in 1..=6 {
        assert_eq!(control.begin_model_call("executor"), Ok(call));
    }
    assert!(control.record_observation("model_result", "executor", "new model wording"));
    assert_eq!(
        control.begin_model_call("executor"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );

    let control = AgentRunControl::new("fast");
    for call in 1..=6 {
        assert_eq!(control.begin_model_call("executor"), Ok(call));
    }
    let tools = vec![ToolSpec::builtin(
        "file.read",
        "test",
        "Read required evidence",
        ToolRisk::ReadOnly,
        r#"{"type":"object"}"#,
    )];
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "read required evidence",
        AgentRuntimeConfig::default(),
    );
    runtime.task_contract.require_tool_success("file.read");
    let delta = AgentKernel::new(&mut runtime, &tools)
        .apply_tool_observation(
            &AgentToolRequest {
                call_id: agent_core::ToolCallId("goal-delta-read".to_string()),
                tool_name: "file.read".to_string(),
                input: r#"{"path":"goal.md"}"#.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
            "required evidence",
        )
        .expect("the required evidence should emit one Goal Delta");
    assert!(control.record_goal_delta_at(0, &delta));
    assert_eq!(control.begin_model_call("executor"), Ok(7));
    assert!(!control.record_goal_delta_at(0, &delta));

    let cycle_control = AgentRunControl::new("fast");
    for input in ["a", "b", "a", "b", "a", "b", "a"] {
        assert!(cycle_control
            .begin_tool_call("executor", "file.read", input)
            .is_ok());
    }
    assert_eq!(
        cycle_control.begin_tool_call("executor", "file.read", "b"),
        Err(RunStopReason::RepeatedAction)
    );
}

#[test]
fn partial_budget_completion_exposes_a_continuation() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("prompt".to_string(), "finish the long task".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        context.clone(),
    )
    .expect("run start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [
                ("completion".to_string(), "partial".to_string()),
                ("continuation_available".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("partial completion should append");

    let state =
        agent_state_for_session(&store, None, Some("session-a")).expect("agent state should load");
    assert_eq!(state.status, "completed");
    assert!(state.can_retry);
    assert!(state.can_continue);
}
