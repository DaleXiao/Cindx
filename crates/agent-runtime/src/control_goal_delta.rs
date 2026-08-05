use super::*;
use crate::AgentGoalDelta;

impl AgentRunControl {
    /// Accepts task-contract progress only at the active objective epoch.
    /// Callers must invoke this after the matching runtime transition commits
    /// and its canonical outcome is durable or recoverable.
    pub fn record_goal_delta_at(&self, expected_epoch: u64, delta: &AgentGoalDelta) -> bool {
        let mut state = self.state.lock().expect("run control state poisoned");
        self.record_goal_delta_locked(&mut state, expected_epoch, delta)
    }

    /// Stages a recoverable control snapshot and commits it before publishing
    /// the same Goal Delta credit to the live control. The closure runs while
    /// the control lock is held and must not call back into this control.
    pub fn commit_goal_deltas_at_with<T, E, F>(
        &self,
        expected_epoch: u64,
        deltas: &[AgentGoalDelta],
        commit: F,
    ) -> Result<(T, usize), E>
    where
        F: FnOnce(&RunControlSnapshot) -> Result<T, E>,
    {
        let mut state = self.state.lock().expect("run control state poisoned");
        let staged = AgentRunControl::from_snapshot(self.snapshot_locked(&state));
        let staged_accepted = deltas
            .iter()
            .filter(|delta| staged.record_goal_delta_at(expected_epoch, delta))
            .count();
        let staged_snapshot = staged.snapshot();
        let value = commit(&staged_snapshot)?;
        let live_accepted = deltas
            .iter()
            .filter(|delta| self.record_goal_delta_locked(&mut state, expected_epoch, delta))
            .count();
        debug_assert_eq!(staged_accepted, live_accepted);
        Ok((value, live_accepted))
    }

    fn record_goal_delta_locked(
        &self,
        state: &mut RunMutableState,
        expected_epoch: u64,
        delta: &AgentGoalDelta,
    ) -> bool {
        if state.phase == RunPhase::TerminalCommitted
            || state.stop_reason.is_some()
            || self.cancellation_requested()
            || !state.pending_steers.is_empty()
            || state.applied_steer_epoch != expected_epoch
            || self.steer_epoch.load(Ordering::SeqCst) != expected_epoch
        {
            return false;
        }

        let receipts = delta
            .receipt_ids()
            .iter()
            .map(|receipt| fingerprint(&("goal_delta_receipt", receipt)))
            .collect::<Vec<_>>();
        if receipts
            .iter()
            .all(|receipt| state.goal_delta_fingerprints.contains(receipt))
        {
            return false;
        }
        state.goal_delta_fingerprints.extend(receipts);

        state.goal_delta_count = state.goal_delta_count.saturating_add(1);
        let evidence = fingerprint(&("goal_delta", delta.fingerprint()));
        if state.checkpoint_fingerprints.insert(evidence) {
            state.checkpoint_count = state.checkpoint_count.saturating_add(1);
        }
        state.stage = "goal_delta".to_string();
        state.detail = delta.kind_labels();
        state.last_progress_at = Instant::now();
        true
    }
}
