use crate::{AgentLoopState, ContractEvidence};
use agent_core::{Message, MessageRole};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY: &str = "contract_evidence_sequences_json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequiredEvidenceCapsule {
    pub sequences: Vec<u64>,
    pub sources: Vec<String>,
    pub observation: String,
}

pub fn message_contract_evidence_sequences(message: &Message) -> Vec<u64> {
    let mut sequences = message
        .metadata
        .get(CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY)
        .and_then(|value| serde_json::from_str::<Vec<u64>>(value).ok())
        .unwrap_or_default();
    if let Some(sequence) = message
        .metadata
        .get("contract_evidence_sequence")
        .and_then(|value| value.parse::<u64>().ok())
    {
        sequences.push(sequence);
    }
    sequences.sort_unstable();
    sequences.dedup();
    sequences
}

pub(crate) fn annotate_latest_tool_observation(
    messages: &mut [Message],
    evidence: &[ContractEvidence],
) {
    if evidence.is_empty() {
        return;
    }
    let Some(message) = messages
        .last_mut()
        .filter(|message| message.role == MessageRole::Tool)
    else {
        return;
    };
    let sequences = evidence
        .iter()
        .map(|item| item.sequence)
        .collect::<Vec<_>>();
    if let Ok(encoded) = serde_json::to_string(&sequences) {
        message.metadata.insert(
            CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY.to_string(),
            encoded,
        );
    }
}

pub(crate) fn required_tool_evidence_capsules(
    state: &AgentLoopState,
    steer_epoch: u64,
    already_visible: &BTreeSet<u64>,
) -> Vec<RequiredEvidenceCapsule> {
    let required = state
        .task_contract
        .grounded_completion_required_evidence_sequences(steer_epoch)
        .into_iter()
        .collect::<BTreeSet<_>>();
    if required.is_subset(already_visible) {
        return Vec::new();
    }
    let evidence_sources = state
        .task_contract
        .evidence()
        .iter()
        .map(|item| (item.sequence, item.source.clone()))
        .collect::<BTreeMap<_, _>>();
    let legacy_sequences = legacy_unique_tool_carrier_sequences(state, &required);
    let mut covered = already_visible.clone();
    let mut capsules = Vec::new();
    for (message_index, message) in state.messages.iter().enumerate().rev() {
        if message.role != MessageRole::Tool || message.content.trim().is_empty() {
            continue;
        }
        let sequences = message_contract_evidence_sequences(message)
            .into_iter()
            .chain(
                legacy_sequences
                    .get(&message_index)
                    .into_iter()
                    .flatten()
                    .copied(),
            )
            .filter(|sequence| required.contains(sequence) && !covered.contains(sequence))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if sequences.is_empty() {
            continue;
        }
        let sources = sequences
            .iter()
            .filter_map(|sequence| evidence_sources.get(sequence).cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        covered.extend(sequences.iter().copied());
        capsules.push(RequiredEvidenceCapsule {
            sequences,
            sources,
            observation: message.content.clone(),
        });
        if required.is_subset(&covered) {
            break;
        }
    }
    capsules.reverse();
    capsules
}

fn legacy_unique_tool_carrier_sequences(
    state: &AgentLoopState,
    required: &BTreeSet<u64>,
) -> BTreeMap<usize, Vec<u64>> {
    let mut call_inputs = BTreeMap::<String, String>::new();
    for message in &state.messages {
        if message.role != MessageRole::Assistant {
            continue;
        }
        let Some(raw_calls) = message.metadata.get("raw_tool_calls_json") else {
            continue;
        };
        let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(raw_calls) else {
            continue;
        };
        for call in calls {
            let Some(call_id) = call.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let input = call
                .pointer("/function/arguments")
                .or_else(|| call.get("arguments_json"))
                .or_else(|| call.get("input"))
                .and_then(serde_json::Value::as_str);
            if let Some(input) = input {
                call_inputs.insert(call_id.to_string(), input.to_string());
            }
        }
    }

    let mut carriers = BTreeMap::<(String, String), Vec<usize>>::new();
    for (index, message) in state.messages.iter().enumerate() {
        if message.role != MessageRole::Tool
            || !trusted_legacy_tool_carrier(message)
            || message
                .metadata
                .contains_key(CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY)
        {
            continue;
        }
        let Some(call_id) = message.metadata.get("tool_call_id") else {
            continue;
        };
        let Some(input) = call_inputs.get(call_id) else {
            continue;
        };
        let Some(source) = message
            .metadata
            .get("tool_name")
            .or_else(|| message.metadata.get("tool"))
        else {
            continue;
        };
        carriers
            .entry((source.clone(), raw_input_fingerprint(input)))
            .or_default()
            .push(index);
    }

    let evidence = state.task_contract.evidence();
    carriers
        .into_iter()
        .filter_map(|((source, input_fingerprint), indices)| {
            let [message_index] = indices.as_slice() else {
                return None;
            };
            let message = &state.messages[*message_index];
            let existing = message_contract_evidence_sequences(message);
            if existing.is_empty()
                || !existing.iter().any(|sequence| {
                    evidence.iter().any(|item| {
                        item.sequence == *sequence
                            && item.source == source
                            && item.input_fingerprint == input_fingerprint
                    })
                })
            {
                return None;
            }
            let sequences = evidence
                .iter()
                .filter(|item| {
                    item.source == source
                        && item.input_fingerprint == input_fingerprint
                        && required.contains(&item.sequence)
                })
                .map(|item| item.sequence)
                .collect::<Vec<_>>();
            (!sequences.is_empty()).then_some((*message_index, sequences))
        })
        .collect()
}

fn trusted_legacy_tool_carrier(message: &Message) -> bool {
    let live = message
        .metadata
        .get("tool_evidence_schema")
        .map(String::as_str)
        == Some("cindx.tool_evidence.v1")
        && message
            .metadata
            .get("tool_evidence_provenance")
            .map(String::as_str)
            == Some("runtime_dispatch")
        && message.metadata.get("tool_status").map(String::as_str) == Some("succeeded");
    let recovered = message
        .metadata
        .get("permission_observation_schema")
        .map(String::as_str)
        == Some("cindx.permission-tool-observation.v1")
        && message
            .metadata
            .get("permission_observation_provenance")
            .map(String::as_str)
            == Some("runtime_permission_resolution")
        && message.metadata.get("status").map(String::as_str) == Some("succeeded");
    live || recovered
}

fn raw_input_fingerprint(input: &str) -> String {
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

pub(crate) fn visible_required_evidence_sequences(
    messages: &[Message],
    required: &BTreeSet<u64>,
) -> Vec<u64> {
    if required.is_empty() {
        return Vec::new();
    }
    messages
        .iter()
        .filter(|message| !message.content.trim().is_empty())
        .flat_map(message_contract_evidence_sequences)
        .filter(|sequence| required.contains(sequence))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{start_agent_loop, AgentKernel, AgentRuntimeConfig, AgentToolRequest};
    use agent_core::{Metadata, TaskId, ToolCallId, ToolOutcomeStatus, ToolRisk, ToolSpec};

    #[test]
    fn legacy_singular_carrier_recovers_a_unique_multi_lineage_call() {
        let mut state = start_agent_loop(
            TaskId("legacy-grounding-carrier".to_string()),
            "read README.md",
            AgentRuntimeConfig::default(),
        );
        state.task_contract.require_tool_success("file.read");
        let tools = vec![ToolSpec::builtin(
            "file.read",
            "file",
            "read",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )];
        let input = r#"{"path":"README.md"}"#;
        state.messages.push(Message {
            role: MessageRole::Assistant,
            content: String::new(),
            metadata: [(
                "raw_tool_calls_json".to_string(),
                format!(
                    r#"[{{"id":"read-1","type":"function","function":{{"name":"file_read","arguments":{}}}}}]"#,
                    serde_json::to_string(input).unwrap()
                ),
            )]
            .into_iter()
            .collect::<Metadata>(),
        });
        let mut kernel = AgentKernel::new(&mut state, &tools);
        kernel.replace_prompt_evidence_requirement(0, Some("workspace"), ["file.read"]);
        kernel.apply_tool_observation(
            &AgentToolRequest {
                call_id: ToolCallId("read-1".to_string()),
                tool_name: "file.read".to_string(),
                input: input.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
            "substantive README evidence",
        );
        let required = kernel
            .state()
            .task_contract
            .grounded_completion_required_evidence_sequences(0);
        let tool_message = kernel.state_mut().messages.last_mut().unwrap();
        tool_message
            .metadata
            .remove(CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY);
        tool_message.metadata.extend([
            (
                "contract_evidence_sequence".to_string(),
                required.last().unwrap().to_string(),
            ),
            ("tool_name".to_string(), "file.read".to_string()),
            (
                "tool_evidence_schema".to_string(),
                "cindx.tool_evidence.v1".to_string(),
            ),
            (
                "tool_evidence_provenance".to_string(),
                "runtime_dispatch".to_string(),
            ),
            ("tool_status".to_string(), "succeeded".to_string()),
        ]);

        let capsules = required_tool_evidence_capsules(kernel.state(), 0, &BTreeSet::new());
        assert_eq!(capsules.len(), 1);
        assert_eq!(capsules[0].sequences, required);
    }
}
