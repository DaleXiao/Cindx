use agent_core::{
    agent_run_id, decode_event_type, DecodedEventType, Event, EventTypeV1, Metadata, TaskId,
};
use sha2::{Digest, Sha256};
use std::fmt;

pub const AGENT_STRATEGY_RECEIPT_SCHEMA: &str = "cindx.agent.strategy-receipt.v1";
pub const AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY: &str = "agent_strategy_receipt_schema";
pub const AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY: &str = "agent_strategy_receipt_key";
pub const AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY: &str = "agent_strategy_receipt_steer_epoch";
pub const AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY: &str = "agent_strategy_receipt_plan_sha256";
pub const AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY: &str = "agent_strategy_receipt_status";
pub const AGENT_STRATEGY_RECEIPT_SELECTED: &str = "selected";
pub const AGENT_STRATEGY_RECEIPT_NOT_SELECTED: &str = "not_selected";

const AGENT_STRATEGY_RECEIPT_HASH_DOMAIN: &str = "cindx.agent.strategy-receipt.v1\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStrategyDecisionReceipt {
    key: String,
    agent_run_id: String,
    steer_epoch: u64,
    plan_sha256: String,
}

impl AgentStrategyDecisionReceipt {
    pub fn new(
        task_id: &TaskId,
        run_context: &Metadata,
        plan_sha256: &str,
    ) -> Result<Self, AgentStrategyReceiptError> {
        let agent_run_id = agent_run_id(run_context)
            .filter(|value| !value.trim().is_empty())
            .ok_or(AgentStrategyReceiptError::MissingRunId)?;
        let steer_epoch = run_context
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(AgentStrategyReceiptError::MissingSteerEpoch)?;
        validate_sha256(plan_sha256)?;
        let session_id = run_context
            .get("session_id")
            .map(String::as_str)
            .unwrap_or_default();
        let canonical = serde_json::to_vec(&(
            AGENT_STRATEGY_RECEIPT_HASH_DOMAIN,
            task_id.0.as_str(),
            session_id,
            agent_run_id,
            steer_epoch,
            plan_sha256,
        ))
        .map_err(|error| AgentStrategyReceiptError::IdentityEncoding(error.to_string()))?;
        Ok(Self {
            key: hex_sha256(&canonical),
            agent_run_id: agent_run_id.to_string(),
            steer_epoch,
            plan_sha256: plan_sha256.to_string(),
        })
    }

    pub fn from_metadata(metadata: &Metadata) -> Result<Option<Self>, AgentStrategyReceiptError> {
        let schema = metadata.get(AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY);
        let status = metadata.get(AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY);
        let key = metadata.get(AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY);
        let epoch = metadata.get(AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY);
        let plan = metadata.get(AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY);
        if schema.is_none()
            && status.is_none()
            && key.is_none()
            && epoch.is_none()
            && plan.is_none()
        {
            return Ok(None);
        }
        if schema.map(String::as_str) != Some(AGENT_STRATEGY_RECEIPT_SCHEMA)
            || status.map(String::as_str) != Some(AGENT_STRATEGY_RECEIPT_SELECTED)
        {
            return Err(AgentStrategyReceiptError::MalformedReceipt);
        }
        let key = required_non_empty(key)?;
        validate_sha256(key)?;
        let agent_run_id = agent_run_id(metadata)
            .filter(|value| !value.trim().is_empty())
            .ok_or(AgentStrategyReceiptError::MissingRunId)?;
        let steer_epoch = epoch
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(AgentStrategyReceiptError::MissingSteerEpoch)?;
        let plan_sha256 = required_non_empty(plan)?;
        validate_sha256(plan_sha256)?;
        Ok(Some(Self {
            key: key.to_string(),
            agent_run_id: agent_run_id.to_string(),
            steer_epoch,
            plan_sha256: plan_sha256.to_string(),
        }))
    }

    pub fn from_decision_event(event: &Event) -> Result<Self, AgentStrategyReceiptError> {
        if !matches!(
            decode_event_type(event),
            DecodedEventType::V1(typed)
                if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
        ) {
            return Err(AgentStrategyReceiptError::MalformedReceipt);
        }
        let receipt = Self::from_metadata(&event.metadata)?
            .ok_or(AgentStrategyReceiptError::MalformedReceipt)?;
        let expected = Self::new(&event.task_id, &event.metadata, receipt.plan_sha256())?;
        if receipt != expected {
            return Err(AgentStrategyReceiptError::IdentityMismatch);
        }
        Ok(receipt)
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn agent_run_id(&self) -> &str {
        &self.agent_run_id
    }

    pub fn steer_epoch(&self) -> u64 {
        self.steer_epoch
    }

    pub fn plan_sha256(&self) -> &str {
        &self.plan_sha256
    }

    pub fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::new();
        self.insert_into(&mut metadata)
            .expect("empty receipt metadata must accept a valid receipt");
        metadata
    }

    pub fn insert_into(&self, metadata: &mut Metadata) -> Result<(), AgentStrategyReceiptError> {
        let steer_epoch = self.steer_epoch.to_string();
        let fields = [
            (
                AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY,
                AGENT_STRATEGY_RECEIPT_SCHEMA,
            ),
            (
                AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY,
                AGENT_STRATEGY_RECEIPT_SELECTED,
            ),
            (AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY, self.key.as_str()),
            (
                AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY,
                steer_epoch.as_str(),
            ),
            (
                AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY,
                self.plan_sha256.as_str(),
            ),
        ];
        for (key, value) in &fields {
            if metadata
                .get(*key)
                .is_some_and(|existing| existing != *value)
            {
                return Err(AgentStrategyReceiptError::ReservedMetadataConflict(key));
            }
        }
        for (key, value) in fields {
            metadata.insert(key.to_string(), value.to_string());
        }
        Ok(())
    }

    pub fn matches_decision_event(&self, event: &Event) -> bool {
        Self::from_decision_event(event).is_ok_and(|persisted| persisted == *self)
    }
}

pub fn insert_strategy_not_selected(
    metadata: &mut Metadata,
    steer_epoch: u64,
) -> Result<(), AgentStrategyReceiptError> {
    let fields = [
        (
            AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY,
            AGENT_STRATEGY_RECEIPT_SCHEMA.to_string(),
        ),
        (
            AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY,
            AGENT_STRATEGY_RECEIPT_NOT_SELECTED.to_string(),
        ),
        (
            AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY,
            steer_epoch.to_string(),
        ),
    ];
    for (key, value) in &fields {
        if metadata.get(*key).is_some_and(|existing| existing != value) {
            return Err(AgentStrategyReceiptError::ReservedMetadataConflict(key));
        }
    }
    if metadata.contains_key(AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY)
        || metadata.contains_key(AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY)
    {
        return Err(AgentStrategyReceiptError::MalformedReceipt);
    }
    metadata.extend(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    );
    Ok(())
}

pub fn strategy_receipt_is_explicitly_not_selected(metadata: &Metadata) -> bool {
    metadata
        .get(AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY)
        .map(String::as_str)
        == Some(AGENT_STRATEGY_RECEIPT_SCHEMA)
        && metadata
            .get(AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY)
            .map(String::as_str)
            == Some(AGENT_STRATEGY_RECEIPT_NOT_SELECTED)
        && !metadata.contains_key(AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY)
        && !metadata.contains_key(AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStrategyReceiptError {
    MissingRunId,
    MissingSteerEpoch,
    InvalidPlanDigest,
    IdentityEncoding(String),
    MalformedReceipt,
    IdentityMismatch,
    ReservedMetadataConflict(&'static str),
}

impl fmt::Display for AgentStrategyReceiptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRunId => write!(
                formatter,
                "strategy receipt requires a physical agent_run_id"
            ),
            Self::MissingSteerEpoch => {
                write!(formatter, "strategy receipt requires a valid steer_epoch")
            }
            Self::InvalidPlanDigest => {
                write!(formatter, "strategy receipt requires a SHA-256 plan digest")
            }
            Self::IdentityEncoding(error) => write!(
                formatter,
                "failed to encode strategy receipt identity: {error}"
            ),
            Self::MalformedReceipt => write!(formatter, "persisted strategy receipt is malformed"),
            Self::IdentityMismatch => write!(
                formatter,
                "persisted strategy receipt does not match its run identity"
            ),
            Self::ReservedMetadataConflict(key) => write!(
                formatter,
                "reserved strategy receipt metadata conflicts at `{key}`"
            ),
        }
    }
}

impl std::error::Error for AgentStrategyReceiptError {}

fn required_non_empty(value: Option<&String>) -> Result<&str, AgentStrategyReceiptError> {
    value
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(AgentStrategyReceiptError::MalformedReceipt)
}

fn validate_sha256(value: &str) -> Result<(), AgentStrategyReceiptError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AgentStrategyReceiptError::InvalidPlanDigest)
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
    use agent_core::{insert_event_type_v1, EventId, EventKind};

    fn context() -> Metadata {
        [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
            ("steer_epoch".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn receipt_round_trips_and_matches_only_its_typed_decision() {
        let receipt = AgentStrategyDecisionReceipt::new(
            &TaskId("agent".to_string()),
            &context(),
            &"a".repeat(64),
        )
        .unwrap();
        let mut metadata = context();
        receipt.insert_into(&mut metadata).unwrap();
        insert_event_type_v1(
            &agent_core::EventKind::TaskStatusChanged,
            &mut metadata,
            EventTypeV1::AgentRunDecisionSelected,
        )
        .unwrap();
        let event = Event {
            id: EventId("decision-a".to_string()),
            task_id: TaskId("agent".to_string()),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: "localized summary".to_string(),
            metadata,
        };

        assert_eq!(
            AgentStrategyDecisionReceipt::from_metadata(&event.metadata).unwrap(),
            Some(receipt.clone())
        );
        assert_eq!(
            AgentStrategyDecisionReceipt::from_decision_event(&event).unwrap(),
            receipt
        );
        assert!(receipt.matches_decision_event(&event));

        let mut tampered = event;
        tampered.metadata.insert(
            AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY.to_string(),
            "0".repeat(64),
        );
        assert_eq!(
            AgentStrategyDecisionReceipt::from_decision_event(&tampered),
            Err(AgentStrategyReceiptError::IdentityMismatch)
        );
    }

    #[test]
    fn partial_or_conflicting_receipts_fail_closed() {
        let mut partial = context();
        partial.insert(
            AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY.to_string(),
            AGENT_STRATEGY_RECEIPT_SCHEMA.to_string(),
        );
        assert_eq!(
            AgentStrategyDecisionReceipt::from_metadata(&partial),
            Err(AgentStrategyReceiptError::MalformedReceipt)
        );

        let receipt = AgentStrategyDecisionReceipt::new(
            &TaskId("agent".to_string()),
            &context(),
            &"b".repeat(64),
        )
        .unwrap();
        let mut conflicting = receipt.metadata();
        conflicting.insert(
            AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY.to_string(),
            "c".repeat(64),
        );
        assert!(matches!(
            receipt.insert_into(&mut conflicting),
            Err(AgentStrategyReceiptError::ReservedMetadataConflict(_))
        ));
    }
}
