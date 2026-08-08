use crate::AgentRunEvent;
use agent_core::{agent_run_id, Event, Metadata, TaskId};
use sha2::{Digest, Sha256};
use std::fmt;

pub const AGENT_TERMINAL_COMMIT_SCHEMA: &str = "cindx.agent.terminal-commit.v1";
pub const AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY: &str = "agent_terminal_commit_schema";
pub const AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY: &str = "agent_terminal_commit_key";
pub const AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY: &str = "agent_terminal_commit_steer_epoch";

const AGENT_TERMINAL_COMMIT_HASH_DOMAIN: &str = "cindx.agent.terminal-commit.v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTerminalCommitState {
    Pending,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTerminalCommitError {
    MissingRunId,
    IdentityEncoding(String),
    MalformedIdentity,
    NonterminalEvent,
    DuplicateTerminalEvents,
    InvalidLifecycleEvent(String),
}

impl fmt::Display for AgentTerminalCommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRunId => {
                write!(
                    formatter,
                    "terminal commit requires a physical agent_run_id"
                )
            }
            Self::IdentityEncoding(error) => {
                write!(
                    formatter,
                    "failed to encode terminal commit identity: {error}"
                )
            }
            Self::MalformedIdentity => {
                write!(formatter, "persisted terminal commit identity is malformed")
            }
            Self::NonterminalEvent => {
                write!(
                    formatter,
                    "terminal commit identity is attached to a nonterminal event"
                )
            }
            Self::DuplicateTerminalEvents => write!(
                formatter,
                "terminal commit identity resolves to multiple terminal events"
            ),
            Self::InvalidLifecycleEvent(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for AgentTerminalCommitError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTerminalCommitIdentity {
    key: String,
    agent_run_id: String,
    steer_epoch: u64,
}

impl AgentTerminalCommitIdentity {
    pub fn new(
        task_id: &TaskId,
        run_context: &Metadata,
        steer_epoch: u64,
    ) -> Result<Self, AgentTerminalCommitError> {
        let agent_run_id = agent_run_id(run_context)
            .filter(|value| !value.trim().is_empty())
            .ok_or(AgentTerminalCommitError::MissingRunId)?;
        let session_id = run_context
            .get("session_id")
            .map(String::as_str)
            .unwrap_or_default();
        let canonical = serde_json::to_vec(&(
            AGENT_TERMINAL_COMMIT_HASH_DOMAIN,
            task_id.0.as_str(),
            session_id,
            agent_run_id,
            steer_epoch,
        ))
        .map_err(|error| AgentTerminalCommitError::IdentityEncoding(error.to_string()))?;
        Ok(Self {
            key: hex_sha256(&canonical),
            agent_run_id: agent_run_id.to_string(),
            steer_epoch,
        })
    }

    pub fn agent_run_id(&self) -> &str {
        &self.agent_run_id
    }

    pub fn metadata(&self) -> Metadata {
        [
            (
                AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY.to_string(),
                AGENT_TERMINAL_COMMIT_SCHEMA.to_string(),
            ),
            (
                AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY.to_string(),
                self.key.clone(),
            ),
            (
                AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY.to_string(),
                self.steer_epoch.to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    pub fn inspect_events(
        &self,
        events: &[Event],
    ) -> Result<AgentTerminalCommitState, AgentTerminalCommitError> {
        let mut matching = events.iter().filter(|event| {
            event.metadata.get("agent_run_id").map(String::as_str)
                == Some(self.agent_run_id.as_str())
                && event
                    .metadata
                    .get(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY)
                    .map(String::as_str)
                    == Some(self.key.as_str())
        });
        let Some(event) = matching.next() else {
            return Ok(AgentTerminalCommitState::Pending);
        };
        if matching.next().is_some() {
            return Err(AgentTerminalCommitError::DuplicateTerminalEvents);
        }
        if event
            .metadata
            .get(AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY)
            .map(String::as_str)
            != Some(AGENT_TERMINAL_COMMIT_SCHEMA)
            || event
                .metadata
                .get(AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY)
                .and_then(|value| value.parse::<u64>().ok())
                != Some(self.steer_epoch)
        {
            return Err(AgentTerminalCommitError::MalformedIdentity);
        }
        let terminal = AgentRunEvent::try_from_event(event)
            .map_err(|error| AgentTerminalCommitError::InvalidLifecycleEvent(error.to_string()))?
            .is_some_and(|event| event.status().is_terminal());
        if !terminal {
            return Err(AgentTerminalCommitError::NonterminalEvent);
        }
        Ok(AgentTerminalCommitState::Committed)
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{
        insert_event_type_v1, EventId, EventKind, EventTypeV1, EVENT_TYPE_METADATA_KEY,
    };

    fn context() -> Metadata {
        [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
        ]
        .into_iter()
        .collect()
    }

    fn terminal_event(identity: &AgentTerminalCommitIdentity) -> Event {
        let mut metadata = identity.metadata();
        metadata.insert("agent_run_id".to_string(), "run-a".to_string());
        insert_event_type_v1(
            &EventKind::TaskStatusChanged,
            &mut metadata,
            EventTypeV1::AgentRunCompleted,
        )
        .unwrap();
        Event {
            id: EventId("terminal-a".to_string()),
            task_id: TaskId("agent".to_string()),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: "localized completion".to_string(),
            metadata,
        }
    }

    #[test]
    fn terminal_identity_is_stable_and_scoped_to_epoch() {
        let task = TaskId("agent".to_string());
        let first = AgentTerminalCommitIdentity::new(&task, &context(), 0).unwrap();
        let replay = AgentTerminalCommitIdentity::new(&task, &context(), 0).unwrap();
        let steered = AgentTerminalCommitIdentity::new(&task, &context(), 1).unwrap();

        assert_eq!(first, replay);
        assert_ne!(first, steered);
        assert_eq!(first.agent_run_id(), "run-a");
    }

    #[test]
    fn lifecycle_contract_distinguishes_pending_and_committed() {
        let identity =
            AgentTerminalCommitIdentity::new(&TaskId("agent".to_string()), &context(), 0).unwrap();
        assert_eq!(
            identity.inspect_events(&[]).unwrap(),
            AgentTerminalCommitState::Pending
        );
        assert_eq!(
            identity
                .inspect_events(&[terminal_event(&identity)])
                .unwrap(),
            AgentTerminalCommitState::Committed
        );
    }

    #[test]
    fn duplicate_malformed_and_nonterminal_commits_fail_closed() {
        let identity =
            AgentTerminalCommitIdentity::new(&TaskId("agent".to_string()), &context(), 0).unwrap();
        let terminal = terminal_event(&identity);
        assert_eq!(
            identity.inspect_events(&[terminal.clone(), terminal.clone()]),
            Err(AgentTerminalCommitError::DuplicateTerminalEvents)
        );

        let mut malformed = terminal.clone();
        malformed.metadata.insert(
            AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY.to_string(),
            "1".to_string(),
        );
        assert_eq!(
            identity.inspect_events(&[malformed]),
            Err(AgentTerminalCommitError::MalformedIdentity)
        );

        let mut nonterminal = terminal;
        nonterminal.metadata.insert(
            EVENT_TYPE_METADATA_KEY.to_string(),
            EventTypeV1::AgentRunPaused.id().to_string(),
        );
        assert_eq!(
            identity.inspect_events(&[nonterminal]),
            Err(AgentTerminalCommitError::NonterminalEvent)
        );
    }

    #[test]
    fn missing_physical_run_id_is_rejected() {
        assert_eq!(
            AgentTerminalCommitIdentity::new(&TaskId("agent".to_string()), &Metadata::new(), 0,),
            Err(AgentTerminalCommitError::MissingRunId)
        );
    }
}
