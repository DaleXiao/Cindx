use crate::{
    strategy_receipt_is_explicitly_not_selected, AgentRunEvent, AgentStrategyDecisionReceipt,
    AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY,
};
use agent_core::{
    agent_run_id, decode_event_type, DecodedEventType, Event, EventTypeV1, Metadata, TaskId,
};
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
    MissingStrategyDecision,
    DuplicateStrategyDecisions,
    StrategyDecisionMismatch,
    TerminalPrecedesStrategyDecision,
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
            Self::MissingStrategyDecision => {
                write!(
                    formatter,
                    "terminal commit has no matching durable strategy decision"
                )
            }
            Self::DuplicateStrategyDecisions => write!(
                formatter,
                "terminal commit resolves to multiple strategy decisions for one epoch"
            ),
            Self::StrategyDecisionMismatch => write!(
                formatter,
                "terminal commit strategy receipt does not match its durable decision"
            ),
            Self::TerminalPrecedesStrategyDecision => write!(
                formatter,
                "terminal commit does not follow its durable strategy decision"
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
    strategy_receipt: Option<AgentStrategyDecisionReceipt>,
    strategy_not_selected: bool,
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
        let strategy_not_selected = strategy_receipt_is_explicitly_not_selected(run_context);
        let strategy_receipt = if strategy_not_selected {
            let receipt_epoch = run_context
                .get(AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY)
                .and_then(|value| value.parse::<u64>().ok());
            if receipt_epoch != Some(steer_epoch) {
                return Err(AgentTerminalCommitError::MalformedIdentity);
            }
            None
        } else {
            AgentStrategyDecisionReceipt::from_metadata(run_context)
                .map_err(|_| AgentTerminalCommitError::MalformedIdentity)?
        };
        if strategy_receipt.as_ref().is_some_and(|receipt| {
            receipt.agent_run_id() != agent_run_id || receipt.steer_epoch() != steer_epoch
        }) {
            return Err(AgentTerminalCommitError::MalformedIdentity);
        }
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
            strategy_receipt,
            strategy_not_selected,
        })
    }

    pub fn agent_run_id(&self) -> &str {
        &self.agent_run_id
    }

    pub fn metadata(&self) -> Metadata {
        let mut metadata = [
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
        .collect();
        if let Some(receipt) = &self.strategy_receipt {
            receipt
                .insert_into(&mut metadata)
                .expect("validated strategy receipt must fit terminal metadata");
        } else if self.strategy_not_selected {
            crate::insert_strategy_not_selected(&mut metadata, self.steer_epoch)
                .expect("validated not-selected state must fit terminal metadata");
        }
        metadata
    }

    pub fn inspect_events(
        &self,
        events: &[Event],
    ) -> Result<AgentTerminalCommitState, AgentTerminalCommitError> {
        let mut terminal_event = None;
        for event in events.iter().filter(|event| {
            event.metadata.get("agent_run_id").map(String::as_str)
                == Some(self.agent_run_id.as_str())
        }) {
            let terminal = AgentRunEvent::try_from_event(event)
                .map_err(|error| {
                    AgentTerminalCommitError::InvalidLifecycleEvent(error.to_string())
                })?
                .is_some_and(|event| event.status().is_terminal());
            if terminal && terminal_event.replace(event).is_some() {
                return Err(AgentTerminalCommitError::DuplicateTerminalEvents);
            }
        }
        let Some(event) = terminal_event else {
            if events.iter().any(|event| {
                event.metadata.get("agent_run_id").map(String::as_str)
                    == Some(self.agent_run_id.as_str())
                    && event
                        .metadata
                        .get(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY)
                        .map(String::as_str)
                        == Some(self.key.as_str())
            }) {
                return Err(AgentTerminalCommitError::NonterminalEvent);
            }
            self.validate_strategy_decision(events)?;
            return Ok(AgentTerminalCommitState::Pending);
        };
        if event
            .metadata
            .get(AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY)
            .map(String::as_str)
            != Some(AGENT_TERMINAL_COMMIT_SCHEMA)
            || event
                .metadata
                .get(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY)
                .map(String::as_str)
                != Some(self.key.as_str())
            || event
                .metadata
                .get(AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY)
                .and_then(|value| value.parse::<u64>().ok())
                != Some(self.steer_epoch)
        {
            return Err(AgentTerminalCommitError::MalformedIdentity);
        }
        match &self.strategy_receipt {
            Some(receipt)
                if AgentStrategyDecisionReceipt::from_metadata(&event.metadata)
                    .ok()
                    .flatten()
                    .as_ref()
                    != Some(receipt) =>
            {
                return Err(AgentTerminalCommitError::StrategyDecisionMismatch)
            }
            None if self.strategy_not_selected
                && !strategy_receipt_is_explicitly_not_selected(&event.metadata) =>
            {
                return Err(AgentTerminalCommitError::StrategyDecisionMismatch)
            }
            None if !self.strategy_not_selected
                && event
                    .metadata
                    .contains_key(crate::AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY) =>
            {
                return Err(AgentTerminalCommitError::StrategyDecisionMismatch)
            }
            _ => {}
        }
        if self
            .validate_strategy_decision(events)?
            .is_some_and(|decision| event.sequence <= decision.sequence)
        {
            return Err(AgentTerminalCommitError::TerminalPrecedesStrategyDecision);
        }
        Ok(AgentTerminalCommitState::Committed)
    }

    fn validate_strategy_decision<'a>(
        &self,
        events: &'a [Event],
    ) -> Result<Option<&'a Event>, AgentTerminalCommitError> {
        let decisions = events
            .iter()
            .filter(|event| {
                event.metadata.get("agent_run_id").map(String::as_str)
                    == Some(self.agent_run_id.as_str())
                    && event
                        .metadata
                        .get("steer_epoch")
                        .and_then(|value| value.parse::<u64>().ok())
                        == Some(self.steer_epoch)
                    && matches!(
                        decode_event_type(event),
                        DecodedEventType::V1(typed)
                            if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
                    )
            })
            .collect::<Vec<_>>();
        match &self.strategy_receipt {
            Some(_) if decisions.is_empty() => {
                Err(AgentTerminalCommitError::MissingStrategyDecision)
            }
            Some(_) if decisions.len() > 1 => {
                Err(AgentTerminalCommitError::DuplicateStrategyDecisions)
            }
            Some(receipt) if !receipt.matches_decision_event(decisions[0]) => {
                Err(AgentTerminalCommitError::StrategyDecisionMismatch)
            }
            Some(_) => Ok(Some(decisions[0])),
            None if !decisions.is_empty() => {
                Err(AgentTerminalCommitError::StrategyDecisionMismatch)
            }
            _ => Ok(None),
        }
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
            sequence: 2,
            timestamp_ms: 2,
            kind: EventKind::TaskStatusChanged,
            summary: "localized completion".to_string(),
            metadata,
        }
    }

    fn selected_context() -> (Metadata, AgentStrategyDecisionReceipt) {
        let mut context = context();
        context.insert("steer_epoch".to_string(), "0".to_string());
        let receipt = AgentStrategyDecisionReceipt::new(
            &TaskId("agent".to_string()),
            &context,
            &"a".repeat(64),
        )
        .unwrap();
        receipt.insert_into(&mut context).unwrap();
        (context, receipt)
    }

    fn decision_event(receipt: &AgentStrategyDecisionReceipt) -> Event {
        let mut metadata = context();
        metadata.insert("steer_epoch".to_string(), "0".to_string());
        receipt.insert_into(&mut metadata).unwrap();
        insert_event_type_v1(
            &EventKind::TaskStatusChanged,
            &mut metadata,
            EventTypeV1::AgentRunDecisionSelected,
        )
        .unwrap();
        Event {
            id: EventId("decision-a".to_string()),
            task_id: TaskId("agent".to_string()),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: "localized decision".to_string(),
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
    fn agent_strategy_lifecycle_contract_distinguishes_pending_and_committed() {
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
    fn agent_strategy_lifecycle_contract_duplicate_malformed_and_nonterminal_commits_fail_closed() {
        let identity =
            AgentTerminalCommitIdentity::new(&TaskId("agent".to_string()), &context(), 0).unwrap();
        let terminal = terminal_event(&identity);
        assert_eq!(
            identity.inspect_events(&[terminal.clone(), terminal.clone()]),
            Err(AgentTerminalCommitError::DuplicateTerminalEvents)
        );
        let mut unlinked = terminal.clone();
        unlinked
            .metadata
            .remove(AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY);
        assert_eq!(
            identity.inspect_events(&[terminal.clone(), unlinked]),
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

    #[test]
    fn agent_strategy_lifecycle_contract_selected_strategy_must_match_before_terminal_commit() {
        let (context, receipt) = selected_context();
        let identity =
            AgentTerminalCommitIdentity::new(&TaskId("agent".to_string()), &context, 0).unwrap();
        assert_eq!(
            identity.inspect_events(&[]),
            Err(AgentTerminalCommitError::MissingStrategyDecision)
        );

        let decision = decision_event(&receipt);
        let terminal = terminal_event(&identity);
        assert_eq!(
            identity
                .inspect_events(&[decision.clone(), terminal])
                .unwrap(),
            AgentTerminalCommitState::Committed
        );
        let mut early_terminal = terminal_event(&identity);
        early_terminal.sequence = decision.sequence;
        assert_eq!(
            identity.inspect_events(&[decision.clone(), early_terminal]),
            Err(AgentTerminalCommitError::TerminalPrecedesStrategyDecision)
        );
        assert_eq!(
            identity.inspect_events(&[decision.clone(), decision]),
            Err(AgentTerminalCommitError::DuplicateStrategyDecisions)
        );
    }
}
