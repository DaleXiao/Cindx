use agent_core::{Event, EventKind, Message, MessageRole};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MEMORY_LEDGER_SCHEMA: &str = "cindx.memory-ledger.v3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Requirement,
    Outcome,
    Evidence,
}

impl MemoryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Requirement => "requirement",
            Self::Outcome => "outcome",
            Self::Evidence => "evidence",
        }
    }

    fn recall_weight(self) -> f64 {
        match self {
            Self::Requirement => 1.0,
            Self::Evidence => 0.92,
            Self::Outcome => 0.72,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTrust {
    UserStated,
    ToolVerified,
    AssistantReported,
}

impl MemoryTrust {
    pub fn label(self) -> &'static str {
        match self {
            Self::UserStated => "user_stated",
            Self::ToolVerified => "tool_verified",
            Self::AssistantReported => "assistant_reported",
        }
    }

    fn recall_weight(self) -> f64 {
        match self {
            Self::UserStated => 1.0,
            Self::ToolVerified => 0.96,
            Self::AssistantReported => 0.68,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProvenance {
    pub project_id: String,
    pub session_id: String,
    pub event_id: String,
    pub agent_run_id: Option<String>,
    pub sequence: u64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub fingerprint: String,
    pub kind: MemoryKind,
    pub trust: MemoryTrust,
    pub content: String,
    pub importance: u8,
    pub provenance: MemoryProvenance,
    pub source_event_ids: Vec<String>,
    pub source_session_ids: Vec<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub recall_count: u64,
    pub last_recalled_at_ms: Option<u64>,
    #[serde(default)]
    pub observed_use_count: u64,
    #[serde(default)]
    pub last_observed_use_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLedger {
    pub schema: String,
    pub project_id: String,
    pub revision: u64,
    pub event_count: u64,
    pub records: Vec<MemoryRecord>,
}

impl MemoryLedger {
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            schema: MEMORY_LEDGER_SCHEMA.to_string(),
            project_id: project_id.into(),
            revision: 0,
            event_count: 0,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecall {
    pub record: MemoryRecord,
    pub score: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryMergeStats {
    pub inserted: usize,
    pub updated: usize,
    pub evicted: usize,
}

pub fn extract_durable_memories(
    events: &[Event],
    project_id: &str,
    session_id: &str,
) -> Vec<MemoryRecord> {
    let completed = events
        .iter()
        .any(|event| event.summary == "Agent task completed");
    if !completed {
        return Vec::new();
    }

    let successful_tools = events
        .iter()
        .filter(|event| {
            matches!(event.kind, EventKind::ToolCallFinished)
                && event.metadata.get("status").map(String::as_str) == Some("succeeded")
        })
        .collect::<Vec<_>>();
    let evidence_ids = successful_tools
        .iter()
        .map(|event| event.id.0.clone())
        .take(8)
        .collect::<Vec<_>>();
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

    if let Some(event) = events.iter().rev().find(|event| {
        matches!(event.kind, EventKind::MessageAdded)
            && event.metadata.get("role").map(String::as_str) == Some("assistant")
            && event.metadata.get("internal").map(String::as_str) != Some("true")
            && event
                .metadata
                .get("content")
                .is_some_and(|content| !content.trim().is_empty())
    }) {
        let outcome = truncate(
            &sanitize_line(
                event
                    .metadata
                    .get("content")
                    .map(String::as_str)
                    .unwrap_or("")
            ),
            900,
        );
        let content = format!("Assistant outcome: {outcome}");
        if !is_durable_outcome_content(&outcome, !evidence_ids.is_empty()) {
            return records;
        }
        records.push(memory_record(
            MemoryKind::Outcome,
            if evidence_ids.is_empty() {
                MemoryTrust::AssistantReported
            } else {
                MemoryTrust::ToolVerified
            },
            content,
            if evidence_ids.is_empty() { 58 } else { 76 },
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

pub fn merge_memory_records(
    ledger: &mut MemoryLedger,
    candidates: impl IntoIterator<Item = MemoryRecord>,
    max_records: usize,
) -> MemoryMergeStats {
    let mut stats = MemoryMergeStats::default();
    for candidate in candidates {
        if let Some(existing) = ledger
            .records
            .iter_mut()
            .find(|record| record.fingerprint == candidate.fingerprint)
        {
            let previous_source_count = existing.source_event_ids.len();
            for event_id in &candidate.source_event_ids {
                if !existing.source_event_ids.contains(event_id) {
                    existing.source_event_ids.push(event_id.clone());
                }
            }
            for session_id in &candidate.source_session_ids {
                if !existing.source_session_ids.contains(session_id) {
                    existing.source_session_ids.push(session_id.clone());
                }
            }
            existing.source_event_ids.truncate(8);
            existing.source_session_ids.truncate(8);
            existing.updated_at_ms = existing.updated_at_ms.max(candidate.updated_at_ms);
            existing.importance = existing.importance.max(candidate.importance);
            if candidate.provenance.timestamp_ms >= existing.provenance.timestamp_ms {
                existing.provenance = candidate.provenance;
                existing.trust = candidate.trust;
            }
            if existing.source_event_ids.len() != previous_source_count {
                stats.updated += 1;
            }
            continue;
        }
        ledger.records.push(candidate);
        stats.inserted += 1;
    }

    ledger.records.sort_by(|left, right| {
        right
            .importance
            .cmp(&left.importance)
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| right.recall_count.cmp(&left.recall_count))
            .then_with(|| left.id.cmp(&right.id))
    });
    let limit = max_records.max(1);
    if ledger.records.len() > limit {
        stats.evicted = ledger.records.len() - limit;
        ledger.records.truncate(limit);
    }
    stats
}

pub fn recall_memories_at(
    ledger: &MemoryLedger,
    query: &str,
    current_session_id: Option<&str>,
    limit: usize,
    now_ms: u64,
) -> Vec<MemoryRecall> {
    let query_terms = memory_terms(query);
    if query_terms.is_empty() {
        return Vec::new();
    }
    let normalized_query = normalize_memory_text(query);
    let mut recalls = ledger
        .records
        .iter()
        .filter_map(|record| {
            let terms = memory_terms(&record.content);
            let overlap = query_terms.intersection(&terms).count();
            let overlap_score = overlap as f64 / query_terms.len().max(1) as f64;
            let exact = !normalized_query.is_empty()
                && normalize_memory_text(&record.content).contains(&normalized_query);
            let identifier_match = query_terms.intersection(&terms).any(|term| {
                term.contains('_') || term.contains('/') || term.contains('.')
            });
            if (overlap == 0 && !exact)
                || (!exact && overlap < 2 && query_terms.len() > 3 && !identifier_match)
            {
                return None;
            }
            let mut reasons = Vec::new();
            if exact {
                reasons.push("exact_phrase".to_string());
            }
            if overlap > 0 {
                reasons.push(format!("term_overlap:{overlap}"));
            }
            let cross_session = current_session_id.is_some_and(|session_id| {
                !record
                    .source_session_ids
                    .iter()
                    .any(|source| source == session_id)
            });
            if cross_session {
                reasons.push("cross_session".to_string());
            }
            reasons.push(format!("trust:{}", record.trust.label()));
            let age_days = now_ms
                .saturating_sub(record.updated_at_ms)
                .checked_div(86_400_000)
                .unwrap_or_default()
                .min(365) as f64;
            let recency = 1.0 / (1.0 + age_days / 30.0);
            let score = (overlap_score * 0.68 + if exact { 0.32 } else { 0.0 })
                * record.kind.recall_weight()
                * record.trust.recall_weight()
                * (0.8 + recency * 0.2)
                * (0.85 + f64::from(record.importance) / 100.0 * 0.15)
                * if cross_session { 1.08 } else { 0.92 };
            (score >= 0.1).then(|| MemoryRecall {
                record: record.clone(),
                score,
                reasons,
            })
        })
        .collect::<Vec<_>>();
    recalls.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.record.importance.cmp(&left.record.importance))
            .then_with(|| left.record.id.cmp(&right.record.id))
    });
    let mut diversified = Vec::new();
    let mut per_session = std::collections::BTreeMap::<String, usize>::new();
    let mut per_kind = std::collections::BTreeMap::<String, usize>::new();
    for recall in recalls {
        let session = recall.record.provenance.session_id.clone();
        let kind = recall.record.kind.label().to_string();
        let kind_limit = match recall.record.kind {
            MemoryKind::Requirement => 3,
            MemoryKind::Evidence | MemoryKind::Outcome => 2,
        };
        if per_session.get(&session).copied().unwrap_or_default() >= 2
            || per_kind.get(&kind).copied().unwrap_or_default() >= kind_limit
        {
            continue;
        }
        *per_session.entry(session).or_default() += 1;
        *per_kind.entry(kind).or_default() += 1;
        diversified.push(recall);
        if diversified.len() >= limit.max(1) {
            break;
        }
    }
    diversified
}

pub fn record_memory_recalls(
    ledger: &mut MemoryLedger,
    recalls: &[MemoryRecall],
    recalled_at_ms: u64,
) {
    let recalled = recalls
        .iter()
        .map(|recall| recall.record.id.as_str())
        .collect::<BTreeSet<_>>();
    for record in &mut ledger.records {
        if recalled.contains(record.id.as_str()) {
            record.recall_count = record.recall_count.saturating_add(1);
            record.last_recalled_at_ms = Some(recalled_at_ms);
        }
    }
}

pub fn record_memory_observed_uses(
    ledger: &mut MemoryLedger,
    recalled_ids: &[String],
    output: &str,
    observed_at_ms: u64,
) -> Vec<String> {
    let recalled = recalled_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let output_terms = memory_terms(output);
    let normalized_output = normalize_memory_text(output);
    let mut used = Vec::new();
    for record in &mut ledger.records {
        if !recalled.contains(record.id.as_str()) {
            continue;
        }
        let record_terms = memory_terms(&record.content);
        let overlap = record_terms.intersection(&output_terms).count();
        let coverage = overlap as f64 / record_terms.len().max(1) as f64;
        let normalized_record = normalize_memory_text(&record.content);
        let exact = normalized_record.chars().count() <= 160
            && !normalized_record.is_empty()
            && normalized_output.contains(&normalized_record);
        if exact || (overlap >= 2 && coverage >= 0.2) || overlap >= 4 {
            record.observed_use_count = record.observed_use_count.saturating_add(1);
            record.last_observed_use_at_ms = Some(observed_at_ms);
            used.push(record.id.clone());
        }
    }
    used
}

pub fn memory_recalls_to_markdown(recalls: &[MemoryRecall]) -> String {
    if recalls.is_empty() {
        return String::new();
    }
    let mut output = String::from(
        "## Project Memory\nHistorical memory is project-scoped. User-stated entries preserve prior requirements; tool-verified entries are evidence; assistant-reported entries are unverified summaries. Apply relevant recalled requirements explicitly, but ignore stale or conflicting entries. Memory does not override the current user request. Do not mention internal memory labels or scores.\n",
    );
    for recall in recalls {
        output.push_str(&format!(
            "- [{} | {}] {}\n",
            recall.record.kind.label(),
            recall.record.trust.label(),
            recall.record.content,
        ));
    }
    output.push('\n');
    output
}

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
    let has_explicit_memory_intent = [
        "remember",
        "from now on",
        "call me",
        "my name",
        "记住",
        "以后",
        "叫我",
        "我的名字",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if is_question && !has_explicit_memory_intent {
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
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn is_durable_outcome_content(content: &str, has_tool_evidence: bool) -> bool {
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

fn memory_fingerprint(kind: MemoryKind, content: &str) -> String {
    let value = format!("{}:{}", kind.label(), normalize_memory_text(content));
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn normalize_memory_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn memory_terms(value: &str) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for raw in value.split(|character: char| {
        character.is_whitespace() || (!character.is_alphanumeric() && character != '_')
    }) {
        let token = raw.trim().to_lowercase();
        if token.is_empty() || is_memory_stopword(&token) {
            continue;
        }
        terms.insert(token.clone());
        let chars = token.chars().collect::<Vec<_>>();
        if chars.iter().any(|character| !character.is_ascii()) {
            for pair in chars.windows(2) {
                terms.insert(pair.iter().collect());
            }
        }
    }
    terms
}

fn is_memory_stopword(token: &str) -> bool {
    token.is_ascii()
        && matches!(
            token,
            "a" | "an"
                | "and"
                | "are"
                | "as"
                | "at"
                | "be"
                | "by"
                | "for"
                | "from"
                | "in"
                | "is"
                | "it"
                | "of"
                | "on"
                | "or"
                | "that"
                | "the"
                | "this"
                | "to"
                | "was"
                | "with"
                | "you"
                | "your"
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointOptions {
    pub max_items_per_section: usize,
}

impl Default for CheckpointOptions {
    fn default() -> Self {
        Self {
            max_items_per_section: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCheckpoint {
    pub id: String,
    pub generated_at_ms: u64,
    pub event_count: usize,
    pub task_count: usize,
    pub latest_event_ms: u64,
    pub current_goal: Option<String>,
    pub completed_steps: Vec<String>,
    pub pending_steps: Vec<String>,
    pub decisions: Vec<String>,
    pub file_changes: Vec<String>,
    pub commands_run: Vec<String>,
    pub tool_results: Vec<String>,
    pub retrievals: Vec<String>,
    pub artifacts: Vec<String>,
    pub errors: Vec<String>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreContextPack {
    pub checkpoint: SessionCheckpoint,
    pub text: String,
}

pub fn build_session_checkpoint(events: &[Event], options: CheckpointOptions) -> SessionCheckpoint {
    let generated_at_ms = events
        .iter()
        .map(|event| event.timestamp_ms)
        .max()
        .unwrap_or(0);
    build_session_checkpoint_at(events, options, generated_at_ms)
}

pub fn build_session_checkpoint_at(
    events: &[Event],
    options: CheckpointOptions,
    generated_at_ms: u64,
) -> SessionCheckpoint {
    let limit = options.max_items_per_section.max(1);
    let mut sorted = events.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        left.timestamp_ms
            .cmp(&right.timestamp_ms)
            .then(left.sequence.cmp(&right.sequence))
            .then(left.task_id.0.cmp(&right.task_id.0))
            .then(left.id.0.cmp(&right.id.0))
    });

    let mut task_ids = BTreeSet::new();
    let mut resolved_permissions = BTreeSet::new();
    for event in &sorted {
        task_ids.insert(event.task_id.0.clone());
        if matches!(event.kind, EventKind::PermissionResolved) {
            if let Some(permission_id) = event.metadata.get("permission_id") {
                resolved_permissions.insert(permission_id.clone());
            }
        }
    }

    let mut checkpoint = SessionCheckpoint {
        id: format!("ctx-{generated_at_ms}-{}", events.len()),
        generated_at_ms,
        event_count: events.len(),
        task_count: task_ids.len(),
        latest_event_ms: sorted
            .last()
            .map(|event| event.timestamp_ms)
            .unwrap_or(generated_at_ms),
        current_goal: None,
        completed_steps: Vec::new(),
        pending_steps: Vec::new(),
        decisions: Vec::new(),
        file_changes: Vec::new(),
        commands_run: Vec::new(),
        tool_results: Vec::new(),
        retrievals: Vec::new(),
        artifacts: Vec::new(),
        errors: Vec::new(),
        next_actions: Vec::new(),
    };

    let mut completed_seen = BTreeSet::new();
    let mut pending_seen = BTreeSet::new();
    let mut decision_seen = BTreeSet::new();
    let mut file_seen = BTreeSet::new();
    let mut command_seen = BTreeSet::new();
    let mut tool_seen = BTreeSet::new();
    let mut retrieval_seen = BTreeSet::new();
    let mut artifact_seen = BTreeSet::new();
    let mut error_seen = BTreeSet::new();

    for event in sorted.iter().rev() {
        if checkpoint.current_goal.is_none() {
            checkpoint.current_goal = goal_from_event(event);
        }

        match event.kind {
            EventKind::TaskStatusChanged | EventKind::ModelRequestFinished => {
                push_unique_limited(
                    &mut checkpoint.completed_steps,
                    &mut completed_seen,
                    event_line(event),
                    limit,
                );

                if event.metadata.contains_key("router_model") {
                    push_unique_limited(
                        &mut checkpoint.decisions,
                        &mut decision_seen,
                        router_decision_line(event),
                        limit,
                    );
                }
            }
            EventKind::ToolCallFinished => {
                push_unique_limited(
                    &mut checkpoint.completed_steps,
                    &mut completed_seen,
                    event_line(event),
                    limit,
                );
                push_unique_limited(
                    &mut checkpoint.tool_results,
                    &mut tool_seen,
                    tool_result_line(event),
                    limit,
                );

                if let Some(change) = file_change_line(event) {
                    push_unique_limited(
                        &mut checkpoint.file_changes,
                        &mut file_seen,
                        change,
                        limit,
                    );
                }

                if let Some(command) = command_line(event) {
                    push_unique_limited(
                        &mut checkpoint.commands_run,
                        &mut command_seen,
                        command,
                        limit,
                    );
                }
            }
            EventKind::PermissionRequested | EventKind::ToolCallProposed => {
                let permission_id = event.metadata.get("permission_id");
                let is_pending = permission_id
                    .map(|id| !resolved_permissions.contains(id))
                    .unwrap_or(true);
                if is_pending {
                    push_unique_limited(
                        &mut checkpoint.pending_steps,
                        &mut pending_seen,
                        event_line(event),
                        limit,
                    );
                }
            }
            EventKind::PermissionResolved => {
                push_unique_limited(
                    &mut checkpoint.decisions,
                    &mut decision_seen,
                    event_line(event),
                    limit,
                );
            }
            EventKind::RetrievalPerformed => {
                push_unique_limited(
                    &mut checkpoint.retrievals,
                    &mut retrieval_seen,
                    retrieval_line(event),
                    limit,
                );
            }
            EventKind::Error => {
                push_unique_limited(
                    &mut checkpoint.errors,
                    &mut error_seen,
                    error_line(event),
                    limit,
                );
            }
            EventKind::MessageAdded
            | EventKind::ModelRequestStarted
            | EventKind::ToolCallStarted
            | EventKind::TaskCreated => {}
        }

        for artifact in artifact_lines(event) {
            push_unique_limited(
                &mut checkpoint.artifacts,
                &mut artifact_seen,
                artifact,
                limit,
            );
        }
    }

    checkpoint.next_actions = next_actions_for(&checkpoint);
    checkpoint
}

pub fn build_restore_context_pack(checkpoint: SessionCheckpoint) -> RestoreContextPack {
    let text = checkpoint_to_markdown(&checkpoint);
    RestoreContextPack { checkpoint, text }
}

pub fn conversation_memory_to_markdown(messages: &[Message], max_items: usize) -> String {
    let limit = max_items.max(1);
    let first_user_index = messages.iter().position(|message| {
        matches!(message.role, MessageRole::User)
            && message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
            && !message.content.trim().is_empty()
    });
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(index) = first_user_index {
        let message = &messages[index];
        let value = format!(
            "Initial user request: {}",
            truncate(&sanitize_line(&message.content), 800)
        );
        seen.insert(value.clone());
        selected.push(value);
    }

    let remaining = limit.saturating_sub(selected.len());
    let mut recent = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != first_user_index)
        .filter_map(|(_, message)| conversation_memory_line(message))
        .rev()
        .filter(|value| seen.insert(value.clone()))
        .take(remaining)
        .collect::<Vec<_>>();
    recent.reverse();
    selected.extend(recent);

    if selected.is_empty() {
        return String::new();
    }

    let mut output = String::from(
        "## Conversation Memory\n- Historical extracts preserve continuity. User entries are requirements; assistant entries are prior claims and should be verified when material.\n",
    );
    for item in selected {
        output.push_str("- ");
        output.push_str(&item);
        output.push('\n');
    }
    output.push('\n');
    output
}

fn conversation_memory_line(message: &Message) -> Option<String> {
    if message.content.trim().is_empty()
        || message.metadata.get("kind").map(String::as_str) == Some("tool_observation")
    {
        return None;
    }
    match message.role {
        MessageRole::User => Some(format!(
            "User requirement: {}",
            truncate(&sanitize_line(&message.content), 800)
        )),
        MessageRole::Assistant
            if !message
                .metadata
                .get("tool_call_count")
                .and_then(|value| value.parse::<usize>().ok())
                .is_some_and(|count| count > 0) =>
        {
            Some(format!(
                "Assistant outcome: {}",
                truncate(&sanitize_line(&message.content), 600)
            ))
        }
        _ => None,
    }
}

pub fn checkpoint_to_markdown(checkpoint: &SessionCheckpoint) -> String {
    let mut output = String::new();
    output.push_str("# Cindx Context Checkpoint\n\n");
    output.push_str(&format!("- Checkpoint: {}\n", checkpoint.id));
    output.push_str(&format!("- Generated: {}\n", checkpoint.generated_at_ms));
    output.push_str(&format!("- Latest event: {}\n", checkpoint.latest_event_ms));
    output.push_str(&format!("- Events: {}\n", checkpoint.event_count));
    output.push_str(&format!("- Tasks: {}\n\n", checkpoint.task_count));

    output.push_str("## Current Goal\n");
    output.push_str("- ");
    output.push_str(
        checkpoint
            .current_goal
            .as_deref()
            .unwrap_or("No explicit goal found in the event log."),
    );
    output.push_str("\n\n");

    push_markdown_section(
        &mut output,
        "Completed Steps",
        &checkpoint.completed_steps,
        "No completed steps captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Pending Steps",
        &checkpoint.pending_steps,
        "No pending approvals or proposed actions captured.",
    );
    push_markdown_section(
        &mut output,
        "Decisions",
        &checkpoint.decisions,
        "No decisions captured yet.",
    );
    push_markdown_section(
        &mut output,
        "File Changes",
        &checkpoint.file_changes,
        "No file changes captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Commands Run",
        &checkpoint.commands_run,
        "No shell commands captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Tool Results",
        &checkpoint.tool_results,
        "No tool results captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Retrievals",
        &checkpoint.retrievals,
        "No retrievals captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Artifacts",
        &checkpoint.artifacts,
        "No artifacts captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Errors",
        &checkpoint.errors,
        "No errors captured.",
    );
    push_markdown_section(
        &mut output,
        "Next Actions",
        &checkpoint.next_actions,
        "Ask the user for the next concrete goal.",
    );

    output
}

fn push_markdown_section(output: &mut String, title: &str, items: &[String], empty: &str) {
    output.push_str("## ");
    output.push_str(title);
    output.push('\n');
    if items.is_empty() {
        output.push_str("- ");
        output.push_str(empty);
        output.push_str("\n\n");
        return;
    }

    for item in items {
        output.push_str("- ");
        output.push_str(item);
        output.push('\n');
    }
    output.push('\n');
}

fn goal_from_event(event: &Event) -> Option<String> {
    if matches!(event.kind, EventKind::MessageAdded)
        && event
            .metadata
            .get("role")
            .map(|role| role == "user")
            .unwrap_or(false)
    {
        return event
            .metadata
            .get("content")
            .map(|value| sanitize_line(value));
    }

    first_metadata_value(event, &["prompt"])
        .map(|value| sanitize_line(value))
        .filter(|value| !value.is_empty())
}

fn event_line(event: &Event) -> String {
    let detail = match event.kind {
        EventKind::MessageAdded => first_metadata_value(event, &["content"])
            .map(|value| {
                let role = event
                    .metadata
                    .get("role")
                    .map(String::as_str)
                    .unwrap_or("message");
                format!("{role}: {}", truncate(&sanitize_line(value), 220))
            })
            .unwrap_or_else(|| event.summary.clone()),
        _ => event.summary.clone(),
    };
    format!(
        "{} [{}] {}",
        event.task_id.0,
        event_kind_name(&event.kind),
        sanitize_line(&detail)
    )
}

fn router_decision_line(event: &Event) -> String {
    let model = event
        .metadata
        .get("router_model")
        .map(String::as_str)
        .unwrap_or("unknown model");
    let mode = event
        .metadata
        .get("router_retrieval_mode")
        .map(String::as_str)
        .unwrap_or("unknown retrieval mode");
    let explanation = event
        .metadata
        .get("router_explanation")
        .map(|value| truncate(&sanitize_line(value), 180))
        .unwrap_or_else(|| "no explanation".to_string());
    format!("router selected {model} with {mode}: {explanation}")
}

fn tool_result_line(event: &Event) -> String {
    let tool = first_metadata_value(event, &["tool", "tool_name"]).unwrap_or("tool");
    let status = first_metadata_value(event, &["status"]).unwrap_or("unknown");
    let output = first_metadata_value(event, &["output"])
        .map(|value| truncate(&sanitize_line(value), 220))
        .unwrap_or_else(|| event.summary.clone());
    format!("{tool} {status}: {output}")
}

fn file_change_line(event: &Event) -> Option<String> {
    let tool = first_metadata_value(event, &["tool", "tool_name"])?;
    if tool != "file.write" {
        return None;
    }

    let path = first_metadata_value(event, &["result_path", "path"]).unwrap_or("<unknown path>");
    let bytes = first_metadata_value(event, &["result_bytes", "bytes"]).unwrap_or("unknown");
    Some(format!("wrote {path} ({bytes} bytes)"))
}

fn command_line(event: &Event) -> Option<String> {
    let tool = first_metadata_value(event, &["tool", "tool_name"])?;
    if tool != "shell.run" {
        return None;
    }

    let command =
        first_metadata_value(event, &["result_command", "command"]).unwrap_or("<unknown command>");
    let cwd = first_metadata_value(event, &["result_cwd", "cwd"]).unwrap_or(".");
    let code = first_metadata_value(event, &["result_exit_code", "exit_code"]).unwrap_or("unknown");
    Some(format!("{command} (cwd {cwd}, exit {code})"))
}

fn retrieval_line(event: &Event) -> String {
    let action = first_metadata_value(event, &["action"]).unwrap_or("retrieval");
    let query = first_metadata_value(event, &["query"])
        .map(|value| format!(" query={}", truncate(&sanitize_line(value), 160)))
        .unwrap_or_default();
    let count = first_metadata_value(event, &["selected_count", "result_count", "chunks_indexed"])
        .map(|value| format!(" count={value}"))
        .unwrap_or_default();
    let mode = first_metadata_value(event, &["retrieval_mode", "embedding_backend"])
        .map(|value| format!(" mode={value}"))
        .unwrap_or_default();
    format!("{action}{query}{count}{mode}")
}

fn artifact_lines(event: &Event) -> Vec<String> {
    const KEYS: &[&str] = &[
        "index_path",
        "lancedb_export_path",
        "graph_store_path",
        "artifact_path",
        "text_path",
        "screenshot_path",
        "redaction_manifest_path",
        "result_artifact_path",
        "result_text_path",
        "result_screenshot_path",
        "result_redaction_manifest_path",
    ];

    KEYS.iter()
        .filter_map(|key| {
            event
                .metadata
                .get(*key)
                .map(|value| format!("{key}: {value}"))
        })
        .collect()
}

fn error_line(event: &Event) -> String {
    first_metadata_value(event, &["error"])
        .map(|value| {
            format!(
                "{}: {}",
                event.summary,
                truncate(&sanitize_line(value), 220)
            )
        })
        .unwrap_or_else(|| event_line(event))
}

fn next_actions_for(checkpoint: &SessionCheckpoint) -> Vec<String> {
    let mut actions = Vec::new();
    if !checkpoint.pending_steps.is_empty() {
        actions.push("Resolve pending approvals or proposed local actions.".to_string());
    }
    if !checkpoint.errors.is_empty() {
        actions.push("Inspect the latest error before continuing execution.".to_string());
    }
    if checkpoint.current_goal.is_some() {
        actions.push("Resume from this restore pack and continue the current goal.".to_string());
    } else {
        actions.push("Ask the user for the next concrete goal.".to_string());
    }
    actions
}

fn first_metadata_value<'a>(event: &'a Event, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| event.metadata.get(*key).map(String::as_str))
}

fn push_unique_limited(
    target: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    value: String,
    limit: usize,
) {
    if target.len() >= limit {
        return;
    }

    let value = sanitize_line(&value);
    if value.is_empty() || !seen.insert(value.clone()) {
        return;
    }

    target.push(value);
}

fn sanitize_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(character);
    }
    output
}

fn event_kind_name(kind: &EventKind) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, Metadata, TaskId};

    #[test]
    fn builds_checkpoint_from_event_log() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "Build a context manager")],
            ),
            event(
                2,
                EventKind::ToolCallFinished,
                "Tool finished",
                [
                    ("tool", "file.write"),
                    ("status", "succeeded"),
                    ("output", "wrote file"),
                    ("result_path", "docs/context.md"),
                    ("result_bytes", "42"),
                ],
            ),
            event(
                3,
                EventKind::RetrievalPerformed,
                "RAG search completed",
                [
                    ("action", "search"),
                    ("query", "context manager"),
                    ("selected_count", "2"),
                    ("retrieval_mode", "graph_rag"),
                ],
            ),
        ];

        let checkpoint = build_session_checkpoint_at(&events, CheckpointOptions::default(), 9);

        assert_eq!(
            checkpoint.current_goal.as_deref(),
            Some("Build a context manager")
        );
        assert_eq!(checkpoint.event_count, 3);
        assert_eq!(checkpoint.task_count, 1);
        assert!(checkpoint
            .file_changes
            .iter()
            .any(|item| item.contains("docs/context.md")));
        assert!(checkpoint
            .retrievals
            .iter()
            .any(|item| item.contains("graph_rag")));
    }

    #[test]
    fn excludes_resolved_permissions_from_pending_steps() {
        let events = vec![
            event(
                1,
                EventKind::PermissionRequested,
                "Permission requested",
                [("permission_id", "perm-1"), ("tool", "shell.run")],
            ),
            event(
                2,
                EventKind::PermissionResolved,
                "Permission approved",
                [("permission_id", "perm-1"), ("decision", "allow_once")],
            ),
        ];

        let checkpoint = build_session_checkpoint(&events, CheckpointOptions::default());

        assert!(checkpoint.pending_steps.is_empty());
        assert_eq!(checkpoint.decisions.len(), 1);
    }

    #[test]
    fn restore_pack_contains_operational_sections() {
        let events = vec![event(
            1,
            EventKind::RetrievalPerformed,
            "Workspace indexed",
            [
                ("action", "index"),
                ("chunks_indexed", "5"),
                ("lancedb_export_path", ".cindx/lancedb-records.jsonl"),
                ("graph_store_path", ".cindx/graph.tsv"),
            ],
        )];

        let checkpoint = build_session_checkpoint_at(&events, CheckpointOptions::default(), 7);
        let pack = build_restore_context_pack(checkpoint);

        assert!(pack.text.contains("## Retrievals"));
        assert!(pack.text.contains("lancedb_export_path"));
        assert!(pack.text.contains("graph_store_path"));
    }

    #[test]
    fn limits_sections_to_latest_items() {
        let mut events = Vec::new();
        for sequence in 1..=4 {
            events.push(event(
                sequence,
                EventKind::ToolCallFinished,
                "Tool finished",
                [
                    ("tool", "shell.run"),
                    ("status", "succeeded"),
                    ("output", "ok"),
                    (
                        "result_command",
                        if sequence == 4 { "pwd" } else { "echo ok" },
                    ),
                    ("result_exit_code", "0"),
                ],
            ));
        }

        let checkpoint = build_session_checkpoint(
            &events,
            CheckpointOptions {
                max_items_per_section: 2,
            },
        );

        assert_eq!(checkpoint.commands_run.len(), 2);
        assert!(checkpoint.commands_run[0].contains("pwd"));
    }

    #[test]
    fn conversation_memory_preserves_requirements_and_completed_outcomes() {
        let messages = vec![
            message(MessageRole::User, "Build a local agent", []),
            message(MessageRole::Assistant, "I created the first version", []),
            message(
                MessageRole::Assistant,
                "Calling a tool",
                [("tool_call_count", "2")],
            ),
            message(
                MessageRole::User,
                "Tool observation that should stay out",
                [("kind", "tool_observation")],
            ),
            message(MessageRole::User, "Keep permissions explicit", []),
            message(MessageRole::Assistant, "Permissions are now gated", []),
        ];

        let memory = conversation_memory_to_markdown(&messages, 4);

        assert!(memory.contains("Initial user request: Build a local agent"));
        assert!(memory.contains("User requirement: Keep permissions explicit"));
        assert!(memory.contains("Assistant outcome: Permissions are now gated"));
        assert!(!memory.contains("Calling a tool"));
        assert!(!memory.contains("Tool observation that should stay out"));
    }

    #[test]
    fn durable_memory_requires_a_completed_run_and_preserves_provenance() {
        let mut events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Keep the selected effort scoped to this session"),
                ],
            ),
            event(
                2,
                EventKind::ToolCallFinished,
                "Tool finished",
                [
                    ("tool", "file.write"),
                    ("status", "succeeded"),
                    ("result_path", "src/session.ts"),
                ],
            ),
            event(
                3,
                EventKind::MessageAdded,
                "Assistant message",
                [
                    ("role", "assistant"),
                    ("content", "The effort setting is now stored per session."),
                ],
            ),
        ];

        assert!(extract_durable_memories(&events, "project-a", "session-a").is_empty());
        events.push(event(
            4,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            [],
        ));

        let records = extract_durable_memories(&events, "project-a", "session-a");
        assert_eq!(records.len(), 3);
        assert!(records.iter().any(|record| {
            record.kind == MemoryKind::Requirement
                && record.trust == MemoryTrust::UserStated
                && record.provenance.session_id == "session-a"
        }));
        assert!(records.iter().any(|record| {
            record.kind == MemoryKind::Evidence
                && record.trust == MemoryTrust::ToolVerified
                && record.content.contains("src/session.ts")
        }));
        assert!(records.iter().any(|record| {
            record.kind == MemoryKind::Outcome && record.trust == MemoryTrust::ToolVerified
        }));
    }

    #[test]
    fn memory_merge_deduplicates_and_recall_explains_cross_session_matches() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    (
                        "content",
                        "The sidebar must keep a white frosted glass material",
                    ),
                ],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];
        let mut ledger = MemoryLedger::new("project-a");
        let records = extract_durable_memories(&events, "project-a", "session-a");
        let first = merge_memory_records(&mut ledger, records.clone(), 32);
        let second = merge_memory_records(&mut ledger, records, 32);

        assert_eq!(first.inserted, 1);
        assert_eq!(second.inserted, 0);
        assert_eq!(ledger.records.len(), 1);
        let recalls =
            recall_memories_at(&ledger, "white frosted sidebar", Some("session-b"), 4, 10);
        assert_eq!(recalls.len(), 1);
        assert!(recalls[0].reasons.contains(&"cross_session".to_string()));
        assert!(recalls[0]
            .reasons
            .contains(&"trust:user_stated".to_string()));

        record_memory_recalls(&mut ledger, &recalls, 11);
        assert_eq!(ledger.records[0].recall_count, 1);
        assert_eq!(ledger.records[0].last_recalled_at_ms, Some(11));

        let used = record_memory_observed_uses(
            &mut ledger,
            &[recalls[0].record.id.clone()],
            "Kept the sidebar white with a frosted glass material.",
            12,
        );
        assert_eq!(used, vec![recalls[0].record.id.clone()]);
        assert_eq!(ledger.records[0].observed_use_count, 1);
        assert_eq!(ledger.records[0].last_observed_use_at_ms, Some(12));

        let unrelated = record_memory_observed_uses(
            &mut ledger,
            &[recalls[0].record.id.clone()],
            "The database migration completed.",
            13,
        );
        assert!(unrelated.is_empty());
        assert_eq!(ledger.records[0].observed_use_count, 1);
    }

    #[test]
    fn memory_markdown_keeps_trust_boundaries_explicit() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Project deletion must require confirmation"),
                ],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];
        let mut ledger = MemoryLedger::new("project-a");
        merge_memory_records(
            &mut ledger,
            extract_durable_memories(&events, "project-a", "session-a"),
            32,
        );
        let recalls = recall_memories_at(
            &ledger,
            "project deletion confirmation",
            Some("session-b"),
            2,
            10,
        );
        let markdown = memory_recalls_to_markdown(&recalls);

        assert!(markdown.contains("User-stated entries preserve prior requirements"));
        assert!(markdown.contains("does not override the current user request"));
        assert!(markdown.contains("user_stated"));
        assert!(!markdown.contains("score 0."));
        assert!(!markdown.contains("term_overlap"));
    }

    #[test]
    fn recall_diversifies_sources_instead_of_filling_from_one_session() {
        let mut ledger = MemoryLedger::new("project-a");
        for index in 0..4 {
            let content = format!("Always keep sidebar white material variant {index}");
            let events = vec![
                event(
                    index + 1,
                    EventKind::MessageAdded,
                    "User message",
                    [("role", "user"), ("content", content.as_str())],
                ),
                event(
                    index + 10,
                    EventKind::TaskStatusChanged,
                    "Agent task completed",
                    [],
                ),
            ];
            merge_memory_records(
                &mut ledger,
                extract_durable_memories(&events, "project-a", "session-a"),
                32,
            );
        }
        let other_session = vec![
            event(
                20,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Always keep sidebar white material accessible"),
                ],
            ),
            event(
                21,
                EventKind::TaskStatusChanged,
                "Agent task completed",
                [],
            ),
        ];
        merge_memory_records(
            &mut ledger,
            extract_durable_memories(&other_session, "project-a", "session-b"),
            32,
        );

        let recalls = recall_memories_at(
            &ledger,
            "keep sidebar white material",
            Some("session-c"),
            6,
            30,
        );

        assert_eq!(recalls.len(), 3);
        assert_eq!(
            recalls
                .iter()
                .filter(|recall| recall.record.provenance.session_id == "session-a")
                .count(),
            2
        );
        assert!(recalls
            .iter()
            .any(|recall| recall.record.provenance.session_id == "session-b"));
    }

    #[test]
    fn greetings_do_not_become_durable_memory() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "你好")],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        assert!(extract_durable_memories(&events, "project-a", "session-a").is_empty());
    }

    #[test]
    fn ordinary_questions_and_answers_do_not_become_durable_memory() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "Can you write code?")],
            ),
            event(
                2,
                EventKind::MessageAdded,
                "Assistant message",
                [("role", "assistant"), ("content", "Yes, I can help with code.")],
            ),
            event(3, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        assert!(extract_durable_memories(&events, "project-a", "session-a").is_empty());

        let preference_question = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "Should I always use compact mode?")],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];
        assert!(
            extract_durable_memories(&preference_question, "project-a", "session-a").is_empty()
        );
    }

    #[test]
    fn transient_shell_reads_do_not_fill_long_term_memory() {
        let events = vec![
            event(
                1,
                EventKind::ToolCallFinished,
                "Tool finished",
                [
                    ("tool", "shell.run"),
                    ("status", "succeeded"),
                    ("result_command", "ls -la"),
                ],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        assert!(extract_durable_memories(&events, "project-a", "session-a").is_empty());
    }

    fn message<const N: usize>(
        role: MessageRole,
        content: &str,
        metadata: [(&str, &str); N],
    ) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata: metadata
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    fn event<const N: usize>(
        sequence: u64,
        kind: EventKind,
        summary: &str,
        metadata: [(&str, &str); N],
    ) -> Event {
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: TaskId("task".to_string()),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: summary.to_string(),
            metadata: metadata
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Metadata>(),
        }
    }
}
