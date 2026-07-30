use crate::event_security::{redact_metadata, redact_sensitive_text};
use agent_core::{Message, MessageRole, Metadata};
use agent_runtime::{
    sanitize_assistant_content, AgentTaskStateLineage, AgentTaskStateSnapshot,
    AgentTranscriptFingerprintAccumulator,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentRuntimeSnapshotIdentity {
    pub(super) session_id: String,
    pub(super) project_id: Option<String>,
    pub(super) source_run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedAgentRuntimeSnapshot {
    pub(super) identity: Option<AgentRuntimeSnapshotIdentity>,
    pub(super) task_state: AgentTaskStateSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotCursorLineage {
    task_id: String,
    session_id: Option<String>,
    project_id: Option<String>,
    source_run_id: Option<String>,
    persisted_user_prompt: String,
}

#[derive(Debug, Clone)]
pub(super) struct AgentRuntimeSnapshotCursor {
    lineage: SnapshotCursorLineage,
    runtime_message_count: usize,
    transcript: AgentTranscriptFingerprintAccumulator,
    message_visits: usize,
}

impl AgentRuntimeSnapshotCursor {
    pub(super) fn rebuild(runtime: &agent_runtime::AgentLoopState, run_context: &Metadata) -> Self {
        let lineage = SnapshotCursorLineage::capture(runtime, run_context);
        let mut cursor = Self {
            lineage,
            runtime_message_count: 0,
            transcript: AgentTranscriptFingerprintAccumulator::default(),
            message_visits: 0,
        };
        cursor.append_messages(&runtime.messages);
        cursor.runtime_message_count = runtime.messages.len();
        cursor
    }

    pub(super) fn prepare_after_append(
        &self,
        runtime: &agent_runtime::AgentLoopState,
        stable_prefix_message_count: usize,
        run_context: &Metadata,
    ) -> (PreparedAgentRuntimeSnapshot, Self) {
        // The caller must report the exact immutable prefix. Passing zero after
        // any prefix replacement intentionally forces a full lineage rebuild.
        let expected_lineage = SnapshotCursorLineage::capture(runtime, run_context);
        let can_reuse = self.lineage == expected_lineage
            && self.runtime_message_count == stable_prefix_message_count
            && stable_prefix_message_count <= runtime.messages.len();
        let next = if can_reuse {
            let mut next = self.clone();
            next.append_messages(&runtime.messages[stable_prefix_message_count..]);
            next.runtime_message_count = runtime.messages.len();
            next
        } else {
            Self::rebuild(runtime, run_context)
        };
        let task_state = next.capture_task_state(runtime);
        let identity = match (
            next.lineage.session_id.clone(),
            next.lineage.source_run_id.clone(),
        ) {
            (Some(session_id), Some(source_run_id)) if !source_run_id.trim().is_empty() => {
                Some(AgentRuntimeSnapshotIdentity {
                    session_id,
                    project_id: next.lineage.project_id.clone(),
                    source_run_id,
                })
            }
            _ => None,
        };
        (
            PreparedAgentRuntimeSnapshot {
                identity,
                task_state,
            },
            next,
        )
    }

    pub(super) fn capture_current(
        &self,
        runtime: &agent_runtime::AgentLoopState,
        run_context: &Metadata,
    ) -> (AgentTaskStateSnapshot, Self) {
        let (prepared, next) =
            self.prepare_after_append(runtime, self.runtime_message_count, run_context);
        (prepared.task_state, next)
    }

    fn capture_task_state(
        &self,
        runtime: &agent_runtime::AgentLoopState,
    ) -> AgentTaskStateSnapshot {
        let lineage = AgentTaskStateLineage::from_projection(
            &self.lineage.persisted_user_prompt,
            &self.transcript,
        );
        AgentTaskStateSnapshot::capture_with_lineage(runtime, lineage)
    }

    fn append_messages(&mut self, messages: &[Message]) {
        for message in messages {
            let projected = persisted_message_projection(message);
            self.transcript.append(&projected);
            self.message_visits = self.message_visits.saturating_add(1);
        }
    }

    #[cfg(test)]
    pub(super) fn message_visits(&self) -> usize {
        self.message_visits
    }
}

impl SnapshotCursorLineage {
    fn capture(runtime: &agent_runtime::AgentLoopState, run_context: &Metadata) -> Self {
        Self {
            task_id: runtime.task_id.0.clone(),
            session_id: run_context.get("session_id").cloned(),
            project_id: run_context.get("project_id").cloned(),
            source_run_id: run_context.get("agent_run_id").cloned(),
            persisted_user_prompt: redact_sensitive_text(&runtime.user_prompt),
        }
    }
}

fn persisted_message_projection(message: &Message) -> Message {
    let mut metadata = message.metadata.clone();
    let mut content = message.content.clone();
    if message.role == MessageRole::Assistant {
        content = sanitize_assistant_content(&content);
        if let Some(display_content) = metadata.get_mut("display_content") {
            *display_content = sanitize_assistant_content(display_content);
        }
    }
    Message {
        role: message.role.clone(),
        content: redact_sensitive_text(&content),
        metadata: redact_metadata(&metadata),
    }
}

#[cfg(test)]
#[path = "agent_runtime_snapshot_cursor_tests.rs"]
mod tests;
