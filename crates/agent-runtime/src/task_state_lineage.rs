use agent_core::{Message, MessageRole};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct AgentTranscriptFingerprintAccumulator {
    hasher: Sha256,
    durable_message_count: usize,
}

impl Default for AgentTranscriptFingerprintAccumulator {
    fn default() -> Self {
        Self {
            hasher: Sha256::new(),
            durable_message_count: 0,
        }
    }
}

impl AgentTranscriptFingerprintAccumulator {
    pub fn from_messages<'a>(messages: impl IntoIterator<Item = &'a Message>) -> Self {
        let mut accumulator = Self::default();
        for message in messages {
            accumulator.append(message);
        }
        accumulator
    }

    pub fn append(&mut self, message: &Message) {
        if !is_durable_message(message) {
            return;
        }
        self.durable_message_count = self.durable_message_count.saturating_add(1);
        self.hasher
            .update(message_role_label(&message.role).as_bytes());
        self.hasher.update([0]);
        self.hasher.update(message.content.as_bytes());
        self.hasher.update([0]);
        for key in ["tool_call_id", "tool_call_ids", "kind", "status"] {
            if let Some(value) = message.metadata.get(key) {
                self.hasher.update(key.as_bytes());
                self.hasher.update(*b"=");
                self.hasher.update(value.as_bytes());
                self.hasher.update([0]);
            }
        }
        self.hasher.update(*b"\n");
    }

    pub fn durable_message_count(&self) -> usize {
        self.durable_message_count
    }

    pub fn fingerprint(&self) -> String {
        format!("{:x}", self.hasher.clone().finalize())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskStateLineage {
    pub(crate) user_prompt_fingerprint: String,
    pub(crate) transcript_fingerprint: String,
    pub(crate) durable_message_count: usize,
}

impl AgentTaskStateLineage {
    pub fn from_projection(
        persisted_user_prompt: &str,
        transcript: &AgentTranscriptFingerprintAccumulator,
    ) -> Self {
        Self {
            user_prompt_fingerprint: text_fingerprint(persisted_user_prompt),
            transcript_fingerprint: transcript.fingerprint(),
            durable_message_count: transcript.durable_message_count(),
        }
    }
}

pub(crate) fn is_durable_message(message: &Message) -> bool {
    message.metadata.get("kind").map(String::as_str) != Some("recovery_observation")
        && message.metadata.get("model").map(String::as_str) != Some("run-control")
        && !is_transient_run_context(message)
}

pub(crate) fn is_transient_run_context(message: &Message) -> bool {
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

pub(crate) fn text_fingerprint(text: &str) -> String {
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
    use agent_core::Metadata;

    #[test]
    fn canonical_v1_transcript_fingerprint_remains_stable() {
        let messages = [
            Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            },
            Message {
                role: MessageRole::Assistant,
                content: "answer".to_string(),
                metadata: [
                    ("tool_call_ids".to_string(), "a,b".to_string()),
                    ("kind".to_string(), "model_response".to_string()),
                    ("status".to_string(), "ok".to_string()),
                ]
                .into_iter()
                .collect(),
            },
            Message {
                role: MessageRole::Tool,
                content: "done".to_string(),
                metadata: [
                    ("tool_call_id".to_string(), "a".to_string()),
                    ("kind".to_string(), "tool_observation".to_string()),
                    ("status".to_string(), "succeeded".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        ];
        let transcript = AgentTranscriptFingerprintAccumulator::from_messages(messages.iter());

        assert_eq!(transcript.durable_message_count(), 3);
        assert_eq!(
            transcript.fingerprint(),
            "4b3752a445bfe95e07b3862b84f6623bddc39e71a0a79a9c2fcfd1473969cfb6"
        );
    }
}
