use super::*;

impl AgentRunControl {
    /// Atomically reserves an ordered batch of tool calls for parallel execution.
    ///
    /// This deliberately does not extend the tool budget or stop the run when the
    /// complete batch cannot be admitted. Callers must fall back to the existing
    /// serial path, which preserves checkpoint-driven budget extension and
    /// repeated-action behavior. Every successful reservation must eventually be
    /// paired with one `finish_tool_call_at` call.
    pub fn begin_tool_call_batch_with_epoch(
        &self,
        lease: RunEpochLease,
        scope: &str,
        calls: &[(&str, &str)],
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

        let current_calls = self.tool_calls.load(Ordering::SeqCst);
        let Some(last_call) = current_calls.checked_add(calls.len()) else {
            return RunToolCallBatchStart::SerialRequired;
        };
        if last_call > state.tool_call_limit
            || state.active_tool_calls.checked_add(calls.len()).is_none()
        {
            return RunToolCallBatchStart::SerialRequired;
        }

        let mut action_history = state.action_history.get(scope).copied();
        let mut recent_actions = state.recent_actions.get(scope).cloned().unwrap_or_default();
        for (tool_name, input) in calls {
            let signature = fingerprint(&(*tool_name, *input));
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
                return RunToolCallBatchStart::SerialRequired;
            }

            recent_actions.push_back(signature);
            while recent_actions.len() > 24 {
                recent_actions.pop_front();
            }
            if has_repeated_action_cycle(&recent_actions, self.budget.max_identical_actions + 1) {
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
        state.active_tool_calls += calls.len();
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
