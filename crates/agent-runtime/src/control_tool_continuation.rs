use super::*;
use agent_core::ToolEffectSemantics;

impl AgentRunControl {
    /// Records whether the exact action may be repeated to continue a partial,
    /// typed observation. Only read-only or idempotent tools are eligible; all
    /// legacy, complete, failed, and non-idempotent outcomes clear the lease.
    pub fn record_tool_continuation_at(
        &self,
        expected_epoch: u64,
        scope: &str,
        tool_name: &str,
        input: &str,
        effect_semantics: &ToolEffectSemantics,
        evidence_complete: Option<bool>,
    ) -> bool {
        let mut state = self.state.lock().expect("run control state poisoned");
        if !self.objective_epoch_matches_locked(&state, expected_epoch) {
            return false;
        }
        let signature = fingerprint(&(tool_name, input));
        let continuation_safe = matches!(
            effect_semantics,
            ToolEffectSemantics::ReadOnly | ToolEffectSemantics::Idempotent
        ) && evidence_complete == Some(false);
        if continuation_safe {
            state
                .continuation_actions
                .insert(scope.to_string(), signature);
        } else {
            state.continuation_actions.remove(scope);
        }
        continuation_safe
    }
}
