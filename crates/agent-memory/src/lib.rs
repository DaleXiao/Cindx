use serde::{Deserialize, Serialize};

mod checkpoint;
mod extraction;
mod ledger;
mod memory_text;
mod recall;

pub use checkpoint::{
    build_restore_context_pack, build_session_checkpoint, build_session_checkpoint_at,
    checkpoint_to_markdown, conversation_memory_to_markdown, CheckpointOptions, RestoreContextPack,
    SessionCheckpoint,
};
pub use extraction::extract_durable_memories;
pub use ledger::merge_memory_records;
pub use recall::{
    fuse_memory_recalls_at, memory_recalls_to_markdown, recall_memories_at,
    record_memory_observed_uses, record_memory_recalls,
};

pub const MEMORY_LEDGER_SCHEMA: &str = "cindx.memory-ledger.v4";

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
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub superseded_at_ms: Option<u64>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Event, EventId, EventKind, Message, MessageRole, Metadata, TaskId};
    use std::collections::BTreeMap;

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
    fn user_requirements_survive_incomplete_runs_but_outcomes_require_completion() {
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

        let incomplete = extract_durable_memories(&events, "project-a", "session-a");
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].kind, MemoryKind::Requirement);
        assert_eq!(incomplete[0].trust, MemoryTrust::UserStated);
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
            record.kind == MemoryKind::Outcome
                && record.trust == MemoryTrust::AssistantReported
                && record.source_event_ids == vec!["event-2".to_string()]
        }));
    }

    #[test]
    fn unrelated_successful_tool_does_not_verify_assistant_outcome() {
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
            event(
                2,
                EventKind::MessageAdded,
                "Assistant message",
                [
                    ("role", "assistant"),
                    ("content", "Implemented the requested architecture changes."),
                ],
            ),
            event(3, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        let records = extract_durable_memories(&events, "project-a", "session-a");
        let outcome = records
            .iter()
            .find(|record| record.kind == MemoryKind::Outcome)
            .expect("assistant outcome should remain available as a low-trust memory");

        assert_eq!(outcome.trust, MemoryTrust::AssistantReported);
        assert_eq!(outcome.source_event_ids, vec!["event-2".to_string()]);
    }

    #[test]
    fn evidence_from_an_older_user_turn_does_not_back_a_new_outcome() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "Create the first artifact")],
            ),
            event(
                2,
                EventKind::ToolCallFinished,
                "Tool finished",
                [
                    ("tool", "file.write"),
                    ("status", "succeeded"),
                    ("result_path", "old.txt"),
                ],
            ),
            event(
                3,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Now update the database schema"),
                ],
            ),
            event(
                4,
                EventKind::MessageAdded,
                "Assistant message",
                [
                    ("role", "assistant"),
                    ("content", "Implemented the requested database update."),
                ],
            ),
            event(5, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        let records = extract_durable_memories(&events, "project-a", "session-a");
        let outcome = records
            .iter()
            .find(|record| record.kind == MemoryKind::Outcome)
            .expect("explicit completion language remains a low-trust outcome");

        assert_eq!(outcome.source_event_ids, vec!["event-4".to_string()]);
        assert_eq!(outcome.importance, 58);
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
    fn newer_scoped_requirement_supersedes_but_does_not_delete_history() {
        let older = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "From now on call me Dale")],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];
        let newer = vec![
            event(
                10,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "From now on call me Alex")],
            ),
            event(11, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];
        let mut ledger = MemoryLedger::new("project-a");
        merge_memory_records(
            &mut ledger,
            extract_durable_memories(&older, "project-a", "session-a"),
            32,
        );
        merge_memory_records(
            &mut ledger,
            extract_durable_memories(&newer, "project-a", "session-b"),
            32,
        );

        assert_eq!(ledger.records.len(), 2);
        let dale = ledger
            .records
            .iter()
            .find(|record| record.content.contains("Dale"))
            .expect("older requirement remains auditable");
        let alex = ledger
            .records
            .iter()
            .find(|record| record.content.contains("Alex"))
            .expect("newer requirement is retained");
        assert_eq!(dale.superseded_by.as_deref(), Some(alex.id.as_str()));
        assert!(alex.superseded_by.is_none());

        let recalls = recall_memories_at(
            &ledger,
            "What name should you call me?",
            Some("session-c"),
            4,
            20,
        );
        assert_eq!(recalls.len(), 1);
        assert!(recalls[0].record.content.contains("Alex"));

        // Replaying the older event during a ledger rebuild must not reactivate it.
        merge_memory_records(
            &mut ledger,
            extract_durable_memories(&older, "project-a", "session-a"),
            32,
        );
        assert!(ledger
            .records
            .iter()
            .find(|record| record.content.contains("Dale"))
            .is_some_and(|record| record.superseded_by.is_some()));
    }

    #[test]
    fn questions_about_identity_do_not_become_identity_requirements() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "What should you call me?")],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        assert!(extract_durable_memories(&events, "project-a", "session-a").is_empty());
    }

    #[test]
    fn repeatedly_recalled_but_unused_memory_is_deprioritized() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Always preserve the session effort setting"),
                ],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];
        let mut useful = extract_durable_memories(&events, "project-a", "session-a")
            .into_iter()
            .next()
            .expect("requirement memory");
        useful.id = "useful".to_string();
        useful.fingerprint = "useful".to_string();
        useful.recall_count = 10;
        useful.observed_use_count = 8;
        let mut noisy = useful.clone();
        noisy.id = "noisy".to_string();
        noisy.fingerprint = "noisy".to_string();
        noisy.recall_count = 10;
        noisy.observed_use_count = 0;
        let ledger = MemoryLedger {
            records: vec![noisy, useful],
            ..MemoryLedger::new("project-a")
        };

        let recalls = recall_memories_at(
            &ledger,
            "preserve session effort setting",
            Some("session-b"),
            4,
            10,
        );

        assert_eq!(recalls[0].record.id, "useful");
        assert!(recalls[0].score > recalls[1].score);
    }

    #[test]
    fn semantic_memory_recall_fills_lexical_blind_spots_without_losing_trust_weighting() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    (
                        "content",
                        "Always preserve the frosted translucent title material",
                    ),
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
        let lexical = recall_memories_at(
            &ledger,
            "keep the header visually consistent",
            Some("session-b"),
            4,
            10,
        );
        assert!(lexical.is_empty());

        let semantic_scores = [(ledger.records[0].id.clone(), 0.91)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let recalls =
            fuse_memory_recalls_at(&ledger, lexical, &semantic_scores, Some("session-b"), 4, 10);

        assert_eq!(recalls.len(), 1);
        assert!(recalls[0].reasons.contains(&"semantic_vector".to_string()));
        assert!(recalls[0]
            .reasons
            .contains(&"trust:user_stated".to_string()));
        assert!(recalls[0].reasons.contains(&"cross_session".to_string()));
    }

    #[test]
    fn hybrid_memory_consensus_outranks_single_channel_candidates() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Always preserve the translucent title material"),
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
        let lexical_only = ledger.records[0].clone();
        let mut hybrid = lexical_only.clone();
        hybrid.id = "memory-hybrid".to_string();
        hybrid.fingerprint = "fingerprint-hybrid".to_string();
        hybrid.content = "Keep the frosted header visually consistent".to_string();
        let mut semantic_only = lexical_only.clone();
        semantic_only.id = "memory-semantic".to_string();
        semantic_only.fingerprint = "fingerprint-semantic".to_string();
        semantic_only.content = "Retain the glass surface".to_string();
        ledger.records = vec![lexical_only.clone(), hybrid.clone(), semantic_only.clone()];
        let lexical = vec![
            MemoryRecall {
                record: lexical_only,
                score: 0.9,
                reasons: vec!["term_overlap:3".to_string()],
            },
            MemoryRecall {
                record: hybrid.clone(),
                score: 0.8,
                reasons: vec!["term_overlap:2".to_string()],
            },
        ];
        let semantic_scores = [(hybrid.id.clone(), 0.9), (semantic_only.id.clone(), 0.95)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let recalls =
            fuse_memory_recalls_at(&ledger, lexical, &semantic_scores, Some("session-b"), 4, 10);

        assert_eq!(recalls[0].record.id, "memory-hybrid");
        assert!(recalls[0].reasons.contains(&"hybrid_consensus".to_string()));
        assert!(recalls.iter().all(|recall| recall.score.is_finite()));
    }

    #[test]
    fn semantic_memory_calibration_preserves_trust_ordering() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Always preserve the white sidebar material"),
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
        let trusted = ledger.records[0].clone();
        let mut reported = trusted.clone();
        reported.id = "memory-reported".to_string();
        reported.fingerprint = "fingerprint-reported".to_string();
        reported.trust = MemoryTrust::AssistantReported;
        reported.content = "Preserve the white sidebar appearance".to_string();
        ledger.records.push(reported.clone());
        let semantic_scores = [(trusted.id.clone(), 0.9), (reported.id.clone(), 0.9)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let recalls = fuse_memory_recalls_at(
            &ledger,
            Vec::new(),
            &semantic_scores,
            Some("session-b"),
            4,
            10,
        );

        assert_eq!(recalls[0].record.id, trusted.id);
        assert!(recalls[0].score > recalls[1].score);
    }

    #[test]
    fn durable_memory_recognizes_chinese_need_requirements() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "左侧边栏需要白色高不透明度的磨砂玻璃效果"),
                ],
            ),
            event(2, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        let records = extract_durable_memories(&events, "project-a", "session-a");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, MemoryKind::Requirement);
        assert_eq!(records[0].trust, MemoryTrust::UserStated);
    }

    #[test]
    fn recall_matches_simple_english_plural_variants() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    (
                        "content",
                        "Do not change the validated macOS traffic light vertical position",
                    ),
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
            "Build without moving the traffic lights",
            Some("session-b"),
            3,
            10,
        );

        assert_eq!(recalls.len(), 1);
        assert!(recalls[0]
            .record
            .content
            .contains("traffic light vertical position"));
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
            event(21, EventKind::TaskStatusChanged, "Agent task completed", []),
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
                [
                    ("role", "assistant"),
                    ("content", "Yes, I can help with code."),
                ],
            ),
            event(3, EventKind::TaskStatusChanged, "Agent task completed", []),
        ];

        assert!(extract_durable_memories(&events, "project-a", "session-a").is_empty());

        let preference_question = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [
                    ("role", "user"),
                    ("content", "Should I always use compact mode?"),
                ],
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
