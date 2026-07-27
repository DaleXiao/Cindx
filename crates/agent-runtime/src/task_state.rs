use crate::{
    AgentLoopState, AgentTaskContract, InteractionSurface, PendingInteractionVerification,
};
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
    #[serde(default)]
    pub task_contract: AgentTaskContract,
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
            task_contract: state.task_contract.clone(),
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
        let Some(messages) = self.matching_runtime_projection(messages) else {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint does not match the durable transcript",
            ));
        };

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

        let task_contract = if self.task_contract == AgentTaskContract::default()
            && (self.successful_mutations > 0 || !pending_interaction_verifications.is_empty())
        {
            AgentTaskContract::restore_legacy(
                self.successful_mutations,
                self.verified_after_last_mutation,
                pending_interaction_verifications
                    .iter()
                    .map(|(surface, pending)| (*surface, pending.action_tool.clone()))
                    .collect(),
            )
        } else {
            self.task_contract.clone()
        };

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
            task_contract,
        })
    }

    fn matching_runtime_projection(&self, messages: Vec<Message>) -> Option<Vec<Message>> {
        let durable_indices = messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| is_durable_message(message).then_some(index))
            .collect::<Vec<_>>();
        if self.durable_message_count > durable_indices.len() {
            return None;
        }
        if self.durable_message_count == 0 {
            return (self.transcript_fingerprint == transcript_fingerprint(&[]))
                .then(Vec::new);
        }

        // Context compaction replaces an older durable prefix with a transient
        // restore pack. The remaining durable messages are therefore a suffix
        // of the event transcript, not necessarily the whole transcript.
        let first_durable = durable_indices[durable_indices.len() - self.durable_message_count];
        let projection = messages[first_durable..]
            .iter()
            .filter(|message| !is_transient_run_context(message))
            .cloned()
            .collect::<Vec<_>>();
        let durable_projection = durable_messages(&projection);
        (durable_projection.len() == self.durable_message_count
            && self.transcript_fingerprint == transcript_fingerprint(&durable_projection))
        .then_some(projection)
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
    messages.iter().filter(|message| is_durable_message(message)).collect()
}

fn is_durable_message(message: &Message) -> bool {
    message.metadata.get("kind").map(String::as_str) != Some("recovery_observation")
        && message.metadata.get("model").map(String::as_str) != Some("run-control")
        && !is_transient_run_context(message)
}

fn is_transient_run_context(message: &Message) -> bool {
    if !matches!(message.role, MessageRole::System)
        || message.metadata.get("internal").map(String::as_str) != Some("true")
    {
        return false;
    }
    if message.metadata.contains_key("collaboration_stage") {
        return true;
    }
    matches!(
        message.metadata.get("kind").map(String::as_str),
        Some(
            "image_generation_policy"
                | "workflow_execution_contract"
                | "agent_evidence_packet"
                | "context_restore_pack"
                | "artifact_manifest"
                | "knowledge_context"
                | "project_memory"
                | "skill_context"
                | "single_model_policy_guidance"
        )
    )
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
                hasher.update(*b"=");
                hasher.update(value.as_bytes());
                hasher.update([0]);
            }
        }
        hasher.update(*b"\n");
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
    use crate::{record_tool_outcome_with_risk, start_agent_loop, AgentRuntimeConfig};
    use agent_core::{Metadata, TaskId, ToolOutcomeStatus, ToolRisk};

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
        for path in ["src/one.rs", "src/two.rs", "src/three.rs"] {
            record_tool_outcome_with_risk(
                &mut state,
                "file.write",
                &format!(r#"{{"path":"{path}"}}"#),
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::WritesWorkspace),
            );
        }
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
    fn transient_run_context_does_not_enter_checkpoint_lineage() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        state.messages.insert(
            0,
            Message {
                role: MessageRole::System,
                content: "request-scoped retrieved evidence".to_string(),
                metadata: [
                    ("internal".to_string(), "true".to_string()),
                    ("kind".to_string(), "knowledge_context".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        );
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let durable_transcript = state.messages[1..].to_vec();

        let restored = snapshot
            .restore(state.user_prompt.clone(), state.messages.clone())
            .expect("request-scoped context should be regenerated, not persisted");
        assert_eq!(restored.messages, durable_transcript);
    }

    #[test]
    fn compacted_checkpoint_matches_a_verified_transcript_suffix() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "continue the implementation",
            AgentRuntimeConfig::default(),
        );
        let old_message = Message {
            role: MessageRole::Assistant,
            content: "old completed discussion".to_string(),
            metadata: Metadata::new(),
        };
        let recent_message = Message {
            role: MessageRole::Assistant,
            content: "recent verified work".to_string(),
            metadata: Metadata::new(),
        };
        state.messages = vec![recent_message.clone()];
        let snapshot = AgentTaskStateSnapshot::capture(&state);

        let restored = snapshot
            .restore(
                state.user_prompt.clone(),
                vec![old_message, recent_message.clone()],
            )
            .expect("compacted durable suffix should restore");
        assert_eq!(restored.messages, vec![recent_message]);
    }

    #[test]
    fn durable_internal_instructions_remain_in_checkpoint_lineage() {
        let mut state = start_agent_loop(
            TaskId("task-1".to_string()),
            "inspect the workspace",
            AgentRuntimeConfig::default(),
        );
        state.messages.push(Message {
            role: MessageRole::System,
            content: "verify before completion".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "completion_verification".to_string()),
            ]
            .into_iter()
            .collect(),
        });
        let snapshot = AgentTaskStateSnapshot::capture(&state);
        let mut missing_instruction = state.messages.clone();
        missing_instruction.pop();

        assert!(snapshot
            .restore(state.user_prompt.clone(), missing_instruction)
            .unwrap_err()
            .to_string()
            .contains("durable transcript"));
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
