use super::*;

impl AgentRunControl {
    /// Executes one durable steer batch with stop-first semantics. The closure
    /// runs under the control-state lock and therefore must not call back into
    /// this control. Failed closures leave the pending queue untouched.
    pub fn commit_pending_steers_with<T, E, F>(
        &self,
        commit: F,
    ) -> Result<RunSteerBatchCommit<T>, E>
    where
        F: FnOnce(&[RunSteer]) -> Result<T, E>,
    {
        self.commit_pending_steers_with_applied_objective(commit, |_| false)
    }

    /// Commits a steer batch and, when the durable result actually applied a
    /// new objective, atomically opens one bounded base execution segment for
    /// that objective. Failed, stopped, empty, deleted, and no-op batches must
    /// return `false` from the predicate and cannot mint execution headroom.
    pub fn commit_pending_steers_with_applied_objective<T, E, F, P>(
        &self,
        commit: F,
        objective_applied: P,
    ) -> Result<RunSteerBatchCommit<T>, E>
    where
        F: FnOnce(&[RunSteer]) -> Result<T, E>,
        P: FnOnce(&T) -> bool,
    {
        let mut state = self.state.lock().expect("run control state poisoned");
        if let Some(reason) = self.refresh_stop_reason_locked(&mut state, Instant::now()) {
            return Ok(RunSteerBatchCommit::Stopped(reason));
        }
        if state.phase == RunPhase::TerminalCommitted {
            return Ok(RunSteerBatchCommit::NoPending);
        }
        let steers = state.pending_steers.iter().cloned().collect::<Vec<_>>();
        if steers.is_empty() {
            return Ok(RunSteerBatchCommit::NoPending);
        }
        let value = commit(&steers)?;
        let objective_applied = objective_applied(&value);
        state.pending_steers.drain(..steers.len());
        if let Some(epoch) = steers.last().map(|steer| steer.epoch) {
            state.applied_steer_epoch = state.applied_steer_epoch.max(epoch);
        }
        if objective_applied {
            open_applied_objective_segment(&self.budget, &mut state, self);
        }
        Ok(RunSteerBatchCommit::Committed { value, steers })
    }
}

fn open_applied_objective_segment(
    budget: &RunBudget,
    state: &mut RunMutableState,
    control: &AgentRunControl,
) {
    let model_calls = control.model_calls.load(Ordering::SeqCst);
    let tool_calls = control.tool_calls.load(Ordering::SeqCst);
    let agent_turns = control.agent_turns.load(Ordering::SeqCst);

    state.model_call_limit = state
        .model_call_limit
        .max(model_calls.saturating_add(budget.initial_model_calls.max(1)))
        .min(budget.max_model_calls);
    state.tool_call_limit = state
        .tool_call_limit
        .max(tool_calls.saturating_add(budget.initial_tool_calls.max(1)))
        .min(budget.max_tool_calls);
    state.agent_turn_limit = state
        .agent_turn_limit
        .max(agent_turns.saturating_add(budget.initial_agent_turns.max(1)))
        .min(budget.max_agent_turns);

    // Goal evidence belongs to the objective that produced it. A newly
    // applied objective receives its bounded base segment, but cannot spend an
    // unused Goal Delta from the prior epoch as an additional extension.
    state.model_extension_goal_delta = state.goal_delta_count;
    state.tool_extension_goal_delta = state.goal_delta_count;
    state.agent_turn_extension_goal_delta = state.goal_delta_count;
    state.continuation_actions.clear();
    state.last_progress_at = Instant::now();
}
