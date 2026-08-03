use crate::{
    task_state::{
        AgentTaskStateError, AgentTaskStateSnapshot, AGENT_TASK_STATE_SCHEMA,
        AGENT_TASK_STATE_SCHEMA_V1,
    },
    AgentTaskContract, InteractionSurface, PreparedTaskState, PromptEvidenceScope,
    PromptToolRequirement,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

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
pub struct PreparedTaskStateCheckpoint {
    pub objective_fingerprint: String,
    pub steer_epoch: u64,
    pub contract_epoch: u64,
    pub evidence_scopes: BTreeSet<PromptEvidenceScope>,
    pub tool_requirement: PromptToolRequirement,
}

impl PreparedTaskStateCheckpoint {
    pub(crate) fn capture(prepared: &PreparedTaskState) -> Self {
        Self {
            objective_fingerprint: prepared.objective_fingerprint().to_string(),
            steer_epoch: prepared.steer_epoch(),
            contract_epoch: prepared.contract_epoch(),
            evidence_scopes: prepared.completion_intent().evidence_scopes.clone(),
            tool_requirement: prepared.completion_intent().tool_requirement,
        }
    }

    pub(crate) fn matches(&self, prepared: &PreparedTaskState) -> bool {
        self == &Self::capture(prepared)
    }

    pub(crate) fn restore_with_objective(&self, effective_objective: String) -> PreparedTaskState {
        let run_context = [
            (
                "effective_prompt_objective".to_string(),
                effective_objective.clone(),
            ),
            ("steer_epoch".to_string(), self.steer_epoch.to_string()),
            (
                "prompt_contract_epoch".to_string(),
                self.contract_epoch.to_string(),
            ),
        ]
        .into_iter()
        .collect();
        let mut completion_intent = crate::prompt_completion_intent(&run_context);
        completion_intent.evidence_scopes = self.evidence_scopes.clone();
        completion_intent.tool_requirement = self.tool_requirement;
        PreparedTaskState::from_persisted(
            effective_objective,
            self.steer_epoch,
            self.contract_epoch,
            completion_intent,
        )
    }

    pub(crate) fn validate(&self) -> Result<(), AgentTaskStateError> {
        if !is_sha256_fingerprint(&self.objective_fingerprint) {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint contains an invalid objective fingerprint",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskStateSnapshotV2Ref<'a> {
    schema: &'a str,
    task_id: &'a str,
    user_prompt_fingerprint: &'a str,
    transcript_fingerprint: &'a str,
    durable_message_count: usize,
    turn: usize,
    max_turns: usize,
    failed_tool_signatures: &'a BTreeMap<String, usize>,
    consecutive_empty_responses: usize,
    verification_gate_requests: usize,
    verified_interactions: usize,
    interaction_verification_gate_requests: usize,
    task_contract: &'a AgentTaskContract,
    prepared_task_state: &'a PreparedTaskStateCheckpoint,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskStateSnapshotV2 {
    schema: String,
    task_id: String,
    user_prompt_fingerprint: String,
    transcript_fingerprint: String,
    durable_message_count: usize,
    turn: usize,
    max_turns: usize,
    failed_tool_signatures: BTreeMap<String, usize>,
    consecutive_empty_responses: usize,
    verification_gate_requests: usize,
    verified_interactions: usize,
    interaction_verification_gate_requests: usize,
    task_contract: AgentTaskContract,
    prepared_task_state: PreparedTaskStateCheckpoint,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskStateSnapshotV1Ref<'a> {
    schema: &'a str,
    task_id: &'a str,
    user_prompt_fingerprint: &'a str,
    transcript_fingerprint: &'a str,
    durable_message_count: usize,
    turn: usize,
    max_turns: usize,
    failed_tool_signatures: &'a BTreeMap<String, usize>,
    consecutive_empty_responses: usize,
    successful_mutations: usize,
    verified_after_last_mutation: bool,
    verification_gate_requests: usize,
    pending_interaction_verifications: Vec<PersistedInteractionVerification>,
    verified_interactions: usize,
    interaction_verification_gate_requests: usize,
    task_contract: &'a AgentTaskContract,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskStateSnapshotV1 {
    schema: String,
    task_id: String,
    user_prompt_fingerprint: String,
    transcript_fingerprint: String,
    durable_message_count: usize,
    turn: usize,
    max_turns: usize,
    failed_tool_signatures: BTreeMap<String, usize>,
    consecutive_empty_responses: usize,
    successful_mutations: usize,
    verified_after_last_mutation: bool,
    verification_gate_requests: usize,
    pending_interaction_verifications: Vec<PersistedInteractionVerification>,
    verified_interactions: usize,
    interaction_verification_gate_requests: usize,
    #[serde(default)]
    task_contract: AgentTaskContract,
}

impl Serialize for AgentTaskStateSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.schema.as_str() {
            AGENT_TASK_STATE_SCHEMA => {
                let prepared_task_state = self.prepared_task_state.as_ref().ok_or_else(|| {
                    serde::ser::Error::custom(
                        "v2 agent task checkpoint is missing prepared task state",
                    )
                })?;
                AgentTaskStateSnapshotV2Ref {
                    schema: &self.schema,
                    task_id: &self.task_id,
                    user_prompt_fingerprint: &self.user_prompt_fingerprint,
                    transcript_fingerprint: &self.transcript_fingerprint,
                    durable_message_count: self.durable_message_count,
                    turn: self.turn,
                    max_turns: self.max_turns,
                    failed_tool_signatures: &self.failed_tool_signatures,
                    consecutive_empty_responses: self.consecutive_empty_responses,
                    verification_gate_requests: self.verification_gate_requests,
                    verified_interactions: self.verified_interactions,
                    interaction_verification_gate_requests: self
                        .interaction_verification_gate_requests,
                    task_contract: &self.task_contract,
                    prepared_task_state,
                }
                .serialize(serializer)
            }
            AGENT_TASK_STATE_SCHEMA_V1 => {
                let pending_interaction_verifications = self
                    .task_contract
                    .pending_interactions()
                    .iter()
                    .map(|(surface, action_tool)| PersistedInteractionVerification {
                        surface: (*surface).into(),
                        action_tool: action_tool.clone(),
                    })
                    .collect();
                AgentTaskStateSnapshotV1Ref {
                    schema: &self.schema,
                    task_id: &self.task_id,
                    user_prompt_fingerprint: &self.user_prompt_fingerprint,
                    transcript_fingerprint: &self.transcript_fingerprint,
                    durable_message_count: self.durable_message_count,
                    turn: self.turn,
                    max_turns: self.max_turns,
                    failed_tool_signatures: &self.failed_tool_signatures,
                    consecutive_empty_responses: self.consecutive_empty_responses,
                    successful_mutations: self.task_contract.successful_mutations(),
                    verified_after_last_mutation: self.task_contract.latest_mutation_verified(),
                    verification_gate_requests: self.verification_gate_requests,
                    pending_interaction_verifications,
                    verified_interactions: self.verified_interactions,
                    interaction_verification_gate_requests: self
                        .interaction_verification_gate_requests,
                    task_contract: &self.task_contract,
                }
                .serialize(serializer)
            }
            schema => Err(serde::ser::Error::custom(format!(
                "unsupported agent task checkpoint schema: {schema}"
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for AgentTaskStateSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let schema = value
            .get("schema")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::custom("agent task checkpoint is missing its schema"))?;
        let snapshot = match schema {
            AGENT_TASK_STATE_SCHEMA => {
                let decoded = serde_json::from_value::<AgentTaskStateSnapshotV2>(value)
                    .map_err(D::Error::custom)?;
                AgentTaskStateSnapshot {
                    schema: decoded.schema,
                    task_id: decoded.task_id,
                    user_prompt_fingerprint: decoded.user_prompt_fingerprint,
                    transcript_fingerprint: decoded.transcript_fingerprint,
                    durable_message_count: decoded.durable_message_count,
                    turn: decoded.turn,
                    max_turns: decoded.max_turns,
                    failed_tool_signatures: decoded.failed_tool_signatures,
                    consecutive_empty_responses: decoded.consecutive_empty_responses,
                    verification_gate_requests: decoded.verification_gate_requests,
                    verified_interactions: decoded.verified_interactions,
                    interaction_verification_gate_requests: decoded
                        .interaction_verification_gate_requests,
                    task_contract: decoded.task_contract,
                    prepared_task_state: Some(decoded.prepared_task_state),
                }
            }
            AGENT_TASK_STATE_SCHEMA_V1 => {
                let decoded = serde_json::from_value::<AgentTaskStateSnapshotV1>(value)
                    .map_err(D::Error::custom)?;
                let pending_interactions =
                    legacy_pending_interactions(&decoded.pending_interaction_verifications)
                        .map_err(D::Error::custom)?;
                let task_contract = if decoded.task_contract == AgentTaskContract::default()
                    && (decoded.successful_mutations > 0 || !pending_interactions.is_empty())
                {
                    AgentTaskContract::restore_legacy(
                        decoded.successful_mutations,
                        decoded.verified_after_last_mutation,
                        pending_interactions,
                    )
                } else {
                    decoded.task_contract
                };
                AgentTaskStateSnapshot {
                    schema: decoded.schema,
                    task_id: decoded.task_id,
                    user_prompt_fingerprint: decoded.user_prompt_fingerprint,
                    transcript_fingerprint: decoded.transcript_fingerprint,
                    durable_message_count: decoded.durable_message_count,
                    turn: decoded.turn,
                    max_turns: decoded.max_turns,
                    failed_tool_signatures: decoded.failed_tool_signatures,
                    consecutive_empty_responses: decoded.consecutive_empty_responses,
                    verification_gate_requests: decoded.verification_gate_requests,
                    verified_interactions: decoded.verified_interactions,
                    interaction_verification_gate_requests: decoded
                        .interaction_verification_gate_requests,
                    task_contract,
                    prepared_task_state: None,
                }
            }
            schema => {
                return Err(D::Error::custom(format!(
                    "unsupported agent task checkpoint schema: {schema}"
                )))
            }
        };
        snapshot.validate().map_err(D::Error::custom)?;
        Ok(snapshot)
    }
}

fn legacy_pending_interactions(
    pending: &[PersistedInteractionVerification],
) -> Result<BTreeMap<InteractionSurface, String>, AgentTaskStateError> {
    let mut restored = BTreeMap::new();
    for pending in pending {
        let surface: InteractionSurface = pending.surface.clone().into();
        if pending.action_tool.trim().is_empty() {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint contains an empty interaction tool",
            ));
        }
        if restored
            .insert(surface, pending.action_tool.clone())
            .is_some()
        {
            return Err(AgentTaskStateError::new(
                "agent task checkpoint contains duplicate interaction surfaces",
            ));
        }
    }
    Ok(restored)
}

fn is_sha256_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
