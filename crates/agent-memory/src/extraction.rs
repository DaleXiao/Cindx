use crate::learning_evidence::{
    is_completed_agent_event, trusted_outcome_evidence, TrustedOutcomeEvidence,
};
use crate::memory_text::{
    first_metadata_value, normalize_memory_text, requirement_evidence_sha256, sanitize_line,
    sha256_hex, truncate,
};
use crate::requirement_scope::{
    contains_instruction_override, durable_user_requirement_spans, UserRequirementSpan,
};
use crate::semantic::{parse_semantic_memory_batch, validate_semantic_memory_batch};
use crate::{
    MemoryClaimOrigin, MemoryKind, MemoryProvenance, MemoryRecord, MemoryRequirementScope,
    MemoryTrust, UserRequirementEvidence, USER_REQUIREMENT_EVIDENCE_SCHEMA,
};
use agent_core::{Event, EventKind, EVENT_TYPE_METADATA_KEY};

pub fn extract_durable_memories(
    events: &[Event],
    project_id: &str,
    session_id: &str,
) -> Vec<MemoryRecord> {
    if let Some(records) = semantic_memory_records(events, project_id, session_id) {
        return records;
    }
    let completed = events.iter().any(is_completed_agent_event);
    let mut records = Vec::new();

    for event in events {
        if matches!(event.kind, EventKind::MessageAdded)
            && event.metadata.get("role").map(String::as_str) == Some("user")
            && event.metadata.get("internal").map(String::as_str) != Some("true")
            && event.metadata.get("project_id").map(String::as_str) == Some(project_id)
            && event.metadata.get("session_id").map(String::as_str) == Some(session_id)
        {
            if let Some(source) = event.metadata.get("content") {
                let source_sha256 = sha256_hex(source.as_bytes());
                for span in durable_user_requirement_spans(source) {
                    records.push(user_requirement_memory_record_with_source_sha256(
                        event,
                        project_id,
                        session_id,
                        source,
                        &source_sha256,
                        span,
                        100,
                    ));
                }
            }
        }
    }

    if !completed {
        return records;
    }

    for event in events {
        if let Some(content) = durable_tool_memory(event) {
            records.push(memory_record(
                MemoryKind::Evidence,
                MemoryTrust::ToolVerified,
                content,
                88,
                event,
                project_id,
                session_id,
                vec![event.id.0.clone()],
            ));
        }
    }

    if let Some((outcome_index, event)) = events.iter().enumerate().rev().find(|(_, event)| {
        matches!(event.kind, EventKind::MessageAdded)
            && event.metadata.get("role").map(String::as_str) == Some("assistant")
            && event.metadata.get("internal").map(String::as_str) != Some("true")
            && event
                .metadata
                .get("content")
                .is_some_and(|content| !content.trim().is_empty())
    }) {
        let Some((terminal, terminal_evidence)) = events[outcome_index + 1..]
            .iter()
            .take_while(|event| {
                !(matches!(event.kind, EventKind::MessageAdded)
                    && event.metadata.get("role").map(String::as_str) == Some("user")
                    && event.metadata.get("internal").map(String::as_str) != Some("true"))
            })
            .find_map(|event| trusted_outcome_evidence(event).map(|evidence| (event, evidence)))
        else {
            return records;
        };
        let turn_start = events[..outcome_index]
            .iter()
            .rposition(|event| {
                matches!(event.kind, EventKind::MessageAdded)
                    && event.metadata.get("role").map(String::as_str) == Some("user")
                    && event.metadata.get("internal").map(String::as_str) != Some("true")
            })
            .unwrap_or_default();
        let evidence_ids = events[turn_start..outcome_index]
            .iter()
            .filter(|event| is_successful_tool_event(event))
            .map(|event| event.id.0.clone())
            .take(6)
            .collect::<Vec<_>>();
        let outcome = truncate(
            &sanitize_line(
                event
                    .metadata
                    .get("content")
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
            900,
        );
        let content = format!("Assistant outcome: {outcome}");
        if !is_durable_outcome_content(&outcome, terminal_evidence, !evidence_ids.is_empty()) {
            return records;
        }
        let mut source_event_ids = evidence_ids;
        source_event_ids.push(event.id.0.clone());
        source_event_ids.push(terminal.id.0.clone());
        records.push(memory_record(
            MemoryKind::Outcome,
            MemoryTrust::AssistantReported,
            content,
            68,
            terminal,
            project_id,
            session_id,
            source_event_ids,
        ));
    }

    records
}

fn semantic_memory_records(
    events: &[Event],
    project_id: &str,
    session_id: &str,
) -> Option<Vec<MemoryRecord>> {
    let event = events.iter().rev().find(|event| {
        event.summary == "Semantic memory candidates accepted"
            && !event.metadata.contains_key(EVENT_TYPE_METADATA_KEY)
    })?;
    let payload = event.metadata.get("memory_candidates_json")?;
    let batch = parse_semantic_memory_batch(payload).ok()?;
    Some(validate_semantic_memory_batch(batch, events, project_id, session_id).accepted)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn memory_record(
    kind: MemoryKind,
    trust: MemoryTrust,
    content: String,
    importance: u8,
    event: &Event,
    project_id: &str,
    session_id: &str,
    source_event_ids: Vec<String>,
) -> MemoryRecord {
    let fingerprint = memory_fingerprint(kind, &content);
    MemoryRecord {
        id: format!("memory-{fingerprint}"),
        fingerprint,
        kind,
        trust,
        content,
        importance,
        provenance: MemoryProvenance {
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            event_id: event.id.0.clone(),
            agent_run_id: event.metadata.get("agent_run_id").cloned(),
            sequence: event.sequence,
            timestamp_ms: event.timestamp_ms,
        },
        source_event_ids,
        source_session_ids: vec![session_id.to_string()],
        user_requirement_evidence: Vec::new(),
        created_at_ms: event.timestamp_ms,
        updated_at_ms: event.timestamp_ms,
        recall_count: 0,
        last_recalled_at_ms: None,
        observed_use_count: 0,
        last_observed_use_at_ms: None,
        superseded_by: None,
        superseded_at_ms: None,
    }
}

pub(crate) fn user_requirement_memory_record(
    event: &Event,
    project_id: &str,
    session_id: &str,
    source: &str,
    span: UserRequirementSpan,
    importance: u8,
) -> MemoryRecord {
    let source_sha256 = sha256_hex(source.as_bytes());
    user_requirement_memory_record_with_source_sha256(
        event,
        project_id,
        session_id,
        source,
        &source_sha256,
        span,
        importance,
    )
}

#[allow(clippy::too_many_arguments)]
fn user_requirement_memory_record_with_source_sha256(
    event: &Event,
    project_id: &str,
    session_id: &str,
    source: &str,
    source_sha256: &str,
    span: UserRequirementSpan,
    importance: u8,
) -> MemoryRecord {
    let content = source
        .get(span.start_byte..span.end_byte)
        .expect("validated user requirement span must be a UTF-8 boundary")
        .to_string();
    let quote_start_byte = span.start_byte as u64;
    let quote_end_byte = span.end_byte as u64;
    let evidence_sha256 = requirement_evidence_sha256(
        USER_REQUIREMENT_EVIDENCE_SCHEMA,
        project_id,
        session_id,
        &event.id.0,
        source_sha256,
        quote_start_byte,
        quote_end_byte,
        &content,
    );
    let mut record = memory_record(
        MemoryKind::Requirement,
        MemoryTrust::UserStated,
        content,
        importance,
        event,
        project_id,
        session_id,
        vec![event.id.0.clone()],
    );
    record
        .user_requirement_evidence
        .push(UserRequirementEvidence {
            schema: USER_REQUIREMENT_EVIDENCE_SCHEMA.to_string(),
            origin: MemoryClaimOrigin::UserVerbatim,
            scope: MemoryRequirementScope::ProjectDurable,
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            event_id: event.id.0.clone(),
            source_sha256: source_sha256.to_string(),
            quote_start_byte,
            quote_end_byte,
            evidence_sha256,
        });
    debug_assert!(record.verifies_user_requirement_source(event));
    record
}

fn durable_tool_memory(event: &Event) -> Option<String> {
    if !is_successful_tool_event(event) {
        return None;
    }
    let tool = event.metadata.get("tool")?.as_str();
    match tool {
        "file.write" => {
            let path =
                first_metadata_value(event, &["result_path", "path"]).unwrap_or("<unknown path>");
            Some(format!("file.write succeeded: {path}"))
        }
        "image.generate" => first_metadata_value(
            event,
            &["result_artifact_path", "artifact_path", "result_path"],
        )
        .map(|path| format!("image.generate succeeded: {path}")),
        _ => None,
    }
}

fn is_successful_tool_event(event: &Event) -> bool {
    matches!(event.kind, EventKind::ToolCallFinished)
        && event.metadata.get("status").map(String::as_str) == Some("succeeded")
}

fn is_durable_outcome_content(
    content: &str,
    terminal_evidence: TrustedOutcomeEvidence,
    has_tool_evidence: bool,
) -> bool {
    // Assistant prose and a successful tool call are not durable project truth
    // by themselves. A tool-attributed postcondition must also match a tool
    // success from the same user turn; independent quality evidence can stand
    // on its own. User-stated requirements are handled separately above.
    if contains_instruction_override(content)
        || (terminal_evidence == TrustedOutcomeEvidence::VerifiedPostcondition
            && !has_tool_evidence)
    {
        return false;
    }
    !normalize_memory_text(content).is_empty()
}

fn memory_fingerprint(kind: MemoryKind, content: &str) -> String {
    let value = format!("{}:{}", kind.label(), normalize_memory_text(content));
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, Metadata, TaskId};

    #[test]
    fn trusted_extraction_requires_matching_project_and_session_metadata() {
        let event = |project_id: Option<&str>, session_id: Option<&str>| {
            let mut metadata = [
                ("role".to_string(), "user".to_string()),
                (
                    "content".to_string(),
                    "Always preserve explicit deletion confirmation".to_string(),
                ),
            ]
            .into_iter()
            .collect::<Metadata>();
            if let Some(project_id) = project_id {
                metadata.insert("project_id".to_string(), project_id.to_string());
            }
            if let Some(session_id) = session_id {
                metadata.insert("session_id".to_string(), session_id.to_string());
            }
            Event {
                id: EventId("event-scope".to_string()),
                task_id: TaskId("task-scope".to_string()),
                sequence: 1,
                timestamp_ms: 1,
                kind: EventKind::MessageAdded,
                summary: "user message".to_string(),
                metadata,
            }
        };

        for source in [
            event(None, None),
            event(Some("project-a"), None),
            event(Some("project-b"), Some("session-a")),
            event(Some("project-a"), Some("session-b")),
        ] {
            assert!(extract_durable_memories(&[source], "project-a", "session-a").is_empty());
        }

        let source = event(Some("project-a"), Some("session-a"));
        let records =
            extract_durable_memories(std::slice::from_ref(&source), "project-a", "session-a");
        assert_eq!(records.len(), 1);
        assert!(records[0].verifies_user_requirement_source(&source));
    }

    #[test]
    fn multi_span_extraction_reuses_one_source_digest_without_changing_evidence() {
        let content = concat!(
            "Long-term requirements:\n",
            "1. The sidebar must remain stable.\n",
            "2. The composer must remain responsive.\n",
            "3. The session list must preserve unread state.\n",
            "4. The model picker must remain accessible.\n",
            "5. The knowledge graph must remain searchable.\n",
            "6. Release builds must remain reproducible."
        );
        let source = Event {
            id: EventId("event-multi-span".to_string()),
            task_id: TaskId("task-multi-span".to_string()),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), content.to_string()),
                ("project_id".to_string(), "project-a".to_string()),
                ("session_id".to_string(), "session-a".to_string()),
            ]
            .into_iter()
            .collect(),
        };

        let records =
            extract_durable_memories(std::slice::from_ref(&source), "project-a", "session-a");
        assert_eq!(records.len(), 6);
        assert!(records
            .iter()
            .all(|record| record.verifies_user_requirement_source(&source)));
        assert_eq!(
            records
                .iter()
                .flat_map(|record| &record.user_requirement_evidence)
                .map(|evidence| evidence.source_sha256.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1
        );
    }
}
