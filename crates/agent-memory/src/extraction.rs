use crate::learning_evidence::{
    is_completed_agent_event, trusted_outcome_evidence, TrustedOutcomeEvidence,
};
use crate::memory_text::{first_metadata_value, normalize_memory_text, sanitize_line, truncate};
use crate::semantic::{parse_semantic_memory_batch, validate_semantic_memory_batch};
use crate::{MemoryKind, MemoryProvenance, MemoryRecord, MemoryTrust};
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
        {
            if let Some(content) = event
                .metadata
                .get("content")
                .map(|content| truncate(&sanitize_line(content), 1_200))
                .filter(|content| is_durable_requirement_content(content))
            {
                records.push(memory_record(
                    MemoryKind::Requirement,
                    MemoryTrust::UserStated,
                    content,
                    100,
                    event,
                    project_id,
                    session_id,
                    vec![event.id.0.clone()],
                ));
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

fn is_durable_requirement_content(content: &str) -> bool {
    let normalized = normalize_memory_text(content);
    if normalized.chars().count() < 6 {
        return false;
    }
    if matches!(
        normalized.as_str(),
        "hello" | "hi" | "hey" | "你好" | "您好" | "在吗" | "谢谢" | "thanks"
    ) {
        return false;
    }
    let lower = content.to_lowercase();
    let is_question = content.trim_end().ends_with(['?', '？']);
    let has_explicit_memory_directive = [
        "remember that",
        "remember to",
        "from now on call me",
        "记住",
        "以后叫我",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if is_question && !has_explicit_memory_directive {
        return false;
    }
    [
        "remember",
        "always",
        "never",
        "must",
        "should",
        "prefer",
        "keep ",
        "do not",
        "don't",
        "from now on",
        "call me",
        "my name",
        "requirement",
        "constraint",
        "记住",
        "以后",
        "始终",
        "一直",
        "必须",
        "不要",
        "不能",
        "不允许",
        "偏好",
        "称呼",
        "叫我",
        "我的名字",
        "务必",
        "保持",
        "要求",
        "需要",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
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

pub(crate) fn contains_instruction_override(content: &str) -> bool {
    let lower = content.to_lowercase();
    [
        "ignore previous instruction",
        "ignore all previous",
        "ignore the system message",
        "reveal the system prompt",
        "developer message says",
        "忽略之前的指令",
        "忽略所有之前",
        "忽略系统消息",
        "泄露系统提示",
        "显示系统提示词",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
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
