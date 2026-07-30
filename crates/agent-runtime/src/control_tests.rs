use super::*;
use std::sync::{mpsc, Arc, Barrier};
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
        max_total_tokens: 1_000,
        max_physical_model_attempts: 8,
        terminal_token_reserve: 100,
        terminal_physical_model_attempt_reserve: 1,
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
    assert_eq!(
        budget.max_physical_model_attempts,
        budget
            .max_model_calls
            .saturating_mul(crate::PHYSICAL_MODEL_ATTEMPTS_PER_LOGICAL_CALL)
    );
    assert_eq!(
        budget.max_total_tokens,
        u64::try_from(budget.max_physical_model_attempts)
            .unwrap_or(u64::MAX)
            .saturating_mul(crate::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT)
    );
    assert_eq!(
        budget.terminal_physical_model_attempt_reserve,
        budget
            .terminal_model_call_reserve
            .saturating_mul(crate::PHYSICAL_MODEL_ATTEMPTS_PER_LOGICAL_CALL)
    );
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
fn new_at_steer_epoch_starts_with_a_fully_applied_durable_objective() {
    let control = AgentRunControl::new_at_steer_epoch("pro", 7);

    assert_eq!(control.steer_epoch(), 7);
    assert!(control.pending_steers_snapshot().is_empty());
    assert!(control.begin_preparation());
    assert!(!control.commit_preparation(0));
    assert!(control.commit_preparation(7));
    assert!(matches!(
        control.execution_epoch_lease(),
        RunEpochLeaseOutcome::Acquired(lease) if lease.epoch() == 7
    ));

    assert_eq!(control.request_steer("next-objective"), Ok(true));
    assert_eq!(control.steer_epoch(), 8);
    assert_eq!(
        control
            .pending_steers_snapshot()
            .into_iter()
            .map(|steer| (steer.queue_id, steer.epoch))
            .collect::<Vec<_>>(),
        vec![("next-objective".to_string(), 8)]
    );
}

#[test]
fn objective_epoch_tool_start_is_atomic_during_preparation() {
    let control = AgentRunControl::new_at_steer_epoch("pro", 4);
    assert!(control.begin_preparation());
    assert!(matches!(
        control.begin_tool_call_at(4, "collaboration", "file.read", "a"),
        RunToolCallStart::Started(1)
    ));
    control.finish_tool_call();

    assert_eq!(control.request_steer("new-objective"), Ok(true));
    assert_eq!(
        control.begin_tool_call_at(4, "collaboration", "file.read", "b"),
        RunToolCallStart::RestartAfterSteer
    );
    assert_eq!(control.progress().tool_calls, 1);
    assert!(!control.objective_epoch_is_current(4));
}

#[test]
fn stale_call_finish_releases_activity_without_advancing_new_epoch_progress() {
    let control = AgentRunControl::new("pro");
    assert_eq!(control.begin_model_call_at(0, "old-model"), Ok(Some(1)));
    assert_eq!(
        control.begin_tool_call_at(0, "old-tool", "file.read", "old"),
        RunToolCallStart::Started(1)
    );
    assert_eq!(control.request_steer("new-objective"), Ok(true));
    assert!(control.acknowledge_pending_steer("new-objective"));
    assert!(control.mark_progress_at(1, "new-epoch", "new objective started"));

    assert!(!control.finish_model_call_at(0));
    assert!(!control.finish_tool_call_at(0));
    {
        let state = control.state.lock().expect("run control state should lock");
        assert_eq!(state.active_model_calls, 0);
        assert_eq!(state.active_tool_calls, 0);
    }
    let progress = control.progress();
    assert_eq!(progress.stage, "new-epoch");
    assert_eq!(progress.detail, "new objective started");
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
fn final_available_turn_is_reserved_for_terminal_commit_unless_progress_can_extend_it() {
    let mut budget = test_budget();
    budget.initial_agent_turns = 2;
    budget.max_agent_turns = 4;
    budget.terminal_model_call_reserve = 0;
    budget.terminal_time_reserve = Duration::ZERO;

    let control = AgentRunControl::with_budget(budget);
    assert_eq!(control.record_agent_turn("executor"), Ok(1));
    assert_eq!(
        control.continuation_directive(),
        RunContinuationDirective::CommitTerminalResult
    );

    let control = AgentRunControl::with_budget(budget);
    assert_eq!(control.record_agent_turn("executor"), Ok(1));
    assert!(control.record_checkpoint("tool_result", "evidence", "verified-result"));
    assert_eq!(
        control.continuation_directive(),
        RunContinuationDirective::Continue
    );
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
fn batch_tool_admission_reserves_ordered_calls_atomically() {
    let mut budget = test_budget();
    budget.initial_tool_calls = 8;
    budget.max_tool_calls = 8;
    let control = AgentRunControl::with_budget(budget);
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let calls = [("file.read", "a"), ("file.list", "b"), ("file.search", "c")];

    assert_eq!(
        control.begin_tool_call_batch_with_epoch(lease, "main", &calls),
        RunToolCallBatchStart::Started {
            first_call: 1,
            call_count: 3,
        }
    );
    assert_eq!(control.progress().tool_calls, 3);
    {
        let state = control.state.lock().expect("run control state should lock");
        assert_eq!(state.active_tool_calls, 3);
        assert_eq!(
            state.action_history.get("main"),
            Some(&(fingerprint(&calls[2]), 1))
        );
        assert_eq!(
            state
                .recent_actions
                .get("main")
                .expect("ordered action history should exist")
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            calls.iter().map(fingerprint).collect::<Vec<_>>()
        );
        assert_eq!(state.stage, "tool");
        assert_eq!(state.detail, "file.search");
    }

    for _ in &calls {
        assert!(control.finish_tool_call_at(lease.epoch()));
    }
    assert_eq!(
        control
            .state
            .lock()
            .expect("run control state should lock")
            .active_tool_calls,
        0
    );
}

#[test]
fn batch_tool_admission_without_budget_headroom_has_no_side_effects() {
    let control = AgentRunControl::with_budget(test_budget());
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let before = {
        let state = control.state.lock().expect("run control state should lock");
        (
            state.action_history.clone(),
            state.recent_actions.clone(),
            state.active_tool_calls,
            state.stage.clone(),
            state.detail.clone(),
        )
    };

    assert_eq!(
        control.begin_tool_call_batch_with_epoch(
            lease,
            "main",
            &[("file.read", "a"), ("file.read", "b"), ("file.read", "c"),],
        ),
        RunToolCallBatchStart::SerialRequired
    );
    assert_eq!(control.progress().tool_calls, 0);
    assert_eq!(control.stop_reason(), None);
    let after = {
        let state = control.state.lock().expect("run control state should lock");
        (
            state.action_history.clone(),
            state.recent_actions.clone(),
            state.active_tool_calls,
            state.stage.clone(),
            state.detail.clone(),
        )
    };
    assert_eq!(after, before);

    assert_eq!(
        control.begin_tool_call_batch_with_epoch(
            lease,
            "main",
            &[("file.read", "a"), ("file.read", "b")],
        ),
        RunToolCallBatchStart::Started {
            first_call: 1,
            call_count: 2,
        }
    );
    control.finish_tool_call();
    control.finish_tool_call();
}

#[test]
fn batch_tool_admission_repeated_action_fallback_has_no_side_effects() {
    let mut budget = test_budget();
    budget.initial_tool_calls = 10;
    budget.max_tool_calls = 10;
    let control = AgentRunControl::with_budget(budget);
    assert_eq!(control.begin_tool_call("main", "file.read", "a"), Ok(1));
    control.finish_tool_call();
    assert_eq!(control.begin_tool_call("main", "file.read", "a"), Ok(2));
    control.finish_tool_call();
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let before = {
        let state = control.state.lock().expect("run control state should lock");
        (
            state.action_history.clone(),
            state.recent_actions.clone(),
            state.active_tool_calls,
        )
    };

    assert_eq!(
        control.begin_tool_call_batch_with_epoch(
            lease,
            "main",
            &[("file.read", "a"), ("file.read", "b")],
        ),
        RunToolCallBatchStart::SerialRequired
    );
    assert_eq!(control.progress().tool_calls, 2);
    assert_eq!(control.stop_reason(), None);
    let state = control.state.lock().expect("run control state should lock");
    assert_eq!(
        (
            state.action_history.clone(),
            state.recent_actions.clone(),
            state.active_tool_calls,
        ),
        before
    );
}

#[test]
fn batch_tool_admission_cycle_fallback_has_no_side_effects() {
    let mut budget = test_budget();
    budget.initial_tool_calls = 12;
    budget.max_tool_calls = 12;
    let control = AgentRunControl::with_budget(budget);
    for input in ["a", "b", "a", "b", "a"] {
        assert!(control.begin_tool_call("main", "file.read", input).is_ok());
        control.finish_tool_call();
    }
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let before = {
        let state = control.state.lock().expect("run control state should lock");
        (
            state.action_history.clone(),
            state.recent_actions.clone(),
            state.active_tool_calls,
        )
    };

    assert_eq!(
        control.begin_tool_call_batch_with_epoch(
            lease,
            "main",
            &[("file.read", "b"), ("file.read", "c")],
        ),
        RunToolCallBatchStart::SerialRequired
    );
    assert_eq!(control.progress().tool_calls, 5);
    assert_eq!(control.stop_reason(), None);
    let state = control.state.lock().expect("run control state should lock");
    assert_eq!(
        (
            state.action_history.clone(),
            state.recent_actions.clone(),
            state.active_tool_calls,
        ),
        before
    );
}

#[test]
fn batch_tool_admission_preserves_stale_stop_and_terminal_semantics() {
    let stale_control = AgentRunControl::new("pro");
    let stale_lease = match stale_control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    assert_eq!(stale_control.request_steer("new-objective"), Ok(true));
    assert!(matches!(
        stale_control
            .commit_pending_steers_with(|_| Ok::<_, ()>(()))
            .expect("steer application should succeed"),
        RunSteerBatchCommit::Committed { .. }
    ));
    assert_eq!(
        stale_control.begin_tool_call_batch_with_epoch(
            stale_lease,
            "main",
            &[("file.read", "a"), ("file.read", "b")],
        ),
        RunToolCallBatchStart::RestartAfterSteer
    );
    assert_eq!(stale_control.progress().tool_calls, 0);

    let stopped_control = AgentRunControl::new("pro");
    let stopped_lease = match stopped_control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    stopped_control.request_stop(RunStopReason::ProviderUnavailable);
    assert_eq!(
        stopped_control.begin_tool_call_batch_with_epoch(
            stopped_lease,
            "main",
            &[("file.read", "a"), ("file.read", "b")],
        ),
        RunToolCallBatchStart::Stopped(RunStopReason::ProviderUnavailable)
    );
    assert_eq!(stopped_control.progress().tool_calls, 0);

    let terminal_control = AgentRunControl::new("pro");
    let terminal_lease = match terminal_control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    assert_eq!(
        terminal_control.commit_terminal_result_with(terminal_lease, || Ok::<_, ()>(())),
        Ok(RunTerminalCommit::Committed(()))
    );
    assert_eq!(
        terminal_control.begin_tool_call_batch_with_epoch(
            terminal_lease,
            "main",
            &[("file.read", "a"), ("file.read", "b")],
        ),
        RunToolCallBatchStart::TerminalCommitted
    );
    assert_eq!(terminal_control.progress().tool_calls, 0);
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
    assert!(continued.partial_output().is_empty());
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
fn physical_attempt_usage_is_reserved_then_reconciled() {
    let mut budget = test_budget();
    budget.max_total_tokens = 100;
    budget.max_physical_model_attempts = 4;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    let control = AgentRunControl::with_budget(budget);

    let attempt = control
        .begin_physical_model_attempt("model-a", 40, 20, RunStageClass::Worker)
        .expect("attempt should reserve resources");
    assert_eq!(attempt.model(), "model-a");
    assert_eq!(attempt.reserved_tokens(), 60);
    let reserved = control.resource_usage();
    assert_eq!(reserved.segment.physical_attempts, 1);
    assert_eq!(reserved.segment.reserved_tokens, 60);
    assert_eq!(reserved.segment.total_tokens, 0);

    assert!(control.finish_physical_model_attempt(
        attempt,
        Some(crate::ModelAttemptUsage::new(
            30,
            10,
            40,
            crate::ModelUsageSource::Provider,
        )),
    ));
    let settled = control.resource_usage();
    assert_eq!(settled.segment.reserved_tokens, 0);
    assert_eq!(settled.segment.prompt_tokens, 30);
    assert_eq!(settled.segment.completion_tokens, 10);
    assert_eq!(settled.segment.total_tokens, 40);
    assert_eq!(settled.segment.usage_sources.provider, 1);
    assert_eq!(settled.segment, settled.lineage);
}

#[test]
fn unknown_attempt_usage_commits_the_conservative_reservation() {
    let mut budget = test_budget();
    budget.max_total_tokens = 100;
    budget.max_physical_model_attempts = 2;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    let control = AgentRunControl::with_budget(budget);
    let attempt = control
        .begin_physical_model_attempt("model-a", 15, 10, RunStageClass::Worker)
        .expect("attempt should reserve resources");

    assert!(control.finish_physical_model_attempt(attempt, None));
    let usage = control.resource_usage().segment;
    assert_eq!(usage.total_tokens, 25);
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.completion_tokens, 0);
    assert_eq!(usage.usage_sources.unknown, 1);
    assert_eq!(
        usage.usage_sources.least_complete(),
        Some(crate::ModelUsageSource::Unknown)
    );
}

#[test]
fn concurrent_physical_attempt_reservations_cannot_oversubscribe() {
    let mut budget = test_budget();
    budget.max_total_tokens = 10;
    budget.max_physical_model_attempts = 2;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    let control = Arc::new(AgentRunControl::with_budget(budget));
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2)
        .map(|index| {
            let control = Arc::clone(&control);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                control.begin_physical_model_attempt(
                    &format!("model-{index}"),
                    6,
                    0,
                    RunStageClass::Finalizer,
                )
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("reservation thread should join"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| { matches!(result, Err(RunStopReason::ModelResourceBudgetExceeded)) })
            .count(),
        1
    );
    assert_eq!(control.resource_usage().segment.reserved_tokens, 6);
    for attempt in results.into_iter().flatten() {
        assert!(control.finish_physical_model_attempt(attempt, None));
    }
}

#[test]
fn terminal_attempt_and_token_reserves_remain_available_to_finalizer() {
    let mut budget = test_budget();
    budget.max_total_tokens = 100;
    budget.max_physical_model_attempts = 2;
    budget.terminal_token_reserve = 30;
    budget.terminal_physical_model_attempt_reserve = 1;
    let control = AgentRunControl::with_budget(budget);

    let worker = control
        .begin_physical_model_attempt("worker", 70, 0, RunStageClass::Worker)
        .expect("worker should use only the unprotected allocation");
    assert_eq!(
        control.begin_physical_model_attempt("worker", 31, 0, RunStageClass::Worker),
        Err(RunStopReason::StageBudgetExhausted)
    );
    assert_eq!(control.stop_reason(), None);
    let finalizer = control
        .begin_physical_model_attempt("finalizer", 30, 0, RunStageClass::Finalizer)
        .expect("finalizer should own the terminal reserve");
    assert_eq!(control.resource_usage().segment.reserved_tokens, 100);

    assert!(control.finish_physical_model_attempt(worker, None));
    assert!(control.finish_physical_model_attempt(finalizer, None));
    assert_eq!(control.resource_usage().segment.total_tokens, 100);
    assert_eq!(
        control.begin_physical_model_attempt("finalizer", 0, 0, RunStageClass::Finalizer),
        Err(RunStopReason::ModelResourceBudgetExceeded)
    );
}

#[test]
fn normal_snapshot_preserves_segment_and_lineage_resources() {
    let mut budget = test_budget();
    budget.max_total_tokens = 100;
    budget.max_physical_model_attempts = 4;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    let control = AgentRunControl::with_budget(budget);
    let attempt = control
        .begin_physical_model_attempt("model-a", 20, 10, RunStageClass::Worker)
        .expect("attempt should start");
    assert!(control.finish_physical_model_attempt(
        attempt,
        Some(crate::ModelAttemptUsage::new(
            12,
            4,
            16,
            crate::ModelUsageSource::Estimated,
        )),
    ));

    let expected = control.resource_usage();
    let resumed = AgentRunControl::from_snapshot(control.snapshot());
    assert_eq!(resumed.resource_usage(), expected);
}

#[test]
fn durable_resource_restore_conservatively_settles_inflight_attempts() {
    let control = AgentRunControl::new("fast");
    let _attempt = control
        .begin_physical_model_attempt("model-a", 12, 8, RunStageClass::Worker)
        .expect("attempt should reserve resources");

    let restored = AgentRunControl::new_at_steer_epoch_with_resource_snapshot(
        "fast",
        7,
        control.resource_usage(),
    );
    assert_eq!(restored.steer_epoch(), 7);
    let resources = restored.resource_usage();
    assert_eq!(resources.segment.reserved_tokens, 0);
    assert_eq!(resources.segment.total_tokens, 20);
    assert_eq!(resources.segment.usage_sources.unknown, 1);
    assert_eq!(resources.segment, resources.lineage);
}

#[test]
fn explicit_continuation_resets_segment_but_preserves_lineage_resources() {
    let mut budget = test_budget();
    budget.max_total_tokens = 100;
    budget.max_physical_model_attempts = 4;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    let control = AgentRunControl::with_budget(budget);
    let attempt = control
        .begin_physical_model_attempt("model-a", 20, 0, RunStageClass::Worker)
        .expect("attempt should start");
    assert!(control.finish_physical_model_attempt(attempt, None));
    control.request_stop(RunStopReason::ModelCallBudgetExceeded);

    let continued = AgentRunControl::from_snapshot_for_continuation(control.snapshot())
        .expect("explicit continuation should start a fresh segment");
    let resources = continued.resource_usage();
    assert_eq!(resources.segment, crate::RunResourceUsage::default());
    assert_eq!(resources.lineage.physical_attempts, 1);
    assert_eq!(resources.lineage.total_tokens, 20);
    let next = continued
        .begin_physical_model_attempt("model-b", 10, 0, RunStageClass::Worker)
        .expect("new segment should have a fresh allocation");
    assert!(continued.finish_physical_model_attempt(next, None));
    let resources = continued.resource_usage();
    assert_eq!(resources.segment.total_tokens, 10);
    assert_eq!(resources.lineage.total_tokens, 30);
    assert_eq!(resources.lineage.physical_attempts, 2);
}

#[test]
fn durable_continuation_charges_inflight_attempt_to_lineage_before_resetting_segment() {
    let control = AgentRunControl::new("fast");
    let _attempt = control
        .begin_physical_model_attempt("model-a", 12, 8, RunStageClass::Worker)
        .expect("attempt should reserve resources");

    let continued = AgentRunControl::new_for_continuation_at_steer_epoch_with_resource_snapshot(
        "fast",
        4,
        control.resource_usage(),
    );
    let resources = continued.resource_usage();
    assert_eq!(continued.steer_epoch(), 4);
    assert_eq!(resources.segment, crate::RunResourceUsage::default());
    assert_eq!(resources.lineage.reserved_tokens, 0);
    assert_eq!(resources.lineage.total_tokens, 20);
    assert_eq!(resources.lineage.usage_sources.unknown, 1);
}

#[test]
fn resource_ledger_bounds_model_cardinality() {
    let mut budget = test_budget();
    budget.max_total_tokens = 1_000;
    budget.max_physical_model_attempts = 64;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    let control = AgentRunControl::with_budget(budget);

    for index in 0..40 {
        let attempt = control
            .begin_physical_model_attempt(&format!("model-{index}"), 1, 0, RunStageClass::Finalizer)
            .expect("bounded model ledger should not reject valid attempts");
        assert!(control.finish_physical_model_attempt(
            attempt,
            Some(crate::ModelAttemptUsage::new(
                1,
                0,
                1,
                crate::ModelUsageSource::ProviderPartial,
            )),
        ));
    }

    let usage = control.resource_usage();
    assert_eq!(
        usage.segment.models.len(),
        crate::MAX_RESOURCE_LEDGER_MODELS
    );
    assert_eq!(
        usage.lineage.models.len(),
        crate::MAX_RESOURCE_LEDGER_MODELS
    );
    assert_eq!(usage.segment.physical_attempts, 40);
    assert_eq!(usage.segment.usage_sources.provider_partial, 40);
}

#[test]
fn resource_accounting_uses_saturating_arithmetic() {
    let mut budget = test_budget();
    budget.max_total_tokens = u64::MAX;
    budget.max_physical_model_attempts = usize::MAX;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    let control = AgentRunControl::with_budget(budget);
    let attempt = control
        .begin_physical_model_attempt(
            "model-a",
            u64::MAX.saturating_sub(1),
            10,
            RunStageClass::Finalizer,
        )
        .expect("saturated exact-boundary reservation should be admitted");
    assert_eq!(attempt.reserved_tokens(), u64::MAX);
    assert!(control.finish_physical_model_attempt(
        attempt,
        Some(crate::ModelAttemptUsage::new(
            u64::MAX,
            u64::MAX,
            0,
            crate::ModelUsageSource::Estimated,
        )),
    ));
    let usage = control.resource_usage().segment;
    assert_eq!(usage.prompt_tokens, u64::MAX);
    assert_eq!(usage.completion_tokens, u64::MAX);
    assert_eq!(usage.total_tokens, u64::MAX);
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
fn pending_steer_snapshot_does_not_consume_the_queue() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.request_steer("queue-a"), Ok(true));
    assert_eq!(control.request_steer("queue-b"), Ok(true));

    let queue_ids = || {
        control
            .pending_steers_snapshot()
            .into_iter()
            .map(|steer| steer.queue_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(queue_ids(), vec!["queue-a", "queue-b"]);
    assert_eq!(queue_ids(), vec!["queue-a", "queue-b"]);
    assert!(control.has_pending_steer());
}

#[test]
fn unacknowledged_pending_steer_is_not_lost() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.request_steer("queue-a"), Ok(true));

    let _work_snapshot = control.pending_steers_snapshot();

    assert_eq!(
        control
            .take_pending_steers()
            .into_iter()
            .map(|steer| steer.queue_id)
            .collect::<Vec<_>>(),
        vec!["queue-a"]
    );
}

#[test]
fn acknowledging_one_pending_steer_preserves_the_remaining_order() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.request_steer("queue-a"), Ok(true));
    assert_eq!(control.request_steer("queue-b"), Ok(true));
    assert_eq!(control.request_steer("queue-c"), Ok(true));

    assert!(control.acknowledge_pending_steer("queue-b"));
    assert_eq!(
        control
            .pending_steers_snapshot()
            .into_iter()
            .map(|steer| steer.queue_id)
            .collect::<Vec<_>>(),
        vec!["queue-a", "queue-c"]
    );
}

#[test]
fn acknowledging_a_pending_steer_is_idempotent() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.request_steer("queue-a"), Ok(true));
    assert_eq!(control.request_steer("queue-b"), Ok(true));

    assert!(control.acknowledge_pending_steer("queue-a"));
    assert!(!control.acknowledge_pending_steer("queue-a"));
    assert_eq!(
        control
            .pending_steers_snapshot()
            .into_iter()
            .map(|steer| steer.queue_id)
            .collect::<Vec<_>>(),
        vec!["queue-b"]
    );
}

#[test]
fn preparation_commit_linearizes_against_new_steers() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.steer_epoch(), 0);
    assert!(control.begin_preparation());
    assert!(control.commit_preparation(0));

    assert_eq!(control.request_steer("queue-after-commit"), Ok(true));
    assert_eq!(control.steer_epoch(), 1);
    assert!(!control.commit_preparation(0));
    assert!(matches!(
        control.execution_epoch_lease(),
        RunEpochLeaseOutcome::RestartAfterSteer
    ));
    assert!(control.acknowledge_pending_steer("queue-after-commit"));
    assert!(matches!(
        control.execution_epoch_lease(),
        RunEpochLeaseOutcome::Acquired(lease) if lease.epoch() == 1
    ));

    assert!(control.begin_preparation());
    assert!(control.commit_preparation(1));

    assert_eq!(control.request_steer("queue-next"), Ok(true));
    assert_eq!(control.steer_epoch(), 2);
    assert!(!control.commit_preparation(1));
    assert_eq!(
        control
            .pending_steers_snapshot()
            .into_iter()
            .map(|steer| (steer.queue_id, steer.epoch))
            .collect::<Vec<_>>(),
        vec![("queue-next".to_string(), 2)]
    );
}

#[test]
fn terminal_commit_rejects_a_stale_response_after_the_steer_is_applied() {
    let control = AgentRunControl::new("pro");
    let stale_lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("initial execution lease unavailable: {outcome:?}"),
    };
    assert_eq!(control.request_steer("new-objective"), Ok(true));
    assert!(control.acknowledge_pending_steer("new-objective"));

    let mut stale_commit_ran = false;
    let stale = control
        .commit_terminal_result_with(stale_lease, || {
            stale_commit_ran = true;
            Ok::<_, ()>("stale")
        })
        .expect("terminal arbitration should not fail");
    assert_eq!(stale, RunTerminalCommit::RestartAfterSteer);
    assert!(!stale_commit_ran);

    let current_lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("current execution lease unavailable: {outcome:?}"),
    };
    assert_eq!(current_lease.epoch(), 1);
    assert_eq!(
        control
            .commit_terminal_result_with(current_lease, || Ok::<_, ()>("current"))
            .expect("current terminal commit should succeed"),
        RunTerminalCommit::Committed("current")
    );
    assert_eq!(control.request_steer("too-late"), Ok(false));
    assert!(!control.request_cancel());
    assert_eq!(control.stop_reason(), None);
}

#[test]
fn response_step_commit_restarts_without_persisting_after_steer_wins() {
    let control = AgentRunControl::new("pro");
    let stale_lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("initial execution lease unavailable: {outcome:?}"),
    };
    assert_eq!(control.request_steer("new-response-objective"), Ok(true));
    assert!(matches!(
        control
            .commit_pending_steers_with(|_| Ok::<_, ()>(()))
            .expect("steer application should succeed"),
        RunSteerBatchCommit::Committed { .. }
    ));

    let mut persistence_ran = false;
    let committed = control
        .commit_execution_step_with(stale_lease, || {
            persistence_ran = true;
            Ok::<_, ()>(())
        })
        .expect("response arbitration should not fail");

    assert_eq!(committed, RunExecutionStepCommit::RestartAfterSteer);
    assert!(!persistence_ran);
}

#[test]
fn stale_epoch_cannot_start_a_tool_call_after_steer_wins() {
    let control = AgentRunControl::new("pro");
    let stale_lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("initial execution lease unavailable: {outcome:?}"),
    };
    assert_eq!(control.request_steer("new-tool-objective"), Ok(true));
    assert!(matches!(
        control
            .commit_pending_steers_with(|_| Ok::<_, ()>(()))
            .expect("steer application should succeed"),
        RunSteerBatchCommit::Committed { .. }
    ));

    assert_eq!(
        control.begin_tool_call_with_epoch(stale_lease, "main", "file.read", "README.md"),
        RunToolCallStart::RestartAfterSteer
    );
    assert_eq!(control.progress().tool_calls, 0);

    let current_lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("current execution lease unavailable: {outcome:?}"),
    };
    assert!(matches!(
        control.begin_tool_call_with_epoch(current_lease, "main", "file.read", "README.md"),
        RunToolCallStart::Started(1)
    ));
    control.finish_tool_call();
}

#[test]
fn stale_epoch_cannot_consume_model_turn_or_repair_budgets() {
    let control = AgentRunControl::with_budget(test_budget());
    assert_eq!(control.request_steer("new-budget-objective"), Ok(true));
    assert!(matches!(
        control
            .commit_pending_steers_with(|_| Ok::<_, ()>(()))
            .expect("steer application should succeed"),
        RunSteerBatchCommit::Committed { .. }
    ));

    assert_eq!(
        control.begin_stage_model_call_at(0, "old-worker", RunStageClass::Worker),
        Ok(None)
    );
    assert_eq!(control.record_agent_turn_at(0, "old-worker"), Ok(None));
    assert_eq!(
        control.begin_repair_attempt_at(0, "old-worker-repair"),
        Ok(None)
    );
    assert!(!control.mark_progress_at(0, "old-worker", "stale progress"));
    let stale_progress = control.progress();
    assert_eq!(stale_progress.model_calls, 0);
    assert_eq!(stale_progress.agent_turns, 0);
    assert_eq!(stale_progress.repair_attempts, 0);
    assert_eq!(control.stop_reason(), None);

    assert_eq!(
        control.begin_stage_model_call_at(1, "new-worker", RunStageClass::Worker),
        Ok(Some(1))
    );
    control.finish_model_call();
    assert_eq!(control.record_agent_turn_at(1, "new-worker"), Ok(Some(1)));
    assert_eq!(
        control.begin_repair_attempt_at(1, "new-worker-repair"),
        Ok(Some(1))
    );
}

#[test]
fn delayed_old_epoch_worker_cannot_write_after_durable_steer_application() {
    let control = Arc::new(AgentRunControl::new("pro"));
    let old_lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("initial execution lease unavailable: {outcome:?}"),
    };
    let ready = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let worker_control = Arc::clone(&control);
    let worker_ready = Arc::clone(&ready);
    let worker_release = Arc::clone(&release);
    let worker = thread::spawn(move || {
        worker_ready.wait();
        worker_release.wait();
        (
            worker_control.record_partial_output_at(old_lease.epoch(), "stale partial"),
            worker_control.record_best_known_result_at(
                old_lease.epoch(),
                "stale-worker",
                "stale result",
                ResultQuality::Verified,
                1,
                true,
                true,
            ),
        )
    });

    ready.wait();
    assert_eq!(control.request_steer("durable-new-objective"), Ok(true));
    assert!(matches!(
        control
            .commit_pending_steers_with(|pending| Ok::<_, ()>(pending.len()))
            .expect("durable steer application should succeed"),
        RunSteerBatchCommit::Committed { value: 1, .. }
    ));
    release.wait();

    assert_eq!(worker.join().expect("worker should join"), (false, false));
    assert!(control.partial_output().is_empty());
    assert!(control.result_frontier().is_empty());
    assert!(matches!(
        control.execution_epoch_lease(),
        RunEpochLeaseOutcome::Acquired(lease) if lease.epoch() == 1
    ));
}

#[test]
fn terminal_commit_and_steer_have_one_linearization_order() {
    let control = Arc::new(AgentRunControl::new("pro"));
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let commit_control = Arc::clone(&control);
    let commit = thread::spawn(move || {
        commit_control
            .commit_terminal_result_with(lease, || {
                entered_tx.send(()).expect("entry signal should send");
                release_rx.recv().expect("commit should be released");
                Ok::<_, ()>(())
            })
            .expect("terminal commit should not fail")
    });
    entered_rx.recv().expect("terminal commit should enter");

    let steer_control = Arc::clone(&control);
    let steer = thread::spawn(move || steer_control.request_steer("racing-steer"));
    release_tx.send(()).expect("terminal commit should release");

    assert_eq!(
        commit.join().expect("commit thread should join"),
        RunTerminalCommit::Committed(())
    );
    assert_eq!(steer.join().expect("steer thread should join"), Ok(false));
    assert_eq!(control.steer_epoch(), 0);
}

#[test]
fn failed_steer_persistence_does_not_publish_an_epoch() {
    let control = AgentRunControl::new("pro");

    let result =
        control.commit_steer_request_with("not-durable", || Err::<(), _>("storage failed"));

    assert_eq!(result, Err("storage failed"));
    assert_eq!(control.steer_epoch(), 0);
    assert!(control.pending_steers_snapshot().is_empty());
}

#[test]
fn full_steer_queue_rejects_without_persisting_or_dropping_an_accepted_request() {
    let control = AgentRunControl::new("pro");
    for index in 0..16 {
        let result = control
            .commit_steer_request_with(format!("steer-{index}"), || Ok::<_, ()>(()))
            .expect("steer request should arbitrate");
        assert!(matches!(result, RunSteerRequestCommit::Committed { .. }));
    }
    let accepted_before = control.pending_steers_snapshot();
    let mut persisted = false;

    let rejected = control
        .commit_steer_request_with("steer-over-capacity", || {
            persisted = true;
            Ok::<_, ()>(())
        })
        .expect("capacity rejection should arbitrate");

    assert_eq!(rejected, RunSteerRequestCommit::CapacityReached);
    assert!(!persisted);
    assert_eq!(control.steer_epoch(), 16);
    assert_eq!(control.pending_steers_snapshot(), accepted_before);
}

#[test]
fn durable_steer_commit_wins_before_terminal_commit() {
    let control = Arc::new(AgentRunControl::new("pro"));
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let steer_control = Arc::clone(&control);
    let steer = thread::spawn(move || {
        steer_control
            .commit_steer_request_with("durable-first", || {
                entered_tx.send(()).expect("entry signal should send");
                release_rx.recv().expect("steer commit should release");
                Ok::<_, ()>("persisted")
            })
            .expect("steer persistence should succeed")
    });
    entered_rx.recv().expect("steer commit should enter");

    let terminal_control = Arc::clone(&control);
    let terminal = thread::spawn(move || {
        terminal_control
            .commit_terminal_result_with(lease, || Ok::<_, ()>(()))
            .expect("terminal arbitration should succeed")
    });
    release_tx.send(()).expect("steer commit should release");

    assert_eq!(
        steer.join().expect("steer thread should join"),
        RunSteerRequestCommit::Committed {
            value: "persisted",
            steer: RunSteer {
                queue_id: "durable-first".to_string(),
                epoch: 1,
            },
        }
    );
    assert_eq!(
        terminal.join().expect("terminal thread should join"),
        RunTerminalCommit::RestartAfterSteer
    );
}

#[test]
fn terminal_commit_wins_without_persisting_a_late_steer() {
    let control = Arc::new(AgentRunControl::new("pro"));
    let lease = match control.execution_epoch_lease() {
        RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("execution lease unavailable: {outcome:?}"),
    };
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let terminal_control = Arc::clone(&control);
    let terminal = thread::spawn(move || {
        terminal_control
            .commit_terminal_result_with(lease, || {
                entered_tx.send(()).expect("entry signal should send");
                release_rx.recv().expect("terminal commit should release");
                Ok::<_, ()>(())
            })
            .expect("terminal arbitration should succeed")
    });
    entered_rx.recv().expect("terminal commit should enter");

    let (persisted_tx, persisted_rx) = mpsc::channel();
    let steer_control = Arc::clone(&control);
    let steer = thread::spawn(move || {
        steer_control
            .commit_steer_request_with("too-late", || {
                persisted_tx
                    .send(())
                    .expect("persistence signal should send");
                Ok::<_, ()>(())
            })
            .expect("steer arbitration should succeed")
    });
    release_tx.send(()).expect("terminal commit should release");

    assert_eq!(
        terminal.join().expect("terminal thread should join"),
        RunTerminalCommit::Committed(())
    );
    assert_eq!(
        steer.join().expect("steer thread should join"),
        RunSteerRequestCommit::TerminalCommitted
    );
    assert!(persisted_rx.try_recv().is_err());
    assert_eq!(control.steer_epoch(), 0);
    assert!(control.pending_steers_snapshot().is_empty());
}

#[test]
fn preparation_commit_holds_the_handoff_until_persistence_finishes() {
    let control = Arc::new(AgentRunControl::new("pro"));
    assert!(control.begin_preparation());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let commit_control = Arc::clone(&control);
    let commit = thread::spawn(move || {
        commit_control
            .commit_preparation_with(0, || {
                entered_tx.send(()).expect("entry signal should send");
                release_rx.recv().expect("preparation should be released");
                Ok::<_, ()>("persisted")
            })
            .expect("preparation commit should not fail")
    });
    entered_rx.recv().expect("preparation commit should enter");

    let steer_control = Arc::clone(&control);
    let steer = thread::spawn(move || steer_control.request_steer("after-handoff"));
    release_tx
        .send(())
        .expect("preparation commit should release");

    assert_eq!(
        commit.join().expect("commit thread should join"),
        RunPreparationCommit::Committed {
            value: "persisted",
            lease: RunEpochLease { epoch: 0 },
        }
    );
    assert_eq!(steer.join().expect("steer thread should join"), Ok(true));
    assert!(matches!(
        control.execution_epoch_lease(),
        RunEpochLeaseOutcome::RestartAfterSteer
    ));
}

#[test]
fn pending_steer_commit_is_atomic_with_user_cancellation() {
    let control = Arc::new(AgentRunControl::new("pro"));
    assert_eq!(control.request_steer("durable-steer"), Ok(true));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let commit_control = Arc::clone(&control);
    let commit = thread::spawn(move || {
        commit_control
            .commit_pending_steers_with(|pending| {
                assert_eq!(pending.len(), 1);
                entered_tx.send(()).expect("entry signal should send");
                release_rx.recv().expect("steer commit should be released");
                Ok::<_, ()>(pending[0].queue_id.clone())
            })
            .expect("steer commit should not fail")
    });
    entered_rx.recv().expect("steer commit should enter");

    let cancel_control = Arc::clone(&control);
    let cancel = thread::spawn(move || cancel_control.request_cancel());
    release_tx.send(()).expect("steer commit should release");

    assert_eq!(
        commit.join().expect("commit thread should join"),
        RunSteerBatchCommit::Committed {
            value: "durable-steer".to_string(),
            steers: vec![RunSteer {
                queue_id: "durable-steer".to_string(),
                epoch: 1,
            }],
        }
    );
    cancel.join().expect("cancel thread should join");
    assert!(!control.has_pending_steer());
    assert_eq!(control.stop_reason(), Some(RunStopReason::UserCancelled));
}

#[test]
fn stopped_or_failed_steer_commits_leave_the_pending_batch_untouched() {
    let control = AgentRunControl::new("pro");
    assert_eq!(control.request_steer("retry-me"), Ok(true));
    let failed = control.commit_pending_steers_with(|_| Err::<(), _>("storage failed"));
    assert_eq!(failed, Err("storage failed"));
    assert_eq!(control.pending_steers_snapshot().len(), 1);

    control.request_cancel();
    let mut commit_ran = false;
    let stopped = control
        .commit_pending_steers_with(|_| {
            commit_ran = true;
            Ok::<_, ()>(())
        })
        .expect("stop-first arbitration should not fail");
    assert_eq!(
        stopped,
        RunSteerBatchCommit::Stopped(RunStopReason::UserCancelled)
    );
    assert!(!commit_ran);
    assert_eq!(control.pending_steers_snapshot().len(), 1);
}

#[test]
fn steering_invalidates_old_partial_output_and_result_frontier() {
    let control = AgentRunControl::with_budget(test_budget());
    assert!(control.record_partial_output_at(0, "old partial"));
    assert!(control.record_best_known_result_at(
        0,
        "old-stage",
        "old result",
        ResultQuality::Draft,
        0,
        false,
        false,
    ));
    assert_eq!(control.partial_output(), "old partial");
    assert_eq!(control.result_frontier().len(), 1);

    assert_eq!(control.request_steer("new-objective"), Ok(true));
    assert!(control.partial_output().is_empty());
    assert!(control.result_frontier().is_empty());
    assert!(!control.record_partial_output_at(0, "stale partial"));
    assert!(!control.record_best_known_result_at(
        0,
        "stale-stage",
        "stale result",
        ResultQuality::Verified,
        1,
        true,
        true,
    ));

    assert!(control.acknowledge_pending_steer("new-objective"));
    assert!(control.record_partial_output_at(1, "current partial"));
    assert!(control.record_best_known_result_at(
        1,
        "current-stage",
        "current result",
        ResultQuality::Draft,
        0,
        false,
        false,
    ));
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
    assert_eq!(
        control.begin_stage_model_call("synthesis", RunStageClass::Synthesizer),
        Err(RunStopReason::StageBudgetExhausted)
    );

    for stage in ["delivery_1", "delivery_2", "delivery_3", "delivery_4"] {
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

    let internal_terminal = control.stage_model_call_timeout(RunStageClass::Synthesizer);
    assert!(internal_terminal <= Duration::from_secs(40));
    assert!(internal_terminal > Duration::from_secs(39));

    let finalizer = control.stage_model_call_timeout(RunStageClass::Finalizer);
    assert!(finalizer <= Duration::from_secs(60));
    assert!(finalizer > Duration::from_secs(59));
}

#[test]
fn finalizer_timeout_reserves_a_bounded_recovery_window() {
    let mut budget = test_budget();
    budget.max_duration = Duration::from_secs(180);
    budget.model_call_timeout = Duration::from_secs(180);
    budget.terminal_time_reserve = Duration::from_secs(180);
    let control = AgentRunControl::with_budget(budget);

    let primary = control.stage_model_call_timeout_with_recovery(
        RunStageClass::Finalizer,
        1,
        Duration::from_secs(60),
    );
    assert!(primary <= Duration::from_secs(120));
    assert!(primary > Duration::from_secs(119));

    let only_attempt = control.stage_model_call_timeout_with_recovery(
        RunStageClass::Finalizer,
        0,
        Duration::from_secs(60),
    );
    assert!(only_attempt <= Duration::from_secs(180));
    assert!(only_attempt > Duration::from_secs(179));
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
    assert!(control.stage_should_stop(RunStageClass::Synthesizer));
    assert!(!control.stage_should_stop(RunStageClass::Finalizer));
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
