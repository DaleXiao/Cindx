use super::*;

#[test]
fn routing_telemetry_read_model_deduplicates_completed_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let start_context = [
        ("session_id".to_string(), "session-router".to_string()),
        ("agent_run_id".to_string(), "run-router-1".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let run_context = [
        ("session_id".to_string(), "session-router".to_string()),
        ("agent_run_id".to_string(), "run-router-1".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("agent_model".to_string(), "model-a".to_string()),
        ("routing_signature".to_string(), "coding:3".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        start_context,
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        run_context.clone(),
    )
    .expect("dynamic run decision should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("total_tokens".to_string(), "900".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("model telemetry should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        run_context.clone(),
    )
    .expect("run should complete");

    let initial =
        load_routing_telemetry_read_model(&mut store).expect("routing read model should build");
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].selected_model, "model-a");
    assert_eq!(initial[0].task_class, TaskClass::Coding);
    assert_eq!(
        initial[0].selected_policy,
        OrchestrationPolicy::PlanExecuteReview
    );
    assert_eq!(initial[0].context_signature, "coding:3");
    assert_eq!(initial[0].cost_proxy, 900);

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "unrelated follow-up",
        [("session_id".to_string(), "session-router".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("message should append");
    let updated =
        load_routing_telemetry_read_model(&mut store).expect("routing read model should advance");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].selected_model, "model-a");
}

#[test]
fn routing_telemetry_is_reconstructed_from_completed_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = [
        ("agent_run_id".to_string(), "run-1".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::RetrievalPerformed,
        "RAG agent_context completed",
        run_context.clone(),
    )
    .expect("retrieval should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("total_tokens".to_string(), "120".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("model completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        run_context,
    )
    .expect("completion should append");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");

    let telemetry = routing_telemetry_from_events(&events);

    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].task_class, TaskClass::Coding);
    assert_eq!(telemetry[0].outcome, RoutingOutcome::Succeeded);
    assert_eq!(telemetry[0].cost_proxy, 120);
    assert_eq!(telemetry[0].retrieval_count, 1);
}

#[test]
fn goal2_routing_telemetry_learns_only_the_latest_replayed_decision() {
    let base = [
        ("agent_run_id".to_string(), "run-replanned".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let decision = |task_class: &str, policy: &str, model: &str, epoch: &str| {
        metadata_with_context(
            [
                ("task_class".to_string(), task_class.to_string()),
                ("collaboration_policy".to_string(), policy.to_string()),
                ("router_model".to_string(), model.to_string()),
                ("steer_epoch".to_string(), epoch.to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        )
    };
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        base.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        decision("general", "single", "stale-model", "0"),
    )
    .expect("stale decision should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "stale model finished",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "0".to_string()),
                ("total_tokens".to_string(), "100".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("stale cost should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        decision("coding", "plan_execute_review", "replanned-model", "1"),
    )
    .expect("replayed decision should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "current model finished",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "1".to_string()),
                ("total_tokens".to_string(), "7".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("current cost should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [("steer_epoch".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
            &base,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let telemetry = routing_telemetry_from_events(&events);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].task_class, TaskClass::Coding);
    assert_eq!(telemetry[0].selected_model, "replanned-model");
    assert_eq!(telemetry[0].cost_proxy, 7);
}

#[test]
fn routing_telemetry_excludes_unverified_workspace_completions() {
    let run_context = [
        ("agent_run_id".to_string(), "run-unverified".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [
                (
                    "completion_evidence".to_string(),
                    "unverified_mutation".to_string(),
                ),
                ("routing_learning_eligible".to_string(), "false".to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    assert!(routing_telemetry_from_events(&events).is_empty());
}

#[test]
fn routing_telemetry_uses_collaboration_quality_gate_as_outcome() {
    let run_context = [
        ("agent_run_id".to_string(), "run-low-quality".to_string()),
        ("task_class".to_string(), "research".to_string()),
        ("collaboration_policy".to_string(), "best_of_n".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Collaboration quality gate evaluated",
        metadata_with_context(
            [
                ("quality_pass".to_string(), "false".to_string()),
                ("quality_score".to_string(), "0.61".to_string()),
                ("safety_violations".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("quality gate should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [("routing_learning_eligible".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let telemetry = routing_telemetry_from_events(&events);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].outcome, RoutingOutcome::Failed);
    assert_eq!(telemetry[0].quality_score, Some(0.61));
    assert_eq!(telemetry[0].verification_passed, Some(false));
}

#[test]
fn completion_learning_signal_requires_post_mutation_verification() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "update the workspace",
        AgentRuntimeConfig::default(),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("non_mutating", false)
    );

    record_tool_outcome_with_risk(
        &mut runtime,
        "file.write",
        r#"{"path":"src/lib.rs"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::WritesWorkspace),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("unverified_mutation", false)
    );

    record_tool_outcome_with_risk(
        &mut runtime,
        "process.run",
        r#"{"command":"cargo test"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ExecutesProcess),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("unverified_mutation", false)
    );

    record_tool_outcome_with_risk(
        &mut runtime,
        "file.read",
        r#"{"path":"src/lib.rs"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ReadOnly),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("verified_mutation", true)
    );
}
