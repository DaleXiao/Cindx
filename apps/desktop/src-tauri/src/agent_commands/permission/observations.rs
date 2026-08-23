use crate::*;

const PERMISSION_TOOL_OBSERVATION_SCHEMA: &str = "cindx.permission-tool-observation.v1";
const PERMISSION_TOOL_OBSERVATION_PROVENANCE: &str = "runtime_permission_resolution";

fn permission_prompt_contract_epoch(run_context: &Metadata) -> u64 {
    run_context
        .get("prompt_contract_epoch")
        .or_else(|| run_context.get("steer_epoch"))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn permission_tool_observation_metadata(
    request_id: &PermissionRequestId,
    tool_call_id: &str,
    tool_name: &str,
    status: &ToolOutcomeStatus,
    tool_input: &str,
    risk: Option<&ToolRisk>,
    effect_spec: Option<&ToolSpec>,
    postcondition_evidence: Option<&agent_core::ToolPostconditionEvidence>,
    run_context: &Metadata,
) -> Metadata {
    let input_fingerprint = agent_runtime::tool_input_fingerprint(tool_name, tool_input);
    let mut metadata: Metadata = [
        ("kind".to_string(), "tool_observation".to_string()),
        ("tool_call_id".to_string(), tool_call_id.to_string()),
        ("tool".to_string(), tool_name.to_string()),
        ("status".to_string(), tool_outcome_label(status).to_string()),
        ("permission_id".to_string(), request_id.0.clone()),
        (
            "permission_observation_schema".to_string(),
            PERMISSION_TOOL_OBSERVATION_SCHEMA.to_string(),
        ),
        (
            "permission_observation_provenance".to_string(),
            PERMISSION_TOOL_OBSERVATION_PROVENANCE.to_string(),
        ),
        (
            "tool_input_fingerprint".to_string(),
            input_fingerprint.clone(),
        ),
        (
            "prompt_contract_epoch".to_string(),
            permission_prompt_contract_epoch(run_context).to_string(),
        ),
    ]
    .into_iter()
    .collect();
    if matches!(status, ToolOutcomeStatus::Denied) {
        metadata.extend([
            (
                "action_denial_schema".to_string(),
                agent_runtime::ACTION_DENIAL_SCHEMA.to_string(),
            ),
            (
                "action_denial_kind".to_string(),
                "user_permission".to_string(),
            ),
            (
                "action_denial_code".to_string(),
                "user_permission_denied".to_string(),
            ),
            (
                "action_denial_recovery".to_string(),
                "finalize_blocked".to_string(),
            ),
        ]);
    }
    let completion_intent = agent_runtime::prompt_completion_intent(run_context);
    if let Some(witness) = agent_runtime::evidence_target_witness(
        tool_input,
        &completion_intent.target_anchors,
        tool_name,
        &input_fingerprint,
        permission_prompt_contract_epoch(run_context),
    ) {
        metadata.insert("evidence_target_witness".to_string(), witness);
    }
    if matches!(status, ToolOutcomeStatus::Succeeded) {
        if let Some(witness) = agent_runtime::postcondition_lineage_scope(run_context)
            .and_then(|lineage_scope| {
                agent_runtime::PersistedToolEffectWitness::capture_with_postcondition_evidence(
                    tool_name,
                    tool_input,
                    risk,
                    &lineage_scope,
                    effect_spec,
                    postcondition_evidence,
                )
            })
            .and_then(|witness| witness.encode())
        {
            metadata.insert(
                agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY.to_string(),
                witness,
            );
        }
    }
    metadata_with_context(metadata, run_context)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PersistedPermissionObservation {
    pub(super) message_index: usize,
    pub(super) permission_id: PermissionRequestId,
    pub(super) call_id: agent_core::ToolCallId,
    pub(super) tool_name: String,
    pub(super) input_fingerprint: String,
    pub(super) target_witness: Option<String>,
    pub(super) effect_witness: Option<agent_runtime::PersistedToolEffectWitness>,
    pub(super) status: ToolOutcomeStatus,
    pub(super) observation: String,
}

fn persisted_permission_observation_payload_matches(
    left: &PersistedPermissionObservation,
    right: &PersistedPermissionObservation,
) -> bool {
    left.permission_id == right.permission_id
        && left.call_id == right.call_id
        && left.tool_name == right.tool_name
        && left.input_fingerprint == right.input_fingerprint
        && left.target_witness == right.target_witness
        && left.effect_witness == right.effect_witness
        && left.status == right.status
        && left.observation == right.observation
}

pub(super) fn copy_replayed_contract_evidence_metadata(
    runtime: &agent_runtime::AgentLoopState,
    transcript: &mut [Message],
    message_index: usize,
) {
    let Some(source) = runtime.messages.last() else {
        return;
    };
    let Some(target) = transcript.get_mut(message_index) else {
        return;
    };
    for key in [
        agent_runtime::CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY,
        "contract_evidence_sequence",
    ] {
        if let Some(value) = source.metadata.get(key) {
            target.metadata.insert(key.to_string(), value.clone());
        }
    }
}

fn persisted_tool_outcome_status(value: &str) -> Option<ToolOutcomeStatus> {
    match value {
        "succeeded" => Some(ToolOutcomeStatus::Succeeded),
        "failed" => Some(ToolOutcomeStatus::Failed),
        "cancelled" => Some(ToolOutcomeStatus::Cancelled),
        "denied" => Some(ToolOutcomeStatus::Denied),
        _ => None,
    }
}

pub(super) fn persisted_permission_observations(
    transcript: &[Message],
    run_context: &Metadata,
) -> Vec<PersistedPermissionObservation> {
    let Some(session_id) = run_context.get("session_id") else {
        return Vec::new();
    };
    let Some(agent_run_id) = run_context.get("agent_run_id") else {
        return Vec::new();
    };
    let prompt_contract_epoch = permission_prompt_contract_epoch(run_context).to_string();

    let candidates = transcript
        .iter()
        .enumerate()
        .filter_map(|(message_index, message)| {
            if message.role != MessageRole::Tool
                || message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
                || message
                    .metadata
                    .get("permission_observation_schema")
                    .map(String::as_str)
                    != Some(PERMISSION_TOOL_OBSERVATION_SCHEMA)
                || message
                    .metadata
                    .get("permission_observation_provenance")
                    .map(String::as_str)
                    != Some(PERMISSION_TOOL_OBSERVATION_PROVENANCE)
                || message.metadata.get("session_id") != Some(session_id)
                || message.metadata.get("agent_run_id") != Some(agent_run_id)
                || message
                    .metadata
                    .get("prompt_contract_epoch")
                    .map(String::as_str)
                    != Some(prompt_contract_epoch.as_str())
                || message
                    .metadata
                    .get("permission_id")
                    .is_none_or(|value| value.trim().is_empty())
            {
                return None;
            }
            let permission_id = message
                .metadata
                .get("permission_id")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let call_id = message
                .metadata
                .get("tool_call_id")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let tool_name = message
                .metadata
                .get("tool")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let input_fingerprint = message
                .metadata
                .get("tool_input_fingerprint")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let status = persisted_tool_outcome_status(message.metadata.get("status")?)?;
            let effect_witness = match message
                .metadata
                .get(agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY)
            {
                Some(encoded) => Some(agent_runtime::PersistedToolEffectWitness::decode(encoded)?),
                None => None,
            };
            Some(PersistedPermissionObservation {
                message_index,
                permission_id: PermissionRequestId(permission_id),
                call_id: agent_core::ToolCallId(call_id),
                tool_name,
                input_fingerprint,
                target_witness: message.metadata.get("evidence_target_witness").cloned(),
                effect_witness,
                status,
                observation: message.content.clone(),
            })
        })
        .collect::<Vec<_>>();
    let mut first_by_id = BTreeMap::<String, usize>::new();
    let mut conflicting_ids = BTreeSet::new();
    for (index, observation) in candidates.iter().enumerate() {
        match first_by_id.get(&observation.permission_id.0).copied() {
            Some(first_index)
                if !persisted_permission_observation_payload_matches(
                    &candidates[first_index],
                    observation,
                ) =>
            {
                conflicting_ids.insert(observation.permission_id.0.clone());
            }
            Some(_) => {}
            None => {
                first_by_id.insert(observation.permission_id.0.clone(), index);
            }
        }
    }
    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, observation)| {
            (!conflicting_ids.contains(&observation.permission_id.0)
                && first_by_id.get(&observation.permission_id.0) == Some(&index))
            .then_some(observation)
        })
        .collect()
}

pub(super) fn current_permission_observation_boundary(
    observations: &[PersistedPermissionObservation],
    current_permission_ids: &BTreeSet<String>,
    transcript_len: usize,
) -> usize {
    observations
        .iter()
        .find(|observation| current_permission_ids.contains(&observation.permission_id.0))
        .map(|observation| observation.message_index)
        .unwrap_or(transcript_len)
}

pub(super) fn permission_checkpoint_message_boundary(
    events: &[Event],
    recovery: &AgentRecoveryEnvelope,
    snapshot: &AgentTaskStateSnapshot,
) -> Option<usize> {
    let mut message_count = 0usize;
    for event in events {
        if event.kind == EventKind::MessageAdded {
            let role = event
                .metadata
                .get("role")
                .and_then(|role| message_role_from_label(role));
            let has_content = if role == Some(MessageRole::User) {
                event.metadata.contains_key("model_content")
                    || event.metadata.contains_key("content")
            } else {
                role.is_some() && event.metadata.contains_key("content")
            };
            if has_content {
                message_count = message_count.saturating_add(1);
            }
        }
        if event
            .metadata
            .get("recovery_resume_key")
            .map(String::as_str)
            != Some(recovery.identity.resume_key.as_str())
            || event.metadata.get("recovery_state").map(String::as_str) != Some("blocked")
        {
            continue;
        }
        let matches_snapshot = event
            .metadata
            .get("recovery_envelope")
            .and_then(|encoded| serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok())
            .is_some_and(|checkpoint| checkpoint.task_state.as_ref() == Some(snapshot));
        if matches_snapshot {
            return Some(message_count);
        }
    }
    None
}

pub(super) fn restore_permission_snapshot_from_boundary(
    snapshot: &AgentTaskStateSnapshot,
    recovery_prompt: &str,
    effective_objective: &str,
    transcript: &[Message],
    boundary: usize,
) -> Option<(agent_runtime::AgentLoopState, usize)> {
    let boundary = boundary.min(transcript.len());
    snapshot
        .restore_with_effective_objective(
            recovery_prompt.to_string(),
            transcript[..boundary].to_vec(),
            effective_objective,
        )
        .ok()
        .map(|runtime| (runtime, boundary))
}
