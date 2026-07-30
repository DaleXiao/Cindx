use crate::{
    AgentLoopState, AgentTaskContract, InteractionSurface, PendingInteractionVerification,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
struct AgentLoopControlCheckpoint {
    user_prompt: String,
    turn: usize,
    max_turns: usize,
    failed_tool_signatures: BTreeMap<String, usize>,
    consecutive_empty_responses: usize,
    successful_mutations: usize,
    verified_after_last_mutation: bool,
    verification_gate_requests: usize,
    pending_interaction_verifications: BTreeMap<InteractionSurface, PendingInteractionVerification>,
    verified_interactions: usize,
    interaction_verification_gate_requests: usize,
    task_contract: AgentTaskContract,
}

impl AgentLoopControlCheckpoint {
    fn capture(state: &AgentLoopState) -> Self {
        Self {
            user_prompt: state.user_prompt.clone(),
            turn: state.turn,
            max_turns: state.max_turns,
            failed_tool_signatures: state.failed_tool_signatures.clone(),
            consecutive_empty_responses: state.consecutive_empty_responses,
            successful_mutations: state.successful_mutations,
            verified_after_last_mutation: state.verified_after_last_mutation,
            verification_gate_requests: state.verification_gate_requests,
            pending_interaction_verifications: state.pending_interaction_verifications.clone(),
            verified_interactions: state.verified_interactions,
            interaction_verification_gate_requests: state.interaction_verification_gate_requests,
            task_contract: state.task_contract.clone(),
        }
    }

    fn restore(self, state: &mut AgentLoopState) {
        state.user_prompt = self.user_prompt;
        state.turn = self.turn;
        state.max_turns = self.max_turns;
        state.failed_tool_signatures = self.failed_tool_signatures;
        state.consecutive_empty_responses = self.consecutive_empty_responses;
        state.successful_mutations = self.successful_mutations;
        state.verified_after_last_mutation = self.verified_after_last_mutation;
        state.verification_gate_requests = self.verification_gate_requests;
        state.pending_interaction_verifications = self.pending_interaction_verifications;
        state.verified_interactions = self.verified_interactions;
        state.interaction_verification_gate_requests = self.interaction_verification_gate_requests;
        state.task_contract = self.task_contract;
    }
}

/// Rollback guard for runtime transitions that may only append messages.
///
/// Existing messages must remain immutable while this guard is active. The
/// guard deliberately checkpoints only bounded control state, avoiding a clone
/// of the potentially large transcript on every model or tool step.
pub struct AgentLoopAppendTransaction<'state> {
    state: &'state mut AgentLoopState,
    original_message_count: usize,
    checkpoint: Option<AgentLoopControlCheckpoint>,
}

impl<'state> AgentLoopAppendTransaction<'state> {
    pub fn begin(state: &'state mut AgentLoopState) -> Self {
        Self {
            original_message_count: state.messages.len(),
            checkpoint: Some(AgentLoopControlCheckpoint::capture(state)),
            state,
        }
    }

    pub fn state(&self) -> &AgentLoopState {
        self.state
    }

    pub fn state_mut(&mut self) -> &mut AgentLoopState {
        self.state
    }

    pub fn original_message_count(&self) -> usize {
        self.original_message_count
    }

    pub fn commit(mut self) {
        self.checkpoint = None;
    }
}

impl Drop for AgentLoopAppendTransaction<'_> {
    fn drop(&mut self) {
        let Some(checkpoint) = self.checkpoint.take() else {
            return;
        };
        if self.state.messages.len() < self.original_message_count {
            // All production users are append-only. Failing closed here avoids
            // pretending that a removed durable prefix could be reconstructed.
            panic!("agent append transaction removed a committed message prefix");
        }
        self.state.messages.truncate(self.original_message_count);
        checkpoint.restore(self.state);
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
            let state = transaction.state_mut();
            state.user_prompt = "revised objective".to_string();
            state.turn = 7;
            state.max_turns = 99;
            state.failed_tool_signatures.insert("tool:a".to_string(), 2);
            state.consecutive_empty_responses = 2;
            state.successful_mutations = 3;
            state.verified_after_last_mutation = true;
            state.verification_gate_requests = 4;
            state.pending_interaction_verifications.insert(
                InteractionSurface::Browser,
                PendingInteractionVerification {
                    surface: InteractionSurface::Browser,
                    action_tool: "browser.click".to_string(),
                },
            );
            state.verified_interactions = 5;
            state.interaction_verification_gate_requests = 6;
            state.task_contract.merge_workspace_verification_policy(
                WorkspaceVerificationPolicy::RequiredAfterMutation,
            );
            state.messages.push(Message {
                role: MessageRole::Assistant,
                content: "candidate".to_string(),
                metadata: Metadata::new(),
            });
        }
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
            transaction.state_mut().turn = 1;
            transaction.state_mut().messages.push(Message {
                role: MessageRole::Assistant,
                content: "committed".to_string(),
                metadata: Metadata::new(),
            });
            transaction
        };
        transaction.commit();
        assert_eq!(state.turn, 1);
        assert_eq!(state.messages.len(), 2);
    }
}
