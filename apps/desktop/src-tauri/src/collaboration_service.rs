use agent_core::{Message, MessageRole, Metadata};
use agent_runtime::AgentFailure;
use serde::{Deserialize, Serialize};
pub(crate) const COLLABORATION_TOOL_EVIDENCE_SCHEMA: &str = "cindx.collaboration-tool-evidence.v1";
pub(crate) const COLLABORATION_STEER_INTERRUPTED: &str =
    "collaboration interrupted for pending user steer";
#[derive(Debug, Clone)]
pub(crate) struct AgentCollaboration {
    pub(crate) id: String,
    pub(crate) policy: String,
    pub(crate) grounding_receipts: Vec<CollaborationGroundingReceipt>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationGroundingReceipt {
    pub(crate) steer_epoch: u64,
    pub(crate) collaboration_id: String,
    pub(crate) source_step: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) request: String,
    pub(crate) input_fingerprint: String,
    pub(crate) observation: String,
}
#[derive(Debug)]
pub(crate) struct CollaborationCompletion {
    pub(crate) content: Option<String>,
    pub(crate) partial_content: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) failure: Option<AgentFailure>,
    pub(crate) latency_ms: u64,
    pub(crate) usage: Metadata,
    pub(crate) evidence: Vec<CollaborationEvidence>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CollaborationEvidence {
    #[serde(default)]
    pub(crate) evidence_schema: String,
    #[serde(default)]
    pub(crate) steer_epoch: Option<u64>,
    #[serde(default)]
    pub(crate) collaboration_id: String,
    pub(crate) source_step: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    #[serde(default)]
    pub(crate) request: String,
    pub(crate) status: String,
    pub(crate) output: String,
}
impl CollaborationCompletion {
    pub(crate) fn completed_worker(
        content: String,
        latency_ms: u64,
        usage: Metadata,
        evidence: Vec<CollaborationEvidence>,
    ) -> Self {
        Self {
            content: Some(content),
            partial_content: None,
            error: None,
            failure: None,
            latency_ms,
            usage,
            evidence,
        }
    }
    pub(crate) fn failed_worker(
        failure: AgentFailure,
        partial_content: Option<String>,
        latency_ms: u64,
        usage: Metadata,
        evidence: Vec<CollaborationEvidence>,
    ) -> Self {
        Self {
            content: None,
            partial_content,
            error: Some(failure.message.clone()),
            failure: Some(failure),
            latency_ms,
            usage,
            evidence,
        }
    }
    #[cfg(test)]
    pub(crate) fn failed(error: impl Into<String>) -> Self {
        Self::failed_with(AgentFailure::internal("collaboration_internal", error))
    }

    pub(crate) fn failed_with(failure: AgentFailure) -> Self {
        Self {
            content: None,
            partial_content: None,
            error: Some(failure.message.clone()),
            failure: Some(failure),
            latency_ms: 0,
            usage: Metadata::new(),
            evidence: Vec::new(),
        }
    }
}
pub(crate) fn truncate_for_collaboration(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
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
pub(crate) fn collaboration_recent_context(history: &[Message]) -> String {
    history
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|message| {
            let max_chars =
                if message.metadata.get("kind").map(String::as_str) == Some("knowledge_context") {
                    6_000
                } else {
                    1_200
                };
            format!(
                "{}: {}",
                message_role_label(&message.role),
                truncate_for_collaboration(&message.content, max_chars)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[cfg(test)]
mod grounding_receipt_tests {
    use super::*;
    
    fn evidence(
        schema: &str,
        epoch: Option<u64>,
        status: &str,
        tool_name: &str,
        call_id: &str,
    ) -> CollaborationEvidence {
        CollaborationEvidence {
            evidence_schema: schema.to_string(),
            steer_epoch: epoch,
            collaboration_id: "collaboration-1".to_string(),
            source_step: "inspect".to_string(),
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            request: r#"{"path":"README.md"}"#.to_string(),
            status: status.to_string(),
            output: "runtime observation".to_string(),
        }
    }
}