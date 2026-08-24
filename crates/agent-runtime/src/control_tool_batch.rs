use super::*;

impl AgentRunControl {
    /// Atomically reserves an ordered batch of tool calls for parallel execution.
    ///
    /// `permissionless_read_only` is the caller's certification that every call
    /// in the batch is a permissionless read-only tool invocation (the desktop
    /// tool runtime decides this through `ToolRegistry::permissionless_read_tool`).
    /// A certified read-only batch keeps every serial-path safety semantic — the
    /// batch still occupies tool budget, the steer epoch still gates admission,
    /// and the repeated-action guard still applies — but it no longer falls back
    /// to the serial path for the conservative checks: a pending continuation
    /// lease is honored inline exactly like the serial loop, a batch the current
    /// tool budget cannot cover is admitted with the serial path's
    /// checkpoint-driven extension when one is available, and a batch that still
    /// exceeds the budget or trips the repeated-action guard stops the run with
    /// the same reason the serial path would record instead of deferring the
    /// decision. An uncertified batch keeps the conservative behavior: any
    /// budget, repeated-action, or continuation risk returns `SerialRequired` so
    /// the caller re-runs the batch serially.
    ///
    /// Every successful reservation must eventually be paired with one
    /// `finish_tool_call_at` call per admitted call.
    pub fn begin_tool_call_batch_with_epoch(
        &self,
        lease: RunEpochLease,
        scope: &str,
        calls: &[(&str, &str)],
        permissionless_read_only: bool,
    ) -> RunToolCallBatchStart {
        let mut state = self.state.lock().expect("run control state poisoned");
        if let Some(reason) = self.refresh_stop_reason_locked(&mut state, Instant::now()) {
            return RunToolCallBatchStart::Stopped(reason);
        }
        if state.phase == RunPhase::TerminalCommitted {
            return RunToolCallBatchStart::TerminalCommitted;
        }
        if state.phase != RunPhase::Executing
            || !state.pending_steers.is_empty()
            || state.applied_steer_epoch != lease.epoch
            || self.steer_epoch.load(Ordering::SeqCst) != lease.epoch
        {
            return RunToolCallBatchStart::RestartAfterSteer;
        }
        if calls.len() < 2 {
            return RunToolCallBatchStart::SerialRequired;
        }
        let pending_continuation = state.continuation_actions.get(scope).copied();
        if pending_continuation.is_some() && !permissionless_read_only {
            return RunToolCallBatchStart::SerialRequired;
        }

        let current_calls = self.tool_calls.load(Ordering::SeqCst);
        let Some(last_call) = current_calls.checked_add(calls.len()) else {
            return RunToolCallBatchStart::SerialRequired;
        };
        if state.active_tool_calls.checked_add(calls.len()).is_none() {
            return RunToolCallBatchStart::SerialRequired;
        }
        if last_call > state.tool_call_limit {
            if !permissionless_read_only {
                return RunToolCallBatchStart::SerialRequired;
            }
            // The serial path would extend the budget once from recorded goal
            // progress and stop the run when no extension can cover the call.
            // A certified read-only batch applies the same rule atomically.
            if !extend_tool_budget_if_progressed(&self.budget, &mut state, last_call) {
                state.stop_reason = Some(RunStopReason::ToolCallBudgetExceeded);
                return RunToolCallBatchStart::Stopped(RunStopReason::ToolCallBudgetExceeded);
            }
        }

        let mut continuation_signature = pending_continuation;
        let mut action_history = state.action_history.get(scope).copied();
        let mut recent_actions = state.recent_actions.get(scope).cloned().unwrap_or_default();
        for (tool_name, input) in calls {
            let signature = fingerprint(&(*tool_name, *input));
            // A continuation lease lets the exact repeated action resume a
            // partial observation without consuming the repeat guard, exactly
            // like the serial path: a match keeps the lease, and the first
            // mismatch clears it for the rest of the batch.
            if let Some(leased) = continuation_signature {
                if leased == signature {
                    continue;
                }
                continuation_signature = None;
            }
            action_history = Some(match action_history {
                Some((previous, repetitions)) if previous == signature => {
                    let Some(repetitions) = repetitions.checked_add(1) else {
                        return RunToolCallBatchStart::SerialRequired;
                    };
                    (signature, repetitions)
                }
                _ => (signature, 1),
            });
            if action_history
                .is_some_and(|(_, repetitions)| repetitions > self.budget.max_identical_actions)
            {
                if permissionless_read_only {
                    state.stop_reason = Some(RunStopReason::RepeatedAction);
                    return RunToolCallBatchStart::Stopped(RunStopReason::RepeatedAction);
                }
                return RunToolCallBatchStart::SerialRequired;
            }

            recent_actions.push_back(signature);
            while recent_actions.len() > 24 {
                recent_actions.pop_front();
            }
            if has_repeated_action_cycle(&recent_actions, self.budget.max_identical_actions + 1) {
                if permissionless_read_only {
                    state.stop_reason = Some(RunStopReason::RepeatedAction);
                    return RunToolCallBatchStart::Stopped(RunStopReason::RepeatedAction);
                }
                return RunToolCallBatchStart::SerialRequired;
            }
        }

        let first_call = current_calls + 1;
        self.tool_calls.store(last_call, Ordering::SeqCst);
        if let Some(history) = action_history {
            state.action_history.insert(scope.to_string(), history);
        }
        state
            .recent_actions
            .insert(scope.to_string(), recent_actions);
        if pending_continuation.is_some() && continuation_signature.is_none() {
            state.continuation_actions.remove(scope);
        }
        state.active_tool_calls += calls.len();
        state.telemetry.begin_tool_call_batch(calls.len());
        state.stage = "tool".to_string();
        state.detail = calls
            .last()
            .map(|(tool_name, _)| (*tool_name).to_string())
            .unwrap_or_default();
        state.last_progress_at = Instant::now();
        RunToolCallBatchStart::Started {
            first_call,
            call_count: calls.len(),
        }
    }
}
