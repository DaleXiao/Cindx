use crate::{AgentLoopState, InteractionSurface, PendingInteractionVerification};
use agent_core::{Message, MessageRole, TaskId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};

pub const AGENT_TASK_STATE_SCHEMA: &str = "cindx.agent.task-state.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedInteractionSurface {
    Browser,
    Computer,
}

impl From<InteractionSurface> for PersistedInteractionSurface {
    fn from(surface: InteractionSurface) -> Self {
        match surface {
            InteractionSurface::Browser => Self::Browser,
            InteractionSurface::Computer => Self::Computer,
        }
    }
}

impl From<PersistedInteractionSurface> for InteractionSurface {
    fn from(surface: PersistedInteractionSurface) -> Self {
        match surface {
            PersistedInteractionSurface::Browser => Self::Browser,
            PersistedInteractionSurface::Computer => Self::Computer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedInteractionVerification {
    pub surface: PersistedInteractionSurface,
    pub action_tool: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskStateSnapshot {
    pub schema: String,
    pub task_id: String,
    pub user_prompt_fingerprint: String,
    pub transcript_fingerprint: String,
    pub durable_message_count: usize,
    pub turn: usize,
    pub max_turns: usize,
    pub failed_tool_signatures: BTreeMap<String, usize>,
    pub consecutive_empty_responses: usize,
    pub successful_mutations: usize,
    pub verified_after_last_mutation: bool,
    pub verification_gate_requests: usize,
    pub pending_interaction_verifications: Vec<PersistedInteractionVerification>,
    pub verified_interactions: usize,
    pub interaction_verification_gate_requests: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskStateError {
    message: String,
}

impl AgentTaskStateError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AgentTaskStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentTaskStateError {}

impl AgentTaskStateSnapshot {
    pub fn capture(state: &AgentLoopState) -> Self {
        let durable_messages = durable_messages(&state.messages);
        Self {
            schema: AGENT_TASK_STATE_SCHEMA.to_string(),
            task_id: state.task_id.0.clone(),
            user_prompt_fingerprint: text_fingerprint(&state.user_prompt),
            transcript_fingerprint: transcript_fingerprint(&durable_messages),
            durable_message_count: durable_messages.len(),
            turn: state.turn,
            max_turns: state.max_turns,
            failed_tool_signatures: state.failed_tool_signatures.clone(),
            consecutive_empty_responses: state.consecutive_empty_responses,
            successful_mutations: state.successful_mutations,
            verified_after_last_mutation: state.verified_after_last_mutation,
            verification_gate_requests: state.verification_gate_requests,
            pending_interaction_verifications: state
                .pending_interaction_verifications
                .values()
                .map(|pending| PersistedInteractionVerification {
                    surface: pending.surface.into(),
                    action_tool: pending.action_tool.clone(),
                })
                .collect(),
            verified_interactions: state.verified_interactions,
            interaction_verification_gate_requests: state.interaction_verification_gate_requests,
        }
    }

    pub fn restore(
        &self,
        user_prompt: impl Into<String>,
        messages: Vec<Message>,
    ) -> Result<AgentLoopState, AgentTaskStateError> {
        self.validate()?;
        let user_prompt = user_prompt.into();
        if self.user_prompt_fingerprint != text_fingerprint(&user_prompt) {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint does not match the active user prompt",
            ));
        }
        let durable_messages = durable_messages(&messages);
        if self.durable_message_count != durable_messages.len()
            || self.transcript_fingerprint != transcript_fingerprint(&durable_messages)
        {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint does not match the durable transcript",
            ));
        }

        let mut pending_interaction_verifications = BTreeMap::new();
        for pending in &self.pending_interaction_verifications {
            let surface: InteractionSurface = pending.surface.clone().into();
            if pending.action_tool.trim().is_empty() {
                return Err(AgentTaskStateError::new(
                    "agent task checkpoint contains an empty interaction tool",
                ));
            }
            if pending_interaction_verifications
                .insert(
                    surface,
                    PendingInteractionVerification {
                        surface,
                        action_tool: pending.action_tool.clone(),
                    },
                )
                .is_some()
            {
                return Err(AgentTaskStateError::new(
                    "agent task checkpoint contains duplicate interaction surfaces",
                ));
            }
        }

        Ok(AgentLoopState {
            task_id: TaskId(self.task_id.clone()),
            user_prompt,
            messages,
            turn: self.turn,
            max_turns: self.max_turns,
            failed_tool_signatures: self.failed_tool_signatures.clone(),
            consecutive_empty_responses: self.consecutive_empty_responses,
            successful_mutations: self.successful_mutations,
            verified_after_last_mutation: self.verified_after_last_mutation,
            verification_gate_requests: self.verification_gate_requests,
            pending_interaction_verifications,
            verified_interactions: self.verified_interactions,
            interaction_verification_gate_requests: self.interaction_verification_gate_requests,
        })
    }

    pub fn to_json(&self) -> Result<String, AgentTaskStateError> {
        serde_json::to_string(self).map_err(|error| {
            AgentTaskStateError::new(format!("failed to encode agent task checkpoint: {error}"))
        })
    }

    pub fn from_json(encoded: &str) -> Result<Self, AgentTaskStateError> {
        let snapshot = serde_json::from_str::<Self>(encoded).map_err(|error| {
            AgentTaskStateError::new(format!("failed to decode agent task checkpoint: {error}"))
        })?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<(), AgentTaskStateError> {
        if self.schema != AGENT_TASK_STATE_SCHEMA {
            return Err(AgentTaskStateError::new(format!(
                "unsupported agent task checkpoint schema: {}",
                self.schema
            )));
        }
        if self.task_id.trim().is_empty() {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint is missing a task id",
            ));
        }
        if self.max_turns == 0 {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint has an invalid turn budget",
            ));
        }
        Ok(())
    }
}

fn durable_messages(messages: &[Message]) -> Vec<&Message> {
    messages
        .iter()
        .filter(|message| {
            message.metadata.get("kind").map(String::as_str) != Some("recovery_observation")
                && message.metadata.get("model").map(String::as_str) != Some("run-control")
        })
        .collect()
}

fn transcript_fingerprint(messages: &[&Message]) -> String {
    let mut hasher = Sha256::new();
    for message in messages {
        hasher.update(message_role_label(&message.role).as_bytes());
        hasher.update([0]);
        hasher.update(message.content.as_bytes());
        hasher.update([0]);
        for key in ["tool_call_id", "tool_call_ids", "kind", "status"] {
            if let Some(value) = message.metadata.get(key) {
                hasher.update(key.as_bytes());
                hasher.update([b'=']);
                hasher.update(value.as_bytes());
                hasher.update([0]);
            }
        }
        hasher.update([b'\n']);
    }
    format!("{:x}", hasher.finalize())
}

fn text_fingerprint(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn message_role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::Reviewer => "reviewer",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{start_agent_loop, AgentRuntimeConfig};
    use agent_core::{Metadata, TaskId};

    #[test]
    fn round_trips_control_state_against_the_durable_transcript() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig { max_turns: 32 },
        );
        state.turn = 7;
        state
            .failed_tool_signatures
            .insert("shell.run:abc".to_string(), 2);
        state.successful_mutations = 3;
        state.pending_interaction_verifications.insert(
            InteractionSurface::Browser,
            PendingInteractionVerification {
                surface: InteractionSurface::Browser,
                action_tool: "browser.click".to_string(),
            },
        );
        let snapshot = AgentTaskStateSnapshot::from_json(
            &AgentTaskStateSnapshot::capture(&state)
                .to_json()
                .expect("checkpoint encodes"),
        )
        .expect("checkpoint decodes");
        let restored = snapshot
            .restore(state.user_prompt.clone(), state.messages.clone())
            .expect("matching transcript restores");

        assert_eq!(restored, state);
    }

    #[test]
    fn recovery_only_messages_do_not_break_transcript_lineage() {
        let state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let mut messages = state.messages.clone();
        messages.push(Message {
            role: MessageRole::Tool,
            content: "prior tool outcome is unknown".to_string(),
            metadata: [("kind".to_string(), "recovery_observation".to_string())]
                .into_iter()
                .collect::<Metadata>(),
        });

        let restored = snapshot
            .restore(state.user_prompt.clone(), messages.clone())
            .expect("synthetic recovery message is additive");
        assert_eq!(restored.messages, messages);
    }

    #[test]
    fn rejects_stale_or_tampered_transcripts() {
        let state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let mut messages = state.messages.clone();
        messages[0].content = "different request".to_string();

        assert!(snapshot
            .restore(state.user_prompt.clone(), messages)
            .unwrap_err()
            .to_string()
            .contains("durable transcript"));
        assert!(snapshot
            .restore("different prompt", state.messages.clone())
            .unwrap_err()
            .to_string()
            .contains("active user prompt"));
    }
}
