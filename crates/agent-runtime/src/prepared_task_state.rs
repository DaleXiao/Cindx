use crate::{
    prompt_completion_intent, run_context_steer_epoch, task_state_lineage::text_fingerprint,
    PromptCompletionIntent,
};
use agent_core::Metadata;

/// Immutable facts selected for one prepared prompt epoch.
///
/// The raw objective and target anchors are runtime-only. Durable checkpoints
/// retain their fingerprints and typed obligations instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTaskState {
    effective_objective: String,
    objective_fingerprint: String,
    steer_epoch: u64,
    contract_epoch: u64,
    completion_intent: PromptCompletionIntent,
}

impl PreparedTaskState {
    pub fn from_run_context(
        run_context: &Metadata,
        latest_prompt: &str,
        completion_intent: PromptCompletionIntent,
    ) -> Self {
        let effective_objective = crate::effective_agent_objective(run_context, latest_prompt);
        let steer_epoch = run_context_steer_epoch(run_context);
        let contract_epoch = run_context
            .get("prompt_contract_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(steer_epoch);
        Self::new(
            effective_objective.to_string(),
            steer_epoch,
            contract_epoch,
            completion_intent,
        )
    }

    pub fn effective_objective(&self) -> &str {
        &self.effective_objective
    }

    pub fn steer_epoch(&self) -> u64 {
        self.steer_epoch
    }

    pub fn contract_epoch(&self) -> u64 {
        self.contract_epoch
    }

    pub fn completion_intent(&self) -> &PromptCompletionIntent {
        &self.completion_intent
    }

    pub(crate) fn advance_control_epoch(&mut self, steer_epoch: u64) {
        self.steer_epoch = self.steer_epoch.max(steer_epoch);
    }

    pub(crate) fn objective_fingerprint(&self) -> &str {
        &self.objective_fingerprint
    }

    pub(crate) fn initial(objective: &str) -> Self {
        let run_context = [(
            "effective_prompt_objective".to_string(),
            objective.to_string(),
        )]
        .into_iter()
        .collect::<Metadata>();
        let completion_intent = prompt_completion_intent(&run_context);
        Self::from_run_context(&run_context, objective, completion_intent)
    }

    pub(crate) fn from_persisted(
        effective_objective: String,
        steer_epoch: u64,
        contract_epoch: u64,
        completion_intent: PromptCompletionIntent,
    ) -> Self {
        Self::new(
            effective_objective,
            steer_epoch,
            contract_epoch,
            completion_intent,
        )
    }

    fn new(
        effective_objective: String,
        steer_epoch: u64,
        contract_epoch: u64,
        completion_intent: PromptCompletionIntent,
    ) -> Self {
        let objective_fingerprint = text_fingerprint(&effective_objective);
        Self {
            effective_objective,
            objective_fingerprint,
            steer_epoch,
            contract_epoch,
            completion_intent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_effective_objective_and_independent_contract_epoch_once() {
        let run_context = [
            (
                "effective_prompt_objective".to_string(),
                "inspect the prepared objective".to_string(),
            ),
            ("steer_epoch".to_string(), "8".to_string()),
            ("prompt_contract_epoch".to_string(), "7".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let intent = prompt_completion_intent(&run_context);

        let state = PreparedTaskState::from_run_context(
            &run_context,
            "latest prompt fallback",
            intent.clone(),
        );

        assert_eq!(
            state.effective_objective(),
            "inspect the prepared objective"
        );
        assert_eq!(state.steer_epoch(), 8);
        assert_eq!(state.contract_epoch(), 7);
        assert_eq!(state.completion_intent(), &intent);
    }

    #[test]
    fn invalid_or_missing_contract_epoch_falls_back_to_steer_epoch() {
        let run_context = [
            ("steer_epoch".to_string(), "4".to_string()),
            ("prompt_contract_epoch".to_string(), "invalid".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();

        let state = PreparedTaskState::from_run_context(
            &run_context,
            "latest prompt fallback",
            PromptCompletionIntent::default(),
        );

        assert_eq!(state.effective_objective(), "latest prompt fallback");
        assert_eq!(state.steer_epoch(), 4);
        assert_eq!(state.contract_epoch(), 4);
    }

    #[test]
    fn noop_control_epoch_advances_without_replacing_the_contract() {
        let mut state = PreparedTaskState::initial("inspect the workspace");
        state.advance_control_epoch(3);

        assert_eq!(state.steer_epoch(), 3);
        assert_eq!(state.contract_epoch(), 0);
        assert_eq!(state.effective_objective(), "inspect the workspace");
    }
}
