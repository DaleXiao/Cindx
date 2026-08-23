use super::*;
use crate::collaboration_execution::collaboration_model_failure;
use crate::collaboration_stage_runtime::{
    collaboration_stage_result, collaboration_stage_terminal_presentation, CollaborationStageError,
};

#[test]
fn pending_steer_interrupts_collaboration_without_stopping_the_run() {
    let control = Arc::new(AgentRunControl::new("pro"));
    assert!(!collaboration_run_should_interrupt(&control));

    assert_eq!(control.request_steer("queue-steer"), Ok(true));
    assert!(control.has_pending_steer());
    assert!(collaboration_run_should_interrupt(&control));
    assert!(!agent_run_should_stop(&control));

    let pending = control.take_pending_steers();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].queue_id, "queue-steer");
    assert!(!collaboration_run_should_interrupt(&control));
}

#[test]
fn goal1_collaboration_start_and_finish_persist_the_same_typed_attribution() {
    let task_id = phase16_task_id();
    let run_context = [("session_id".to_string(), "typed-pair".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let attribution = AgentModelAttribution::actor(
        AgentActor::Specialist,
        AgentStage::Plan,
        AgentModelProfile::Reasoning,
        agent_core::AgentEffectAuthority::ReadOnly,
    );
    let started = collaboration_stage_started_metadata(
        "collaboration-pair",
        "analysis",
        &ModelRole::Planner,
        "reasoning-model",
        "request-pair",
        attribution,
        &Metadata::new(),
    )
    .expect("started attribution should be valid");
    let completion = CollaborationCompletion::completed_worker(
        "grounded plan".to_string(),
        7,
        Metadata::new(),
        Vec::new(),
    );
    let (summary, finished) = collaboration_stage_finished_metadata(
        "collaboration-pair",
        "analysis",
        &ModelRole::Planner,
        "reasoning-model",
        "request-pair",
        &completion,
        attribution,
        &Metadata::new(),
    )
    .expect("finished attribution should be valid");

    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &task_id,
        EventKind::ModelRequestStarted,
        "Collaboration analysis started",
        metadata_with_context(started, &run_context),
    )
    .expect("started event should persist");
    append_event(
        &mut store,
        &task_id,
        EventKind::ModelRequestFinished,
        summary,
        metadata_with_context(finished, &run_context),
    )
    .expect("finished event should persist");

    let events = store
        .list_by_task(&task_id)
        .expect("attribution events should load");
    assert_eq!(events.len(), 2);
    for key in [
        "agent_model_attribution_schema",
        "agent_actor",
        "agent_service",
        "agent_stage",
        "agent_model_profile",
        "agent_output_trust",
        "agent_effect_authority",
        "agent_attribution_component",
        "agent_attribution_model",
        "agent_attribution_legacy_role",
    ] {
        assert_eq!(
            events[0].metadata.get(key),
            events[1].metadata.get(key),
            "typed attribution diverged at {key}"
        );
    }
    assert_eq!(
        events[0].metadata.get("agent_actor").map(String::as_str),
        Some("specialist")
    );
    assert_eq!(
        events[0].metadata.get("agent_stage").map(String::as_str),
        Some("plan")
    );
}

#[test]
fn goal2_collaboration_terminal_status_distinguishes_interruptions_and_stage_deadlines() {
    let completed = CollaborationCompletion::completed_worker(
        "usable decision".to_string(),
        12,
        Metadata::new(),
        Vec::new(),
    );
    let interrupted =
        CollaborationCompletion::failed_with(AgentFailure::cancelled("user_steer", "superseded"));
    let stage_deadline = CollaborationCompletion::failed_with(AgentFailure::budget(
        RunStopReason::StageBudgetExhausted.code(),
        "deadline exhausted",
    ));
    let unavailable = CollaborationCompletion::failed("provider unavailable");

    assert_eq!(
        collaboration_stage_terminal_presentation(&completed),
        ("completed", "finished")
    );
    assert_eq!(
        collaboration_stage_terminal_presentation(&interrupted),
        ("interrupted", "interrupted")
    );
    assert_eq!(
        collaboration_stage_terminal_presentation(&stage_deadline),
        ("degraded", "deadline exhausted")
    );
    assert_eq!(
        collaboration_stage_terminal_presentation(&unavailable),
        ("degraded", "unavailable")
    );
    let deadline_result =
        collaboration_stage_result(CollaborationCompletion::failed_with(AgentFailure::budget(
            RunStopReason::StageBudgetExhausted.code(),
            "deadline exhausted",
        )));
    assert_eq!(deadline_result, Err(CollaborationStageError::StageDeadline));
}

#[test]
fn goal2_collaboration_cancellation_cause_prefers_run_stop_then_steer() {
    let cancelled = ModelError::new(MODEL_REQUEST_CANCELLED);
    let control = Arc::new(AgentRunControl::new("pro"));
    assert_eq!(control.request_steer("steer-first"), Ok(true));
    let steer_failure =
        collaboration_model_failure(&cancelled, Some(&control), RunStageClass::Conductor);
    assert_eq!(steer_failure.code, "user_steer");
    assert_eq!(steer_failure.class, AgentFailureClass::Cancelled);
    let mut steer_completion = CollaborationCompletion::failed_with(steer_failure);
    steer_completion.usage.insert(
        COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
        COLLABORATION_TERMINATION_STEER.to_string(),
    );
    let steer_result = collaboration_stage_result(steer_completion);
    assert_eq!(steer_result, Err(CollaborationStageError::SteerInterrupted));

    control.request_cancel();
    let cancelled_failure =
        collaboration_model_failure(&cancelled, Some(&control), RunStageClass::Conductor);
    assert_eq!(cancelled_failure.code, RunStopReason::UserCancelled.code());
    assert_eq!(cancelled_failure.class, AgentFailureClass::Cancelled);
    let mut stop_completion = CollaborationCompletion::failed_with(cancelled_failure);
    stop_completion.usage.insert(
        COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
        COLLABORATION_TERMINATION_RUN.to_string(),
    );
    let stop_result = collaboration_stage_result(stop_completion);
    assert_eq!(stop_result, Err(CollaborationStageError::RunStopped));
}

#[test]
fn collaboration_tool_worker_reserves_a_terminal_answer_turn() {
    let turn_policy = WorkerTurnPolicy::isolated_evidence(3, true);
    let mut runtime = start_agent_loop(
        TaskId("worker-finalization".to_string()),
        "Inspect evidence",
        AgentRuntimeConfig {
            max_turns: turn_policy.runtime_turn_limit(),
        },
    );

    assert_eq!(runtime.max_turns, 5);
    assert!(turn_policy.supports_evidence_repair());
    runtime.turn = 2;
    assert_eq!(
        turn_policy.prepare_turn(&mut runtime),
        WorkerTurnPhase::Evidence
    );
    runtime.turn = 3;
    assert_eq!(
        turn_policy.prepare_turn(&mut runtime),
        WorkerTurnPhase::Finalization
    );
    assert_eq!(
        runtime
            .messages
            .last()
            .and_then(|message| message.metadata.get("kind"))
            .map(String::as_str),
        Some("worker_finalization")
    );
    let message_count = runtime.messages.len();
    assert_eq!(
        turn_policy.prepare_turn(&mut runtime),
        WorkerTurnPhase::Finalization
    );
    assert_eq!(runtime.messages.len(), message_count);
}

#[test]
fn collaboration_stages_have_visible_timeline_labels() {
    let event = Event {
        id: EventId("candidate-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::ModelRequestStarted,
        summary: "Collaboration candidate_2 started".to_string(),
        metadata: [
            ("collaboration_id".to_string(), "collab-1".to_string()),
            ("stage".to_string(), "candidate_2".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    assert_eq!(timeline_event_label(&event), "Approach 2");
    assert_eq!(
        collaboration_stage_display_label("coordinator"),
        "Planning"
    );
    assert_eq!(
        collaboration_stage_display_label("conductor_plan"),
        "Planning"
    );
    assert_eq!(
        collaboration_stage_display_label("conductor_repair"),
        "Planning repair"
    );
    assert_eq!(
        collaboration_stage_display_label("run_decision_model_1"),
        "Planning"
    );
    assert_eq!(
        collaboration_stage_display_label("run_decision_model_1_repair"),
        "Planning repair"
    );
    assert_eq!(collaboration_stage_display_label("worker_3"), "Step 3");
    assert_eq!(collaboration_stage_display_label("arbiter"), "Selection");
    assert_eq!(
        collaboration_stage_display_label("synthesizer"),
        "Synthesis"
    );
}

#[test]
fn collaboration_prompt_keeps_core_user_instructions_and_runtime_context() {
    let context = [(
        "current_time".to_string(),
        "2026-07-11 10:30 CST".to_string(),
    )]
    .into_iter()
    .collect();
    let prompt = collaboration_system_prompt_for_run("Answer in Chinese.", &context);

    assert!(prompt.starts_with("You are Cindx"));
    assert!(prompt.contains("<user_instructions>\nAnswer in Chinese."));
    assert!(prompt.contains("<runtime_context>"));
    assert!(prompt.contains("Current date and time: 2026-07-11 10:30 CST"));
}
