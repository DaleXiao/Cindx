use crate::{is_agent_run_start_event, sha256_hex, EventKind};
use agent_core::{Event, AgentRunLineage};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const TOOL_CALL_DOMAIN: &[u8] = b"cindx.agent-realworld-tool-call.v1\0";
const TOOL_ARTIFACT_PATH_DOMAIN: &[u8] = b"cindx.agent-realworld-artifact-path.v1\0";
const TOOL_EVIDENCE_DOMAIN: &[u8] = b"cindx.agent-realworld-tool-evidence.v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ToolReceiptStatus {
    Succeeded,
    Failed,
    Cancelled,
    Denied,
    Incomplete,
    Superseded,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ToolArtifactReceipt {
    pub(super) path_sha256: String,
    pub(super) content_sha256: String,
    pub(super) bytes: u64,
    pub(super) mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ToolAttemptReceipt {
    pub(super) call_sha256: String,
    pub(super) tool: String,
    pub(super) status: ToolReceiptStatus,
    pub(super) input_fingerprint: Option<String>,
    pub(super) started_sequence: Option<u64>,
    pub(super) finished_sequence: Option<u64>,
    pub(super) target_sha256: Option<String>,
    pub(super) evidence_sha256: Option<String>,
    pub(super) artifacts: Vec<ToolArtifactReceipt>,
}

#[derive(Debug, Default)]
pub(super) struct ToolReceiptProjection {
    pub(super) receipts: Vec<ToolAttemptReceipt>,
    pub(super) errors: Vec<String>,
}

#[derive(Debug, Default)]
struct AttemptBuilder {
    call_id: String,
    tool: Option<String>,
    input_fingerprint: Option<String>,
    proposed_sequence: Option<u64>,
    started_sequence: Option<u64>,
    finished: Option<FinishedReceipt>,
    invalid: bool,
}

#[derive(Debug)]
struct FinishedReceipt {
    sequence: u64,
    status: String,
    metadata: agent_core::Metadata,
}

pub(super) fn project_tool_receipts(events: &[Event], root: &Path) -> ToolReceiptProjection {
    let mut projection = ToolReceiptProjection::default();
    let Some(root_event) = events
        .iter()
        .find(|event| is_agent_run_start_event(event))
    else {
        projection
            .errors
            .push("current Agent run identity is missing".to_string());
        return projection;
    };
    let lineage = match AgentRunLineage::from_events(events) {
        Ok(lineage) => lineage,
        Err(error) => {
            projection
                .errors
                .push(format!("current Agent run lineage is invalid: {error}"));
            return projection;
        }
    };
    let Some(root_run_id) = lineage
        .logical_run_id_for_event(root_event)
        .ok()
        .flatten()
    else {
        projection
            .errors
            .push("current Agent run identity is missing".to_string());
        return projection;
    };
    let mut attempts = BTreeMap::<String, AttemptBuilder>::new();
    for event in events.iter().filter(|event| {
        matches!(
            event.kind,
            EventKind::ToolCallProposed | EventKind::ToolCallStarted | EventKind::ToolCallFinished
        )
    }) {
        if lineage
            .logical_run_id_for_event(event)
            .ok()
            .flatten()
            != Some(root_run_id)
        {
            projection.errors.push(format!(
                "tool event {} is outside the current logical Agent run",
                event.sequence
            ));
            continue;
        }
        let Some(call_id) = event.metadata.get("tool_call_id").cloned() else {
            projection.errors.push(format!(
                "tool event {} is missing tool_call_id",
                event.sequence
            ));
            continue;
        };
        let attempt = attempts
            .entry(call_id.clone())
            .or_insert_with(|| AttemptBuilder {
                call_id,
                ..AttemptBuilder::default()
            });
        merge_common_metadata(attempt, event, &mut projection.errors);
        match event.kind {
            EventKind::ToolCallProposed => {
                merge_sequence(
                    &mut attempt.proposed_sequence,
                    event.sequence,
                    "proposed",
                    &attempt.call_id,
                    &mut attempt.invalid,
                    &mut projection.errors,
                );
            }
            EventKind::ToolCallStarted => {
                merge_sequence(
                    &mut attempt.started_sequence,
                    event.sequence,
                    "started",
                    &attempt.call_id,
                    &mut attempt.invalid,
                    &mut projection.errors,
                );
            }
            EventKind::ToolCallFinished => {
                if attempt.finished.is_some() {
                    attempt.invalid = true;
                    projection.errors.push(format!(
                        "tool call {} has multiple finished events",
                        attempt.call_id
                    ));
                } else {
                    attempt.finished = Some(FinishedReceipt {
                        sequence: event.sequence,
                        status: event
                            .metadata
                            .get("status")
                            .cloned()
                            .unwrap_or_else(|| "invalid".to_string()),
                        metadata: event.metadata.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    let canonical_root = root.canonicalize().ok();
    projection.receipts = attempts
        .into_values()
        .map(|attempt| build_receipt(attempt, canonical_root.as_deref(), &mut projection.errors))
        .collect();
    projection
}

fn merge_common_metadata(attempt: &mut AttemptBuilder, event: &Event, errors: &mut Vec<String>) {
    if let Some(tool) = event.metadata.get("tool") {
        if attempt.tool.as_ref().is_some_and(|current| current != tool) {
            attempt.invalid = true;
            errors.push(format!(
                "tool call {} changed tool identity",
                attempt.call_id
            ));
        } else {
            attempt.tool = Some(tool.clone());
        }
    }
    for fingerprint in [
        event.metadata.get("input_fingerprint"),
        event.metadata.get("result_input_fingerprint"),
    ]
    .into_iter()
    .flatten()
    {
        if !is_sha256(fingerprint) {
            attempt.invalid = true;
            errors.push(format!(
                "tool call {} has an invalid input fingerprint",
                attempt.call_id
            ));
        } else if attempt
            .input_fingerprint
            .as_ref()
            .is_some_and(|current| current != fingerprint)
        {
            attempt.invalid = true;
            errors.push(format!(
                "tool call {} changed input fingerprint",
                attempt.call_id
            ));
        } else {
            attempt.input_fingerprint = Some(fingerprint.clone());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_sequence(
    slot: &mut Option<u64>,
    sequence: u64,
    phase: &str,
    call_id: &str,
    invalid: &mut bool,
    errors: &mut Vec<String>,
) {
    if slot.replace(sequence).is_some() {
        *invalid = true;
        errors.push(format!("tool call {call_id} has multiple {phase} events"));
    }
}

fn build_receipt(
    mut attempt: AttemptBuilder,
    canonical_root: Option<&Path>,
    errors: &mut Vec<String>,
) -> ToolAttemptReceipt {
    let tool = attempt.tool.take().unwrap_or_else(|| "unknown".to_string());
    if tool == "unknown" || attempt.input_fingerprint.is_none() {
        attempt.invalid = true;
        errors.push(format!(
            "tool call {} is missing typed identity evidence",
            attempt.call_id
        ));
    }
    let (finished_sequence, target_sha256, evidence_sha256, artifacts, mut status) =
        match attempt.finished.as_ref() {
            Some(finished) => {
                if attempt
                    .proposed_sequence
                    .is_some_and(|sequence| sequence >= finished.sequence)
                    || attempt
                        .started_sequence
                        .is_some_and(|sequence| sequence >= finished.sequence)
                    || attempt
                        .proposed_sequence
                        .zip(attempt.started_sequence)
                        .is_some_and(|(proposed, started)| proposed >= started)
                {
                    attempt.invalid = true;
                    errors.push(format!(
                        "tool call {} has an invalid event sequence",
                        attempt.call_id
                    ));
                }
                let (artifacts, artifacts_valid) =
                    artifact_receipts(&finished.metadata, canonical_root, &attempt.call_id, errors);
                let target_sha256 = tool_target(&tool, &finished.metadata)
                    .map(|target| sha256_hex(target.as_bytes()));
                let evidence_sha256 = tool_evidence_sha256(
                    target_sha256.as_deref(),
                    &artifacts,
                    finished.metadata.get("result_model_observation"),
                );
                let status = if finished
                    .metadata
                    .get("result_superseded_by_steer")
                    .is_some_and(|value| value == "true")
                {
                    ToolReceiptStatus::Superseded
                } else {
                    match finished.status.as_str() {
                        "succeeded" => ToolReceiptStatus::Succeeded,
                        "failed" => ToolReceiptStatus::Failed,
                        "cancelled" => ToolReceiptStatus::Cancelled,
                        "denied" => ToolReceiptStatus::Denied,
                        _ => ToolReceiptStatus::Invalid,
                    }
                };
                if !artifacts_valid {
                    attempt.invalid = true;
                }
                (
                    Some(finished.sequence),
                    target_sha256,
                    evidence_sha256,
                    artifacts,
                    status,
                )
            }
            None => (None, None, None, Vec::new(), ToolReceiptStatus::Incomplete),
        };
    if matches!(
        status,
        ToolReceiptStatus::Succeeded
            | ToolReceiptStatus::Failed
            | ToolReceiptStatus::Cancelled
            | ToolReceiptStatus::Denied
            | ToolReceiptStatus::Superseded
    ) && attempt.proposed_sequence.is_none()
        && attempt.started_sequence.is_none()
    {
        attempt.invalid = true;
        errors.push(format!(
            "tool call {} reached a terminal status without a proposed or started event",
            attempt.call_id
        ));
    }
    if matches!(status, ToolReceiptStatus::Succeeded) && attempt.started_sequence.is_none() {
        attempt.invalid = true;
        errors.push(format!(
            "tool call {} succeeded without a started event",
            attempt.call_id
        ));
    }
    if attempt.invalid {
        status = ToolReceiptStatus::Invalid;
    }
    ToolAttemptReceipt {
        call_sha256: domain_hash(TOOL_CALL_DOMAIN, attempt.call_id.as_bytes()),
        tool,
        status,
        input_fingerprint: attempt.input_fingerprint,
        started_sequence: attempt.started_sequence.or(attempt.proposed_sequence),
        finished_sequence,
        target_sha256,
        evidence_sha256,
        artifacts,
    }
}

fn tool_target<'a>(tool: &str, metadata: &'a agent_core::Metadata) -> Option<&'a str> {
    if tool.starts_with("browser.") {
        metadata.get("result_url").map(String::as_str)
    } else if tool.starts_with("file.") {
        metadata
            .get("result_path")
            .or_else(|| metadata.get("result_source_path"))
            .map(String::as_str)
    } else if tool == "shell.run" {
        metadata.get("result_cwd").map(String::as_str)
    } else {
        None
    }
}

fn artifact_receipts(
    metadata: &agent_core::Metadata,
    canonical_root: Option<&Path>,
    call_id: &str,
    errors: &mut Vec<String>,
) -> (Vec<ToolArtifactReceipt>, bool) {
    let mut candidates = Vec::<(String, Option<String>)>::new();
    if let Some(encoded) = metadata.get("result_artifacts_json") {
        match serde_json::from_str::<serde_json::Value>(encoded)
            .ok()
            .and_then(|value| value.as_array().cloned())
        {
            Some(values) => {
                for value in values {
                    if let Some(path) = value.get("path").and_then(serde_json::Value::as_str) {
                        candidates.push((
                            path.to_string(),
                            value
                                .get("mime_type")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                        ));
                    }
                }
            }
            None => {
                errors.push(format!("tool call {call_id} has invalid artifact metadata"));
                return (Vec::new(), false);
            }
        }
    }
    for key in [
        "result_artifact_path",
        "result_text_path",
        "result_structured_output_path",
    ] {
        if let Some(path) = metadata.get(key) {
            candidates.push((path.clone(), None));
        }
    }
    let had_candidates = !candidates.is_empty();
    let Some(canonical_root) = canonical_root else {
        if had_candidates {
            errors.push(format!(
                "tool call {call_id} artifacts cannot be bound to the workspace"
            ));
        }
        return (Vec::new(), !had_candidates);
    };
    let mut seen = BTreeSet::new();
    let mut receipts = Vec::new();
    let mut valid = true;
    for (path, mime_type) in candidates {
        let candidate = PathBuf::from(&path);
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            canonical_root.join(candidate)
        };
        let Ok(canonical) = candidate.canonicalize() else {
            errors.push(format!("tool call {call_id} artifact is missing"));
            valid = false;
            continue;
        };
        if !canonical.is_file() || !canonical.starts_with(canonical_root) {
            errors.push(format!(
                "tool call {call_id} artifact escaped the evaluation workspace"
            ));
            valid = false;
            continue;
        }
        let Ok(relative) = canonical.strip_prefix(canonical_root) else {
            valid = false;
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if !seen.insert(relative.clone()) {
            continue;
        }
        match fs::read(&canonical) {
            Ok(bytes) => receipts.push(ToolArtifactReceipt {
                path_sha256: domain_hash(TOOL_ARTIFACT_PATH_DOMAIN, relative.as_bytes()),
                content_sha256: sha256_hex(&bytes),
                bytes: bytes.len().min(u64::MAX as usize) as u64,
                mime_type,
            }),
            Err(_) => {
                errors.push(format!("tool call {call_id} artifact could not be read"));
                valid = false;
            }
        }
    }
    (receipts, valid)
}

fn tool_evidence_sha256(
    target_sha256: Option<&str>,
    artifacts: &[ToolArtifactReceipt],
    observation: Option<&String>,
) -> Option<String> {
    if target_sha256.is_none() && artifacts.is_empty() && observation.is_none() {
        return None;
    }
    let encoded = serde_json::json!({
        "target_sha256": target_sha256,
        "artifacts": artifacts,
        "model_observation_sha256": observation.map(|value| sha256_hex(value.as_bytes())),
    })
    .to_string();
    Some(domain_hash(TOOL_EVIDENCE_DOMAIN, encoded.as_bytes()))
}

fn domain_hash(domain: &[u8], value: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(domain.len() + value.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(value);
    sha256_hex(&bytes)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind, Metadata, TaskId};

    fn event(sequence: u64, kind: EventKind, metadata: Metadata) -> Event {
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: TaskId("task".to_string()),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: if sequence == 1 {
                "Agent task started".to_string()
            } else {
                "tool event".to_string()
            },
            metadata,
        }
    }

    fn metadata(call: &str, tool: &str, run: &str) -> Metadata {
        [
            ("agent_run_id".to_string(), run.to_string()),
            ("tool_call_id".to_string(), call.to_string()),
            ("tool".to_string(), tool.to_string()),
            ("input_fingerprint".to_string(), "a".repeat(64)),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn only_finished_success_is_a_success_receipt() {
        let root = tempfile::tempdir().expect("workspace");
        let mut start = Metadata::new();
        start.insert("agent_run_id".to_string(), "run-current".to_string());
        let mut finished = metadata("success", "browser.open", "run-current");
        finished.insert("status".to_string(), "succeeded".to_string());
        finished.insert("result_input_fingerprint".to_string(), "a".repeat(64));
        finished.insert(
            "result_url".to_string(),
            "http://127.0.0.1:4100/fixture".to_string(),
        );
        let mut failed = metadata("failed", "file.write", "run-current");
        failed.insert("status".to_string(), "failed".to_string());
        let events = vec![
            event(1, EventKind::TaskStatusChanged, start),
            event(
                2,
                EventKind::ToolCallStarted,
                metadata("success", "browser.open", "run-current"),
            ),
            event(3, EventKind::ToolCallFinished, finished),
            event(
                4,
                EventKind::ToolCallStarted,
                metadata("incomplete", "file.read", "run-current"),
            ),
            event(
                5,
                EventKind::ToolCallStarted,
                metadata("failed", "file.write", "run-current"),
            ),
            event(6, EventKind::ToolCallFinished, failed),
        ];

        let projection = project_tool_receipts(&events, root.path());

        assert_eq!(projection.receipts.len(), 3);
        assert_eq!(projection.receipts[0].status, ToolReceiptStatus::Failed);
        assert_eq!(projection.receipts[1].status, ToolReceiptStatus::Incomplete);
        assert_eq!(projection.receipts[2].status, ToolReceiptStatus::Succeeded);
        assert!(projection.receipts[2].target_sha256.is_some());
    }

    #[test]
    fn mismatched_fingerprint_fails_closed() {
        let root = tempfile::tempdir().expect("workspace");
        let mut start = Metadata::new();
        start.insert("agent_run_id".to_string(), "run-current".to_string());
        let mut finished = metadata("call", "file.read", "run-current");
        finished.insert("status".to_string(), "succeeded".to_string());
        finished.insert("result_input_fingerprint".to_string(), "b".repeat(64));
        let events = vec![
            event(1, EventKind::TaskStatusChanged, start),
            event(
                2,
                EventKind::ToolCallStarted,
                metadata("call", "file.read", "run-current"),
            ),
            event(3, EventKind::ToolCallFinished, finished),
        ];

        let projection = project_tool_receipts(&events, root.path());

        assert_eq!(projection.receipts[0].status, ToolReceiptStatus::Invalid);
        assert!(!projection.errors.is_empty());
    }

    #[test]
    fn other_run_tool_events_cannot_satisfy_the_current_run() {
        let root = tempfile::tempdir().expect("workspace");
        let mut start = Metadata::new();
        start.insert("agent_run_id".to_string(), "run-current".to_string());
        let mut current_finished = metadata("current", "file.read", "run-current");
        current_finished.insert("status".to_string(), "succeeded".to_string());
        let mut other_finished = metadata("other", "file.write", "run-other");
        other_finished.insert("status".to_string(), "succeeded".to_string());
        let events = vec![
            event(1, EventKind::TaskStatusChanged, start),
            event(
                2,
                EventKind::ToolCallStarted,
                metadata("current", "file.read", "run-current"),
            ),
            event(3, EventKind::ToolCallFinished, current_finished),
            event(
                4,
                EventKind::ToolCallStarted,
                metadata("other", "file.write", "run-other"),
            ),
            event(5, EventKind::ToolCallFinished, other_finished),
        ];

        let projection = project_tool_receipts(&events, root.path());

        assert_eq!(projection.receipts.len(), 1);
        assert_eq!(projection.receipts[0].tool, "file.read");
        assert_eq!(projection.receipts[0].status, ToolReceiptStatus::Succeeded);
        assert_eq!(projection.errors.len(), 2);
    }

    #[test]
    fn legacy_multi_attempt_continuations_share_one_logical_receipt_scope() {
        let root = tempfile::tempdir().expect("workspace");
        let mut start = Metadata::new();
        start.insert("agent_run_id".to_string(), "attempt-a".to_string());

        let mut started_b = metadata("call-b", "file.read", "attempt-b");
        started_b.insert(
            "source_agent_run_id".to_string(),
            "attempt-a".to_string(),
        );
        let mut finished_b = started_b.clone();
        finished_b.insert("status".to_string(), "failed".to_string());

        let mut started_c = metadata("call-c", "file.read", "attempt-c");
        started_c.insert(
            "source_agent_run_id".to_string(),
            "attempt-b".to_string(),
        );
        let mut finished_c = started_c.clone();
        finished_c.insert("status".to_string(), "succeeded".to_string());

        let events = vec![
            event(1, EventKind::TaskStatusChanged, start),
            event(2, EventKind::ToolCallStarted, started_b),
            event(3, EventKind::ToolCallFinished, finished_b),
            event(4, EventKind::ToolCallStarted, started_c),
            event(5, EventKind::ToolCallFinished, finished_c),
        ];

        let projection = project_tool_receipts(&events, root.path());

        assert!(projection.errors.is_empty(), "{:?}", projection.errors);
        assert_eq!(projection.receipts.len(), 2);
        assert_eq!(projection.receipts[0].status, ToolReceiptStatus::Failed);
        assert_eq!(projection.receipts[1].status, ToolReceiptStatus::Succeeded);
    }

    #[test]
    fn succeeded_receipts_require_a_started_event_before_the_finish() {
        let root = tempfile::tempdir().expect("workspace");
        let mut start = Metadata::new();
        start.insert("agent_run_id".to_string(), "run-current".to_string());
        let mut finished = metadata("call", "file.read", "run-current");
        finished.insert("status".to_string(), "succeeded".to_string());
        let events = vec![
            event(1, EventKind::TaskStatusChanged, start),
            event(3, EventKind::ToolCallFinished, finished.clone()),
            event(
                4,
                EventKind::ToolCallStarted,
                metadata("call", "file.read", "run-current"),
            ),
        ];

        let projection = project_tool_receipts(&events, root.path());
        assert_eq!(projection.receipts[0].status, ToolReceiptStatus::Invalid);

        let missing_start = project_tool_receipts(
            &[
                event(1, EventKind::TaskStatusChanged, {
                    let mut metadata = Metadata::new();
                    metadata.insert("agent_run_id".to_string(), "run-current".to_string());
                    metadata
                }),
                event(2, EventKind::ToolCallFinished, finished),
            ],
            root.path(),
        );
        assert_eq!(missing_start.receipts[0].status, ToolReceiptStatus::Invalid);
    }
}
