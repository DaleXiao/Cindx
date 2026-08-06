// Portable receipt construction and bounded persistence contracts.
pub(super) mod persistence;
pub(super) mod replay;

use crate::{
    desktop_prelude::{sha256_hex, Event, EventKind, Metadata, TaskId},
    runtime_constants::AGENT_MEMORY_RECALL_LIMIT,
};
use agent_core::{AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_memory::{
    memory_content_sha256, MemoryEffectReceipt, MemoryEvidenceKind, MemoryInfluenceKind,
    MemoryInfluenceReceipt, MemoryKind, MemoryTrust,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MEMORY_RECALL_ATTRIBUTION_SOURCE_SCHEMA: &str = "cindx.memory-recall-attribution-source.v1";
const MEMORY_ATTRIBUTION_EVENT_SCHEMA: &str = "cindx.memory-attribution-event.v1";
const MEMORY_OBSERVED_CHANNELS_SCHEMA: &str = "cindx.memory-observed-channels.v1";
const MEMORY_ATTRIBUTION_SOURCE_METADATA_KEY: &str = "memory_recall_attribution_source_v1";
const MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY: &str =
    "memory_recall_attribution_source_digest";
const MEMORY_ATTRIBUTION_EVENT_METADATA_KEY: &str = "memory_attribution_event_v1";
const MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY: &str = "memory_attribution_event_digest";
const MEMORY_ATTRIBUTION_EVENT_SUMMARY: &str = "Project memory attribution observed";
const MEMORY_RECALL_EVENT_SUMMARY: &str = "Project memory recalled";
const MEMORY_TERMINAL_EVENT_SUMMARY: &str = "Agent task completed";
const MEMORY_ATTRIBUTION_HASH_DOMAIN: &str = "cindx.memory-attribution.v1\0";
const MEMORY_EVENT_REFERENCE_HASH_DOMAIN: &str = "cindx.memory-event-reference.v1\0";
const MEMORY_ATTRIBUTION_MAX_METADATA_BYTES: usize = 32 * 1024;
const MEMORY_ATTRIBUTION_MAX_ID_BYTES: usize = 192;
const MEMORY_ATTRIBUTION_MAX_GOAL_DELTAS: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemoryRecallAttributionSeed {
    schema: String,
    project_id: String,
    session_id: String,
    logical_run_id: String,
    agent_run_id: String,
    steer_epoch: u64,
    recall_projection_sha256: String,
    task_condition_sha256: String,
    environment_sha256: String,
    memories: Vec<MemoryRecallAttributionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemoryRecallAttributionItem {
    memory_id: String,
    memory_sha256: String,
    durable_user_requirement: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemoryObservedChannels {
    schema: String,
    action_sha256: String,
    goal_delta_fingerprints: Vec<String>,
    successful_tool_evidence: u16,
    verified_postcondition_evidence: u16,
    grounded_completion_digest: String,
    outcome_ledger_digest: String,
    grounded_completion_basis: String,
    influence: Option<MemoryInfluenceKind>,
    evidence: Option<MemoryEvidenceKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedMemoryAttribution {
    memory_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    influence: Option<MemoryInfluenceReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effect: Option<MemoryEffectReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedMemoryAttributionEvent {
    schema: String,
    project_id: String,
    session_id: String,
    logical_run_id: String,
    recall_agent_run_id: String,
    terminal_agent_run_id: String,
    steer_epoch: u64,
    recall_event_id: String,
    recall_event_sha256: String,
    terminal_event_id: String,
    terminal_event_sha256: String,
    observed_channels: MemoryObservedChannels,
    attributions: Vec<PersistedMemoryAttribution>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompletionMemoryAttributionObservation {
    channels: MemoryObservedChannels,
}

impl CompletionMemoryAttributionObservation {
    pub(crate) fn from_runtime(
        runtime: &agent_runtime::AgentLoopState,
        steer_epoch: u64,
        terminal_metadata: &Metadata,
    ) -> Self {
        let mut goal_delta_fingerprints = runtime
            .messages
            .iter()
            .filter(|message| {
                message
                    .metadata
                    .get("steer_epoch")
                    .and_then(|value| value.parse::<u64>().ok())
                    == Some(steer_epoch)
                    && message
                        .metadata
                        .get("goal_delta_schema")
                        .map(String::as_str)
                        == Some(agent_runtime::GOAL_DELTA_SCHEMA)
            })
            .filter_map(|message| message.metadata.get("goal_delta_fingerprint"))
            .filter(|digest| is_sha256_hex(digest))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(MEMORY_ATTRIBUTION_MAX_GOAL_DELTAS)
            .collect::<Vec<_>>();
        goal_delta_fingerprints.sort();
        let successful_tool_evidence =
            bounded_metadata_count(terminal_metadata, "successful_tool_evidence");
        let verified_postcondition_evidence =
            bounded_metadata_count(terminal_metadata, "verified_postcondition_evidence");
        let grounded_completion_digest = terminal_metadata
            .get(agent_runtime::GROUNDED_COMPLETION_DIGEST_METADATA_KEY)
            .filter(|digest| is_sha256_hex(digest))
            .cloned()
            .unwrap_or_default();
        let outcome_ledger_digest = terminal_metadata
            .get(agent_runtime::OUTCOME_LEDGER_DIGEST_METADATA_KEY)
            .filter(|digest| is_sha256_hex(digest))
            .cloned()
            .unwrap_or_default();
        let grounded_completion_basis = terminal_metadata
            .get("grounded_completion_basis")
            .filter(|basis| basis.len() <= 64)
            .cloned()
            .unwrap_or_default();
        let (influence, evidence) = if verified_postcondition_evidence > 0 {
            (
                Some(MemoryInfluenceKind::VerificationObservedInRecalledRun),
                Some(verified_evidence_kind(terminal_metadata)),
            )
        } else if successful_tool_evidence > 0 || !goal_delta_fingerprints.is_empty() {
            (
                Some(MemoryInfluenceKind::ToolObservedInRecalledRun),
                Some(MemoryEvidenceKind::ArtifactReceipt),
            )
        } else if grounded_completion_basis == "constraint_observed" {
            (
                Some(MemoryInfluenceKind::ConstraintObservedInRecalledRun),
                Some(MemoryEvidenceKind::RetrievalReceipt),
            )
        } else {
            (None, None)
        };
        let action_sha256 = observed_channels_sha256(
            &goal_delta_fingerprints,
            successful_tool_evidence,
            verified_postcondition_evidence,
            &grounded_completion_digest,
            &outcome_ledger_digest,
            &grounded_completion_basis,
            influence,
            evidence,
        );
        Self {
            channels: MemoryObservedChannels {
                schema: MEMORY_OBSERVED_CHANNELS_SCHEMA.to_string(),
                action_sha256,
                goal_delta_fingerprints,
                successful_tool_evidence,
                verified_postcondition_evidence,
                grounded_completion_digest,
                outcome_ledger_digest,
                grounded_completion_basis,
                influence,
                evidence,
            },
        }
    }
}

pub(crate) fn insert_memory_recall_attribution_source(
    metadata: &mut Metadata,
    run_context: &Metadata,
    recall_projection_sha256: &str,
    recalls: &[agent_memory::MemoryRecall],
) -> bool {
    let Some((project_id, session_id, logical_run_id, agent_run_id, steer_epoch)) =
        strict_run_scope(run_context)
    else {
        return false;
    };
    let Some(task_condition) = run_context
        .get("effective_prompt_objective")
        .or_else(|| run_context.get("prompt_objective"))
        .or_else(|| metadata.get("query"))
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    if !is_sha256_hex(recall_projection_sha256)
        || recalls.is_empty()
        || recalls.len() > AGENT_MEMORY_RECALL_LIMIT
    {
        return false;
    }
    let mut memory_ids = BTreeSet::new();
    let memories = recalls
        .iter()
        .map(|recall| MemoryRecallAttributionItem {
            memory_id: recall.record.id.clone(),
            memory_sha256: memory_content_sha256(&recall.record.content),
            durable_user_requirement: recall.record.kind == MemoryKind::Requirement
                && recall.record.trust == MemoryTrust::UserStated,
        })
        .collect::<Vec<_>>();
    if memories.iter().any(|item| {
        !valid_id(&item.memory_id)
            || !is_sha256_hex(&item.memory_sha256)
            || !memory_ids.insert(item.memory_id.as_str())
    }) {
        return false;
    }
    let task_condition_sha256 = sha256_hex(task_condition.as_bytes());
    let environment_sha256 = sha256_json(&(
        MEMORY_ATTRIBUTION_HASH_DOMAIN,
        "recall_environment",
        project_id,
        session_id,
        logical_run_id,
        recall_projection_sha256,
    ));
    let seed = MemoryRecallAttributionSeed {
        schema: MEMORY_RECALL_ATTRIBUTION_SOURCE_SCHEMA.to_string(),
        project_id: project_id.to_string(),
        session_id: session_id.to_string(),
        logical_run_id: logical_run_id.to_string(),
        agent_run_id: agent_run_id.to_string(),
        steer_epoch,
        recall_projection_sha256: recall_projection_sha256.to_string(),
        task_condition_sha256,
        environment_sha256,
        memories,
    };
    let Some((encoded, digest)) = encode_with_digest(&seed) else {
        return false;
    };
    metadata.insert(MEMORY_ATTRIBUTION_SOURCE_METADATA_KEY.to_string(), encoded);
    metadata.insert(
        MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY.to_string(),
        digest,
    );
    true
}

fn valid_observed_channels(channels: &MemoryObservedChannels) -> bool {
    if channels.schema != MEMORY_OBSERVED_CHANNELS_SCHEMA
        || !is_sha256_hex(&channels.action_sha256)
        || channels.goal_delta_fingerprints.len() > MEMORY_ATTRIBUTION_MAX_GOAL_DELTAS
        || channels
            .goal_delta_fingerprints
            .iter()
            .any(|digest| !is_sha256_hex(digest))
        || channels
            .goal_delta_fingerprints
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || !is_sha256_hex(&channels.grounded_completion_digest)
        || !is_sha256_hex(&channels.outcome_ledger_digest)
        || channels.grounded_completion_basis.len() > 64
        || channels.influence.is_some() != channels.evidence.is_some()
    {
        return false;
    }
    let channel_is_grounded = match channels.influence {
        Some(MemoryInfluenceKind::VerificationObservedInRecalledRun) => {
            channels.verified_postcondition_evidence > 0
        }
        Some(MemoryInfluenceKind::ToolObservedInRecalledRun) => {
            channels.verified_postcondition_evidence == 0
                && (channels.successful_tool_evidence > 0
                    || !channels.goal_delta_fingerprints.is_empty())
        }
        Some(MemoryInfluenceKind::ConstraintObservedInRecalledRun) => {
            channels.verified_postcondition_evidence == 0
                && channels.successful_tool_evidence == 0
                && channels.goal_delta_fingerprints.is_empty()
                && channels.grounded_completion_basis == "constraint_observed"
        }
        None => {
            channels.verified_postcondition_evidence == 0
                && channels.successful_tool_evidence == 0
                && channels.goal_delta_fingerprints.is_empty()
                && channels.grounded_completion_basis != "constraint_observed"
        }
    };
    if !channel_is_grounded {
        return false;
    }
    channels.action_sha256
        == observed_channels_sha256(
            &channels.goal_delta_fingerprints,
            channels.successful_tool_evidence,
            channels.verified_postcondition_evidence,
            &channels.grounded_completion_digest,
            &channels.outcome_ledger_digest,
            &channels.grounded_completion_basis,
            channels.influence,
            channels.evidence,
        )
}

#[allow(clippy::too_many_arguments)]
fn observed_channels_sha256(
    goal_delta_fingerprints: &[String],
    successful_tool_evidence: u16,
    verified_postcondition_evidence: u16,
    grounded_completion_digest: &str,
    outcome_ledger_digest: &str,
    grounded_completion_basis: &str,
    influence: Option<MemoryInfluenceKind>,
    evidence: Option<MemoryEvidenceKind>,
) -> String {
    sha256_json(&(
        MEMORY_ATTRIBUTION_HASH_DOMAIN,
        MEMORY_OBSERVED_CHANNELS_SCHEMA,
        goal_delta_fingerprints,
        successful_tool_evidence,
        verified_postcondition_evidence,
        grounded_completion_digest,
        outcome_ledger_digest,
        grounded_completion_basis,
        influence,
        evidence,
    ))
}

fn memory_event_reference_sha256(event: &Event, semantic_digest: &str) -> String {
    sha256_json(&(
        MEMORY_EVENT_REFERENCE_HASH_DOMAIN,
        event.id.0.as_str(),
        event.task_id.0.as_str(),
        event.sequence,
        event.timestamp_ms,
        event_kind_label(&event.kind),
        event.summary.as_str(),
        event.metadata.get("project_id").map(String::as_str),
        event.metadata.get("session_id").map(String::as_str),
        event
            .metadata
            .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
            .map(String::as_str),
        event
            .metadata
            .get(AGENT_RUN_ID_METADATA_KEY)
            .map(String::as_str),
        event.metadata.get("steer_epoch").map(String::as_str),
        semantic_digest,
    ))
}

fn verified_evidence_kind(metadata: &Metadata) -> MemoryEvidenceKind {
    let postconditions = metadata
        .get(agent_runtime::OUTCOME_LEDGER_METADATA_KEY)
        .and_then(|encoded| serde_json::from_str::<serde_json::Value>(encoded).ok())
        .and_then(|value| value.get("postconditions").cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    if postconditions.iter().any(|item| {
        item.get("status").and_then(|value| value.as_str()) == Some("verified")
            && item.get("kind").and_then(|value| value.as_str()) == Some("browser_interaction")
    }) {
        MemoryEvidenceKind::BrowserReceipt
    } else if postconditions.iter().any(|item| {
        item.get("status").and_then(|value| value.as_str()) == Some("verified")
            && item.get("kind").and_then(|value| value.as_str()) == Some("workspace_mutation")
    }) {
        MemoryEvidenceKind::WorkspaceRevision
    } else {
        MemoryEvidenceKind::ArtifactReceipt
    }
}

fn strict_run_scope(metadata: &Metadata) -> Option<(&str, &str, &str, &str, u64)> {
    let project_id = metadata.get("project_id").map(String::as_str)?;
    let session_id = metadata.get("session_id").map(String::as_str)?;
    let logical_run_id = metadata
        .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
        .map(String::as_str)?;
    let agent_run_id = metadata
        .get(AGENT_RUN_ID_METADATA_KEY)
        .map(String::as_str)?;
    let steer_epoch = metadata.get("steer_epoch")?.parse::<u64>().ok()?;
    (valid_id(project_id)
        && valid_id(session_id)
        && valid_id(logical_run_id)
        && valid_id(agent_run_id))
    .then_some((
        project_id,
        session_id,
        logical_run_id,
        agent_run_id,
        steer_epoch,
    ))
}

#[allow(clippy::too_many_arguments)]
fn memory_event_has_scope(
    event: &Event,
    task_id: &TaskId,
    project_id: &str,
    session_id: &str,
    logical_run_id: &str,
    agent_run_id: &str,
    steer_epoch: u64,
) -> bool {
    memory_event_has_logical_scope(
        event,
        task_id,
        project_id,
        session_id,
        logical_run_id,
        steer_epoch,
    ) && event
        .metadata
        .get(AGENT_RUN_ID_METADATA_KEY)
        .map(String::as_str)
        == Some(agent_run_id)
}

fn memory_event_has_logical_scope(
    event: &Event,
    task_id: &TaskId,
    project_id: &str,
    session_id: &str,
    logical_run_id: &str,
    steer_epoch: u64,
) -> bool {
    event.task_id == *task_id
        && event.metadata.get("project_id").map(String::as_str) == Some(project_id)
        && event.metadata.get("session_id").map(String::as_str) == Some(session_id)
        && event
            .metadata
            .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
            .map(String::as_str)
            == Some(logical_run_id)
        && event
            .metadata
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            == Some(steer_epoch)
}

fn bounded_metadata_count(metadata: &Metadata, key: &str) -> u16 {
    metadata
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.min(u16::MAX as usize) as u16)
        .unwrap_or_default()
}

fn encode_with_digest<T: Serialize>(value: &T) -> Option<(String, String)> {
    let encoded = serde_json::to_string(value).ok()?;
    (encoded.len() <= MEMORY_ATTRIBUTION_MAX_METADATA_BYTES)
        .then(|| (encoded.clone(), sha256_hex(encoded.as_bytes())))
}

fn decode_with_digest<T: for<'de> Deserialize<'de>>(encoded: &str, digest: &str) -> Option<T> {
    (encoded.len() <= MEMORY_ATTRIBUTION_MAX_METADATA_BYTES
        && is_sha256_hex(digest)
        && sha256_hex(encoded.as_bytes()) == digest)
        .then(|| serde_json::from_str(encoded).ok())
        .flatten()
}

fn sha256_json<T: Serialize>(value: &T) -> String {
    serde_json::to_vec(value)
        .map(|encoded| sha256_hex(&encoded))
        .unwrap_or_default()
}

fn valid_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= MEMORY_ATTRIBUTION_MAX_ID_BYTES
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn event_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::TaskCreated => "task_created",
        EventKind::TaskStatusChanged => "task_status_changed",
        EventKind::MessageAdded => "message_added",
        EventKind::ModelRequestStarted => "model_request_started",
        EventKind::ModelRequestFinished => "model_request_finished",
        EventKind::ToolCallProposed => "tool_call_proposed",
        EventKind::ToolCallStarted => "tool_call_started",
        EventKind::ToolCallFinished => "tool_call_finished",
        EventKind::PermissionRequested => "permission_requested",
        EventKind::PermissionResolved => "permission_resolved",
        EventKind::RetrievalPerformed => "retrieval_performed",
        EventKind::Error => "error",
    }
}
