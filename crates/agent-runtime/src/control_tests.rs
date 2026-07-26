use super::*;
use std::thread;

fn test_budget() -> RunBudget {
    RunBudget {
        max_duration: Duration::from_millis(60),
        model_call_timeout: Duration::from_millis(40),
        tool_call_timeout: Duration::from_millis(45),
        initial_model_calls: 2,
        max_model_calls: 2,
        model_calls_per_extension: 1,
        initial_tool_calls: 2,
        max_tool_calls: 2,
        tool_calls_per_extension: 1,
        no_progress_timeout: Duration::from_millis(25),
        max_identical_actions: 2,
        initial_agent_turns: 2,
        max_agent_turns: 2,
        agent_turns_per_extension: 1,
        max_repair_attempts: 2,
        terminal_model_call_reserve: 1,
        terminal_time_reserve: Duration::from_millis(5),
    }
}

#[test]
fn pro_budget_allows_a_long_running_segment() {
    let budget = RunBudget::for_effort("pro");
    assert_eq!(budget.max_duration, Duration::from_secs(4 * 60 * 60));
    assert_eq!(budget.model_call_timeout, Duration::from_secs(15 * 60));
    assert_eq!(budget.tool_call_timeout, Duration::from_secs(60 * 60));
    assert_eq!(budget.initial_model_calls, 48);
    assert_eq!(budget.max_model_calls, 384);
    assert_eq!(budget.initial_tool_calls, 96);
    assert_eq!(budget.max_tool_calls, 768);
    assert_eq!(budget.no_progress_timeout, Duration::from_secs(5 * 60));
    assert_eq!(budget.max_agent_turns, 384);
    assert_eq!(budget.max_repair_attempts, 8);
    assert_eq!(budget.terminal_model_call_reserve, 8);
}

#[test]
fn runtime_turn_budget_comes_from_the_same_control_budget() {
    let control = AgentRunControl::new("pro");
    assert_eq!(control.runtime_config().max_turns, 384);

    let mut runtime = crate::start_agent_loop(
        agent_core::TaskId("resume".to_string()),
        "continue",
        AgentRuntimeConfig { max_turns: 6 },
    );
    runtime.turn = 23;
    control.extend_runtime_budget(&mut runtime);
    assert_eq!(runtime.max_turns, 407);
}

#[test]
fn successful_provider_responses_keep_runtime_and_control_turn_ledgers_aligned() {
    let control = AgentRunControl::with_budget(test_budget());
    let mut runtime = crate::start_agent_loop(
        agent_core::TaskId("ledger-alignment".to_string()),
        "continue until the bounded run ends",
        control.runtime_config(),
    );

    for expected_turn in 1..=2 {
        crate::AgentKernel::new(&mut runtime, &[])
            .prepare_model_turn(None, None, 8_192, 1_024)
            .expect("the shared turn ledger should admit this request");
        assert_eq!(control.begin_model_call("executor"), Ok(expected_turn));
        control.finish_model_call();
        assert_eq!(control.record_agent_turn("executor"), Ok(expected_turn));

        let response = model_provider::ModelResponse {
            message: agent_core::Message {
                role: agent_core::MessageRole::Assistant,
                content: String::new(),
                metadata: agent_core::Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata: agent_core::Metadata::new(),
        };
        let advance = crate::AgentKernel::new(&mut runtime, &[]).advance_model_response(response);
        if expected_turn < 2 {
            assert!(matches!(advance, crate::AgentAdvance::Retry { .. }));
        } else {
            assert!(matches!(
                advance,
                crate::AgentAdvance::TurnBudgetExhausted(_)
            ));
        }
        assert_eq!(runtime.turn, expected_turn);
        assert_eq!(control.progress().agent_turns, expected_turn);
    }

    assert!(crate::AgentKernel::new(&mut runtime, &[])
        .prepare_model_turn(None, None, 8_192, 1_024)
        .is_err());
    assert_eq!(control.progress().model_calls, 2);
    assert_eq!(control.progress().agent_turns, 2);
}

#[test]
fn material_checkpoints_extend_a_segment_but_new_observations_do_not() {
    let mut budget = test_budget();
    budget.initial_model_calls = 1;
    budget.max_model_calls = 3;
    let control = AgentRunControl::with_budget(budget);

    assert_eq!(control.begin_model_call("one"), Ok(1));
    assert!(control.record_observation("model", "answer", "novel chatter"));
    assert_eq!(
        control.begin_model_call("two"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );

    let control = AgentRunControl::with_budget(budget);
    assert_eq!(control.begin_model_call("one"), Ok(1));
    assert!(control.record_checkpoint("model", "answer", "evidence-a"));
    assert_eq!(control.begin_model_call("two"), Ok(2));
    assert!(!control.record_checkpoint("model", "answer", "evidence-a"));
    assert_eq!(
        control.begin_model_call("three"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );
    assert_eq!(control.progress().budget_extensions, 1);
    assert_eq!(control.progress().observations, 0);
}

#[test]
fn enforces_model_and_tool_call_budgets() {
    let model_control = AgentRunControl::with_budget(test_budget());
    assert_eq!(model_control.begin_model_call("one"), Ok(1));
    assert_eq!(model_control.begin_model_call("two"), Ok(2));
    assert_eq!(
        model_control.begin_model_call("three"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );
    assert_eq!(model_control.progress().model_calls, 2);

    let tool_control = AgentRunControl::with_budget(test_budget());
    assert_eq!(
        tool_control.begin_tool_call("main", "file.read", "a"),
        Ok(1)
    );
    assert_eq!(
        tool_control.begin_tool_call("main", "file.read", "b"),
        Ok(2)
    );
    assert_eq!(
        tool_control.begin_tool_call("main", "file.read", "c"),
        Err(RunStopReason::ToolCallBudgetExceeded)
    );
}

#[test]
fn detects_repeated_actions_within_a_scope() {
    let mut budget = test_budget();
    budget.initial_tool_calls = 10;
    budget.max_tool_calls = 10;
    let control = AgentRunControl::with_budget(budget);
    assert!(control
        .begin_tool_call("worker-1", "file.read", "a")
        .is_ok());
    assert!(control
        .begin_tool_call("worker-1", "file.read", "a")
        .is_ok());
    assert_eq!(
        control.begin_tool_call("worker-1", "file.read", "a"),
        Err(RunStopReason::RepeatedAction)
    );
}

#[test]
fn a_different_action_resets_the_repeat_guard() {
    let mut budget = test_budget();
    budget.initial_tool_calls = 10;
    budget.max_tool_calls = 10;
    let control = AgentRunControl::with_budget(budget);
    assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
    assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
    assert!(control.begin_tool_call("main", "file.read", "b").is_ok());
    assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
    assert_eq!(control.stop_reason(), None);
}

#[test]
fn detects_short_alternating_action_cycles() {
    let mut budget = test_budget();
    budget.initial_tool_calls = 12;
    budget.max_tool_calls = 12;
    let control = AgentRunControl::with_budget(budget);
    for input in ["a", "b", "a", "b", "a"] {
        assert!(control.begin_tool_call("main", "file.read", input).is_ok());
    }
    assert_eq!(
        control.begin_tool_call("main", "file.read", "b"),
        Err(RunStopReason::RepeatedAction)
    );
}

#[test]
fn stops_after_no_progress() {
    let control = AgentRunControl::with_budget(test_budget());
    thread::sleep(Duration::from_millis(35));
    assert_eq!(control.stop_reason(), Some(RunStopReason::NoProgress));
}

#[test]
fn hard_deadline_wins_before_the_idle_limit() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_millis(20);
    budget.no_progress_timeout = Duration::from_secs(1);
    let control = AgentRunControl::with_budget(budget);
    thread::sleep(Duration::from_millis(30));
    assert_eq!(control.stop_reason(), Some(RunStopReason::DeadlineExceeded));
}

#[test]
fn user_cancellation_is_sticky() {
    let control = AgentRunControl::with_budget(test_budget());
    control.request_cancel();
    control.mark_progress("model", "late delta");
    assert_eq!(control.stop_reason(), Some(RunStopReason::UserCancelled));
}

#[test]
fn snapshot_excludes_permission_wait_time() {
    let control = AgentRunControl::with_budget(test_budget());
    control
        .begin_model_call("planning")
        .expect("model call should start");
    control.record_partial_output("verified work");
    assert!(control.record_observation("model_result", "planning", "draft-a"));
    assert!(control.record_checkpoint("tool_result", "file.read", "evidence-a"));
    control.finish_model_call();
    let snapshot = control.snapshot();
    thread::sleep(Duration::from_millis(35));
    let resumed = AgentRunControl::from_snapshot(snapshot);
    assert_eq!(resumed.stop_reason(), None);
    assert_eq!(resumed.partial_output(), "verified work");
    assert_eq!(resumed.progress().model_calls, 1);
    assert_eq!(resumed.progress().observations, 1);
    assert_eq!(resumed.progress().checkpoints, 1);
}

#[test]
fn continuation_starts_a_fresh_bounded_segment_after_budget_exhaustion() {
    let control = AgentRunControl::with_budget(test_budget());
    control.record_partial_output("verified work");
    assert!(control.record_checkpoint("tool", "created file", "artifact-a"));
    assert_eq!(control.request_steer("queue-a"), Ok(true));
    assert_eq!(control.begin_model_call("one"), Ok(1));
    assert_eq!(control.begin_model_call("two"), Ok(2));
    assert_eq!(
        control.begin_model_call("three"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );

    let continued = AgentRunControl::from_snapshot_for_continuation(control.snapshot())
        .expect("budget exhaustion should be resumable");
    assert_eq!(continued.stop_reason(), None);
    assert_eq!(continued.partial_output(), "verified work");
    assert_eq!(continued.progress().model_calls, 0);
    assert_eq!(continued.progress().checkpoints, 1);
    assert_eq!(continued.begin_model_call("continued-one"), Ok(1));
    assert_eq!(continued.begin_model_call("continued-two"), Ok(2));
    assert_eq!(
        continued.begin_model_call("continued-three"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );
    assert_eq!(
        continued
            .take_pending_steers()
            .into_iter()
            .map(|steer| steer.queue_id)
            .collect::<Vec<_>>(),
        vec!["queue-a"]
    );
}

#[test]
fn permission_resume_preserves_consumed_budget() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.begin_model_call("planning"), Ok(1));
    control.finish_model_call();

    let resumed = AgentRunControl::from_snapshot(control.snapshot());
    assert_eq!(resumed.stop_reason(), None);
    assert_eq!(resumed.progress().model_calls, 1);
    assert_eq!(resumed.begin_model_call("after-permission"), Ok(2));
    assert_eq!(
        resumed.begin_model_call("over-budget"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );
}

#[test]
fn user_cancelled_snapshot_cannot_continue() {
    let control = AgentRunControl::with_budget(test_budget());
    control.request_cancel();
    assert_eq!(
        AgentRunControl::from_snapshot_for_continuation(control.snapshot())
            .expect_err("user cancellation must remain terminal"),
        RunStopReason::UserCancelled
    );
}

#[test]
fn active_model_call_uses_the_model_timeout_before_no_progress() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_secs(1);
    budget.no_progress_timeout = Duration::from_millis(20);
    budget.model_call_timeout = Duration::from_millis(80);
    let control = AgentRunControl::with_budget(budget);

    control
        .begin_model_call("executor")
        .expect("model call should start");
    thread::sleep(Duration::from_millis(35));
    assert_eq!(control.stop_reason(), None);

    control.finish_model_call();
    thread::sleep(Duration::from_millis(30));
    assert_eq!(control.stop_reason(), Some(RunStopReason::NoProgress));
}

#[test]
fn active_tool_call_uses_its_own_timeout_and_finishes_cleanly() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_secs(1);
    budget.no_progress_timeout = Duration::from_millis(20);
    budget.tool_call_timeout = Duration::from_millis(90);
    let control = AgentRunControl::with_budget(budget);

    control
        .begin_tool_call("main", "shell.run", "build")
        .expect("tool call should start");
    thread::sleep(Duration::from_millis(35));
    assert_eq!(control.stop_reason(), None);

    control.finish_tool_call();
    thread::sleep(Duration::from_millis(30));
    assert_eq!(control.stop_reason(), Some(RunStopReason::NoProgress));
}

#[test]
fn rejected_tool_calls_do_not_consume_the_call_counter() {
    let mut budget = test_budget();
    budget.initial_tool_calls = 10;
    budget.max_tool_calls = 10;
    budget.max_identical_actions = 1;
    let control = AgentRunControl::with_budget(budget);

    assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
    control.finish_tool_call();
    assert_eq!(
        control.begin_tool_call("main", "file.read", "a"),
        Err(RunStopReason::RepeatedAction)
    );
    assert_eq!(control.progress().tool_calls, 1);
}

#[test]
fn steering_is_deduplicated_and_survives_a_snapshot() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.request_steer("queue-a"), Ok(true));
    assert_eq!(control.request_steer("queue-a"), Ok(false));
    assert!(control.has_pending_steer());

    let resumed = AgentRunControl::from_snapshot(control.snapshot());
    assert_eq!(
        resumed
            .take_pending_steers()
            .into_iter()
            .map(|steer| steer.queue_id)
            .collect::<Vec<_>>(),
        vec!["queue-a"]
    );
    assert!(!resumed.has_pending_steer());
}

#[test]
fn partial_output_keeps_the_latest_bounded_unicode_tail() {
    let control = AgentRunControl::with_budget(test_budget());
    let output = format!(
        "{}{}",
        "old".repeat(PARTIAL_OUTPUT_MAX_CHARS),
        "latest verified result 你好"
    );

    control.record_partial_output(&output);

    let partial = control.partial_output();
    assert_eq!(partial.chars().count(), PARTIAL_OUTPUT_MAX_CHARS);
    assert!(partial.ends_with("latest verified result 你好"));
    assert_ne!(partial, output);
}

#[test]
fn agent_turns_repairs_and_model_calls_are_accounted_independently() {
    let control = AgentRunControl::with_budget(test_budget());

    assert_eq!(control.begin_model_call("executor"), Ok(1));
    control.finish_model_call();
    assert_eq!(control.record_agent_turn("executor"), Ok(1));
    assert_eq!(control.begin_repair_attempt("protocol_repair"), Ok(1));

    let progress = control.progress();
    assert_eq!(progress.model_calls, 1);
    assert_eq!(progress.agent_turns, 1);
    assert_eq!(progress.repair_attempts, 1);
    assert_eq!(progress.tool_calls, 0);
}

#[test]
fn stage_exhaustion_is_local_and_preserves_terminal_reserve() {
    let mut budget = test_budget();
    budget.initial_model_calls = 4;
    budget.max_model_calls = 4;
    budget.terminal_model_call_reserve = 1;
    let control = AgentRunControl::with_budget(budget);

    assert!(control
        .begin_stage_model_call("candidate_1", RunStageClass::Candidate)
        .is_ok());
    control.finish_model_call();
    assert!(control
        .begin_stage_model_call("candidate_2", RunStageClass::Candidate)
        .is_ok());
    control.finish_model_call();
    assert_eq!(
        control.begin_stage_model_call("candidate_3", RunStageClass::Candidate),
        Err(RunStopReason::StageBudgetExhausted)
    );
    assert_eq!(control.stop_reason(), None);
    assert!(control
        .begin_stage_model_call("synthesizer", RunStageClass::Synthesizer)
        .is_ok());
}

#[test]
fn internal_terminal_stages_cannot_consume_final_user_delivery_calls() {
    let mut budget = test_budget();
    budget.initial_model_calls = 6;
    budget.max_model_calls = 6;
    budget.terminal_model_call_reserve = 4;
    let control = AgentRunControl::with_budget(budget);

    for stage in ["candidate_1", "candidate_2"] {
        control
            .begin_stage_model_call(stage, RunStageClass::Candidate)
            .expect("candidate call");
        control.finish_model_call();
    }
    for stage in ["synthesis_1", "synthesis_2"] {
        control
            .begin_stage_model_call(stage, RunStageClass::Synthesizer)
            .expect("internal terminal call");
        control.finish_model_call();
    }
    assert_eq!(
        control.begin_stage_model_call("synthesis_3", RunStageClass::Synthesizer),
        Err(RunStopReason::StageBudgetExhausted)
    );

    for stage in ["delivery_1", "delivery_2"] {
        control
            .begin_stage_model_call(stage, RunStageClass::Finalizer)
            .expect("finalizer call");
        control.finish_model_call();
    }
    assert_eq!(control.progress().model_calls, 6);
}

#[test]
fn main_loop_enters_terminal_commit_before_exhausting_its_last_call() {
    let mut budget = test_budget();
    budget.initial_model_calls = 4;
    budget.max_model_calls = 4;
    budget.terminal_model_call_reserve = 1;
    let control = AgentRunControl::with_budget(budget);

    assert_eq!(
        control.continuation_directive(),
        RunContinuationDirective::Continue
    );
    for _ in 0..3 {
        control.begin_model_call("executor").expect("model call");
        control.finish_model_call();
    }
    assert_eq!(
        control.continuation_directive(),
        RunContinuationDirective::CommitTerminalResult
    );
}

#[test]
fn nonterminal_model_timeout_cannot_consume_terminal_time_reserve() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_secs(100);
    budget.model_call_timeout = Duration::from_secs(90);
    budget.terminal_time_reserve = Duration::from_secs(60);
    let control = AgentRunControl::with_budget(budget);

    let nonterminal = control.stage_model_call_timeout(RunStageClass::Other);
    assert!(nonterminal <= Duration::from_secs(40));
    assert!(nonterminal > Duration::from_secs(39));

    let terminal = control.stage_model_call_timeout(RunStageClass::Synthesizer);
    assert!(terminal <= Duration::from_secs(50));
    assert!(terminal > Duration::from_secs(49));

    let finalizer = control.stage_model_call_timeout(RunStageClass::Finalizer);
    assert!(finalizer <= Duration::from_secs(30));
    assert!(finalizer > Duration::from_secs(29));
}

#[test]
fn terminal_delivery_stages_receive_a_delivery_sized_time_budget() {
    let mut budget = RunBudget::for_effort("pro");
    budget.max_duration = Duration::from_secs(300);

    let reviewer = budget.stage_budget(RunStageClass::Reviewer);
    let synthesizer = budget.stage_budget(RunStageClass::Synthesizer);

    assert!(reviewer.terminal);
    assert!(synthesizer.terminal);
    assert_eq!(reviewer.max_duration, Duration::from_secs(100));
    assert_eq!(synthesizer.max_duration, Duration::from_secs(150));
}

#[test]
fn worker_stage_can_use_the_nonterminal_window_without_an_arbitrary_half_time_cap() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_secs(300);
    budget.model_call_timeout = Duration::from_secs(300);
    budget.terminal_time_reserve = Duration::from_secs(60);
    let control = AgentRunControl::with_budget(budget);

    let worker = control.stage_model_call_timeout(RunStageClass::Worker);
    assert!(worker <= Duration::from_secs(240));
    assert!(worker > Duration::from_secs(239));
}

#[test]
fn nonterminal_stage_yields_when_the_terminal_reserve_begins() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_millis(40);
    budget.terminal_time_reserve = Duration::from_millis(30);
    let mut snapshot = AgentRunControl::with_budget(budget).snapshot();
    snapshot.elapsed_active = Duration::from_millis(12);
    let control = AgentRunControl::from_snapshot(snapshot);

    assert!(control.stage_should_stop(RunStageClass::Worker));
    assert!(!control.stage_should_stop(RunStageClass::Synthesizer));
}

#[test]
fn internal_terminal_stage_yields_before_the_finalizer_reserve() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_millis(100);
    budget.terminal_time_reserve = Duration::from_millis(60);
    let mut snapshot = AgentRunControl::with_budget(budget).snapshot();
    snapshot.elapsed_active = Duration::from_millis(71);
    let control = AgentRunControl::from_snapshot(snapshot);

    assert!(control.stage_should_stop(RunStageClass::Synthesizer));
    assert!(!control.stage_should_stop(RunStageClass::Finalizer));
}

#[test]
fn best_known_result_is_ranked_and_survives_resume() {
    let control = AgentRunControl::with_budget(test_budget());
    assert!(control.record_best_known_result(
        "candidate",
        "grounded candidate",
        ResultQuality::Grounded,
        2,
        false,
        false,
    ));
    assert!(!control.record_best_known_result(
        "draft",
        "longer but weaker draft",
        ResultQuality::Draft,
        0,
        false,
        false,
    ));
    assert!(control.record_best_known_result(
        "verification",
        "verified answer",
        ResultQuality::Verified,
        3,
        true,
        true,
    ));

    let resumed = AgentRunControl::from_snapshot(control.snapshot());
    let best = resumed
        .best_known_result()
        .expect("best result should persist");
    assert_eq!(best.content, "verified answer");
    assert_eq!(best.quality, ResultQuality::Verified);
    assert!(best.verified);
    assert!(best.deliverable);
}

#[test]
fn result_frontier_keeps_stronger_internal_guidance_without_replacing_deliverable() {
    let control = AgentRunControl::with_budget(test_budget());
    assert!(control.record_best_known_result(
        "draft_delivery",
        "safe user-facing draft",
        ResultQuality::Draft,
        0,
        false,
        true,
    ));
    assert!(!control.record_best_known_result(
        "verified_candidate",
        "strong internal evidence",
        ResultQuality::Verified,
        4,
        true,
        false,
    ));

    let resumed = AgentRunControl::from_snapshot(control.snapshot());
    assert_eq!(
        resumed
            .best_known_result()
            .expect("deliverable should remain available")
            .content,
        "safe user-facing draft"
    );
    let guidance = resumed
        .best_guidance_result()
        .expect("strong guidance should survive resume");
    assert_eq!(guidance.content, "strong internal evidence");
    assert!(guidance.verified);
    assert!(!guidance.deliverable);
}
