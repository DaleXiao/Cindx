use crate::memory_text::{first_metadata_value, normalize_memory_text, sanitize_line, truncate};
use crate::{MemoryKind, MemoryProvenance, MemoryRecord, MemoryTrust};
use agent_core::{Event, EventKind};

pub fn extract_durable_memories(
    events: &[Event],
    project_id: &str,
    session_id: &str,
) -> Vec<MemoryRecord> {
    let completed = events
        .iter()
        .any(|event| event.summary == "Agent task completed");
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
            .filter(|event| durable_tool_memory(event).is_some())
            .map(|event| event.id.0.clone())
            .take(8)
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
        if !is_durable_outcome_content(&outcome, !evidence_ids.is_empty()) {
            return records;
        }
        records.push(memory_record(
            MemoryKind::Outcome,
            MemoryTrust::AssistantReported,
            content,
            if evidence_ids.is_empty() { 58 } else { 68 },
            event,
            project_id,
            session_id,
            if evidence_ids.is_empty() {
                vec![event.id.0.clone()]
            } else {
                evidence_ids
            },
        ));
    }

    records
}

#[allow(clippy::too_many_arguments)]
fn memory_record(
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
    if !matches!(event.kind, EventKind::ToolCallFinished)
        || event.metadata.get("status").map(String::as_str) != Some("succeeded")
    {
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

fn is_durable_outcome_content(content: &str, has_tool_evidence: bool) -> bool {
    if contains_instruction_override(content) {
        return false;
    }
    if has_tool_evidence {
        return true;
    }
    let normalized = normalize_memory_text(content);
    if normalized.chars().count() < 16 {
        return false;
    }
    let lower = content.to_lowercase();
    [
        "implemented",
        "fixed",
        "updated",
        "created",
        "completed",
        "configured",
        "stored",
        "added",
        "removed",
        "generated",
        "tests pass",
        "test passed",
        "已实现",
        "已修复",
        "已完成",
        "已更新",
        "已创建",
        "已配置",
        "已保存",
        "已新增",
        "已删除",
        "已生成",
        "测试通过",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn contains_instruction_override(content: &str) -> bool {
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
