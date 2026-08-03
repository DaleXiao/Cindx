use crate::{AgentLoopState, AgentTaskContract, PreparedTaskState};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
struct AgentLoopControlCheckpoint {
    task_id: agent_core::TaskId,
    user_prompt: String,
    turn: usize,
    max_turns: usize,
    failed_tool_signatures: BTreeMap<String, usize>,
    consecutive_empty_responses: usize,
    verification_gate_requests: usize,
    verified_interactions: usize,
    interaction_verification_gate_requests: usize,
    task_contract: AgentTaskContract,
    prepared_task_state: PreparedTaskState,
}

impl AgentLoopControlCheckpoint {
    fn capture(state: &AgentLoopState) -> Self {
        Self {
            task_id: state.task_id.clone(),
            user_prompt: state.user_prompt.clone(),
            turn: state.turn,
            max_turns: state.max_turns,
            failed_tool_signatures: state.failed_tool_signatures.clone(),
            consecutive_empty_responses: state.consecutive_empty_responses,
            verification_gate_requests: state.verification_gate_requests,
            verified_interactions: state.verified_interactions,
            interaction_verification_gate_requests: state.interaction_verification_gate_requests,
            task_contract: state.task_contract.clone(),
            prepared_task_state: state.prepared_task_state().clone(),
        }
    }

    fn restore(self, state: &mut AgentLoopState) {
        state.task_id = self.task_id;
        state.user_prompt = self.user_prompt;
        state.turn = self.turn;
        state.max_turns = self.max_turns;
        state.failed_tool_signatures = self.failed_tool_signatures;
        state.consecutive_empty_responses = self.consecutive_empty_responses;
        state.verification_gate_requests = self.verification_gate_requests;
        state.verified_interactions = self.verified_interactions;
        state.interaction_verification_gate_requests = self.interaction_verification_gate_requests;
        state.task_contract = self.task_contract;
        state.replace_prepared_task_state(self.prepared_task_state);
    }
}

/// Rollback guard for runtime transitions that may only append messages.
///
/// Existing messages must remain immutable while this guard is active. Release
/// builds checkpoint only bounded control state; debug and test builds retain a
/// prefix copy so append-only violations are detected and recoverable without
/// adding a long-transcript scan to the production hot path.
pub struct AgentLoopAppendTransaction<'state> {
    state: &'state mut AgentLoopState,
    original_message_count: usize,
    checkpoint: Option<AgentLoopControlCheckpoint>,
    #[cfg(any(test, debug_assertions))]
    committed_message_prefix: Vec<agent_core::Message>,
}

impl<'state> AgentLoopAppendTransaction<'state> {
    pub fn begin(state: &'state mut AgentLoopState) -> Self {
        Self {
            original_message_count: state.messages.len(),
            checkpoint: Some(AgentLoopControlCheckpoint::capture(state)),
            #[cfg(any(test, debug_assertions))]
            committed_message_prefix: state.messages.clone(),
            state,
        }
    }

    pub fn state(&self) -> &AgentLoopState {
        self.state
    }

    pub fn with_append_only_mutation<T>(
        &mut self,
        mutation: impl FnOnce(&mut AgentLoopState) -> T,
    ) -> T {
        let result = mutation(self.state);
        self.assert_transaction_invariants();
        result
    }

    pub fn original_message_count(&self) -> usize {
        self.original_message_count
    }

    pub fn commit(mut self) {
        self.assert_transaction_invariants();
        self.checkpoint = None;
    }

    fn assert_transaction_invariants(&mut self) {
        let task_id_is_unchanged = self
            .checkpoint
            .as_ref()
            .is_none_or(|checkpoint| checkpoint.task_id == self.state.task_id);
        let prefix_is_present = self.state.messages.len() >= self.original_message_count;
        #[cfg(any(test, debug_assertions))]
        let prefix_is_unchanged = prefix_is_present
            && self.state.messages[..self.original_message_count]
                == self.committed_message_prefix[..];
        #[cfg(not(any(test, debug_assertions)))]
        let prefix_is_unchanged = prefix_is_present;

        if task_id_is_unchanged && prefix_is_unchanged {
            return;
        }
        let violation = if !task_id_is_unchanged {
            "agent append transaction modified its task id"
        } else {
            "agent append transaction modified a committed message prefix"
        };
        self.rollback();
        panic!("{violation}");
    }

    fn rollback(&mut self) {
        let Some(checkpoint) = self.checkpoint.take() else {
            return;
        };
        #[cfg(any(test, debug_assertions))]
        {
            self.state.messages = std::mem::take(&mut self.committed_message_prefix);
        }
        #[cfg(not(any(test, debug_assertions)))]
        {
            self.state.messages.truncate(self.original_message_count);
        }
        checkpoint.restore(self.state);
    }
}

impl Drop for AgentLoopAppendTransaction<'_> {
    fn drop(&mut self) {
        self.rollback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{start_agent_loop, AgentRuntimeConfig, WorkspaceVerificationPolicy};
    use agent_core::{Message, MessageRole, Metadata, TaskId};

    #[test]
    fn failed_append_transaction_restores_messages_and_control_state() {
        let mut state = start_agent_loop(
            TaskId("task-a".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let before = state.clone();
        {
            let mut transaction = AgentLoopAppendTransaction::begin(&mut state);
            transaction.with_append_only_mutation(|state| {
                state.user_prompt = "revised objective".to_string();
                state.turn = 7;
                state.max_turns = 99;
                state.failed_tool_signatures.insert("tool:a".to_string(), 2);
                state.consecutive_empty_responses = 2;
                state.verification_gate_requests = 4;
                state.task_contract = AgentTaskContract::restore_legacy(
                    3,
                    true,
                    [(
                        crate::InteractionSurface::Browser,
                        "browser.click".to_string(),
                    )]
                    .into_iter()
                    .collect(),
                );
                state.verified_interactions = 5;
                state.interaction_verification_gate_requests = 6;
                state.task_contract.merge_workspace_verification_policy(
                    WorkspaceVerificationPolicy::RequiredAfterMutation,
                );
                state.replace_prepared_task_state(PreparedTaskState::from_run_context(
                    &[(
                        "effective_prompt_objective".to_string(),
                        "revised objective".to_string(),
                    )]
                    .into_iter()
                    .collect(),
                    "inspect",
                    crate::PromptCompletionIntent::default(),
                ));
                state.messages.push(Message {
                    role: MessageRole::Assistant,
                    content: "candidate".to_string(),
                    metadata: Metadata::new(),
                });
            });
        }
        assert_eq!(state, before);
    }

    #[test]
    fn failed_append_transaction_restores_task_id() {
        let mut state = start_agent_loop(
            TaskId("task-a".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut transaction = AgentLoopAppendTransaction::begin(&mut state);
            transaction.with_append_only_mutation(|state| {
                state.task_id = TaskId("candidate-task".to_string());
                panic!("injected transaction failure");
            });
        }));

        assert!(result.is_err());
        assert_eq!(state.task_id, TaskId("task-a".to_string()));
    }

    #[test]
    fn committed_task_id_rewrite_is_rejected_and_rolled_back() {
        let mut state = start_agent_loop(
            TaskId("task-a".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let before = state.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut transaction = AgentLoopAppendTransaction::begin(&mut state);
            transaction.with_append_only_mutation(|state| {
                state.task_id = TaskId("candidate-task".to_string());
                state.messages.push(Message {
                    role: MessageRole::Assistant,
                    content: "candidate".to_string(),
                    metadata: Metadata::new(),
                });
            });
            transaction.commit();
        }));

        assert!(result.is_err());
        assert_eq!(state, before);
    }

    #[test]
    fn committed_append_transaction_keeps_messages_and_control_state() {
        let mut state = start_agent_loop(
            TaskId("task-a".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let transaction = {
            let mut transaction = AgentLoopAppendTransaction::begin(&mut state);
            transaction.with_append_only_mutation(|state| {
                state.turn = 1;
                state.messages.push(Message {
                    role: MessageRole::Assistant,
                    content: "committed".to_string(),
                    metadata: Metadata::new(),
                });
            });
            transaction
        };
        transaction.commit();
        assert_eq!(state.turn, 1);
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn committed_prefix_rewrite_is_detected_and_rolled_back() {
        let mut state = start_agent_loop(
            TaskId("task-a".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let before = state.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut transaction = AgentLoopAppendTransaction::begin(&mut state);
            transaction.with_append_only_mutation(|state| {
                state.messages[0].content = "rewritten committed prompt".to_string();
                state.messages.push(Message {
                    role: MessageRole::Assistant,
                    content: "candidate".to_string(),
                    metadata: Metadata::new(),
                });
            });
        }));

        assert!(result.is_err());
        assert_eq!(state, before);
    }
}
