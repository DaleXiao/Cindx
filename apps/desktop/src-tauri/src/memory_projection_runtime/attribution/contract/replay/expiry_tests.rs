use super::super::{
    encode_with_digest, insert_memory_recall_attribution_source, memory_event_reference_sha256,
    observed_channels_sha256, MemoryObservedChannels, PersistedMemoryAttribution,
    PersistedMemoryAttributionEvent, MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY,
    MEMORY_ATTRIBUTION_EVENT_METADATA_KEY, MEMORY_ATTRIBUTION_EVENT_SCHEMA,
    MEMORY_ATTRIBUTION_EVENT_SUMMARY, MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY,
    MEMORY_OBSERVED_CHANNELS_SCHEMA, MEMORY_RECALL_EVENT_SUMMARY, MEMORY_TERMINAL_EVENT_SUMMARY,
};
use super::{decode_recall_seed, replay_project_memory_attribution};
use crate::{
    desktop_prelude::{Event, EventId, EventKind, Metadata, SqliteStore, TaskId},
    project_session_persistence::metadata_with_context,
};
use agent_core::{AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_memory::{
    memory_influence_receipt_sha256, record_memory_utility, MemoryEffectKind, MemoryEffectReceipt,
    MemoryEvidenceKind, MemoryExperienceKey, MemoryInfluenceKind, MemoryInfluenceReceipt,
    MemoryKind, MemoryLedger, MemoryProvenance, MemoryRecall, MemoryRecord, MemoryTrust,
    MemoryUtilityDisposition, MemoryUtilitySummary, MAX_MEMORY_UTILITY_VALIDITY_MS,
    MEMORY_EFFECT_RECEIPT_SCHEMA, MEMORY_INFLUENCE_RECEIPT_SCHEMA,
};
use agent_storage::EventStore;

const RECALL_AT_MS: u64 = 1_000;
const TERMINAL_AT_MS: u64 = 2_000;
const ATTRIBUTION_AT_MS: u64 = 3_000;

fn run_context(agent_run_id: &str) -> Metadata {
    [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        (
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            "logical-a".to_string(),
        ),
        (
            AGENT_RUN_ID_METADATA_KEY.to_string(),
            agent_run_id.to_string(),
        ),
        ("steer_epoch".to_string(), "2".to_string()),
        (
            "effective_prompt_objective".to_string(),
            "Verify deterministic expired attribution replay".to_string(),
        ),
    ]
    .into_iter()
    .collect()
}

fn recalled_memory() -> MemoryRecall {
    MemoryRecall {
        record: MemoryRecord {
            id: "memory-a".to_string(),
            fingerprint: "fingerprint-a".to_string(),
            kind: MemoryKind::Evidence,
            trust: MemoryTrust::ToolVerified,
            content: "Historical evidence remains structurally replayable".to_string(),
            importance: 80,
            provenance: MemoryProvenance {
                project_id: "project-a".to_string(),
                session_id: "source-session".to_string(),
                event_id: "source-event".to_string(),
                agent_run_id: Some("source-run".to_string()),
                sequence: 1,
                timestamp_ms: 1,
            },
            source_event_ids: vec!["source-event".to_string()],
            source_session_ids: vec!["source-session".to_string()],
            user_requirement_evidence: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
            recall_count: 0,
            last_recalled_at_ms: None,
            observed_use_count: 0,
            last_observed_use_at_ms: None,
            utility: MemoryUtilitySummary::default(),
            superseded_by: None,
            superseded_at_ms: None,
        },
        score: 1.0,
        reasons: vec!["test".to_string()],
    }
}

#[test]
fn expired_attribution_rebuild_matches_cached_structure_but_is_inactive_now() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let task_id = TaskId("task-expired-attribution".to_string());
    let recall = recalled_memory();
    let recall_context = run_context("attempt-a");
    let mut recall_metadata = [
        ("action".to_string(), "memory_recall".to_string()),
        ("selected_count".to_string(), "1".to_string()),
        ("memory_ids".to_string(), recall.record.id.clone()),
        (
            "query".to_string(),
            "Verify deterministic expired attribution replay".to_string(),
        ),
    ]
    .into_iter()
    .collect();
    assert!(insert_memory_recall_attribution_source(
        &mut recall_metadata,
        &recall_context,
        &"1".repeat(64),
        std::slice::from_ref(&recall),
    ));
    let recall_event = Event {
        id: EventId("recall-event".to_string()),
        task_id: task_id.clone(),
        sequence: 1,
        timestamp_ms: RECALL_AT_MS,
        kind: EventKind::RetrievalPerformed,
        summary: MEMORY_RECALL_EVENT_SUMMARY.to_string(),
        metadata: metadata_with_context(recall_metadata, &recall_context),
    };
    store
        .append(recall_event.clone())
        .expect("historical recall should append");

    let terminal_context = run_context("attempt-c");
    let terminal_event = Event {
        id: EventId("terminal-event".to_string()),
        task_id: task_id.clone(),
        sequence: 2,
        timestamp_ms: TERMINAL_AT_MS,
        kind: EventKind::TaskStatusChanged,
        summary: MEMORY_TERMINAL_EVENT_SUMMARY.to_string(),
        metadata: metadata_with_context(
            [
                ("outcome_ledger_status".to_string(), "recorded".to_string()),
                (
                    agent_runtime::OUTCOME_LEDGER_DIGEST_METADATA_KEY.to_string(),
                    "2".repeat(64),
                ),
                (
                    agent_runtime::GROUNDED_COMPLETION_DIGEST_METADATA_KEY.to_string(),
                    "3".repeat(64),
                ),
            ]
            .into_iter()
            .collect(),
            &terminal_context,
        ),
    };
    store
        .append(terminal_event.clone())
        .expect("historical terminal should append");

    let seed = decode_recall_seed(&recall_event).expect("recall seed should decode");
    let influence_kind = MemoryInfluenceKind::ToolObservedInRecalledRun;
    let evidence_kind = MemoryEvidenceKind::ArtifactReceipt;
    let channels = MemoryObservedChannels {
        schema: MEMORY_OBSERVED_CHANNELS_SCHEMA.to_string(),
        action_sha256: observed_channels_sha256(
            &[],
            1,
            0,
            &"3".repeat(64),
            &"2".repeat(64),
            "evidence_visible",
            Some(influence_kind),
            Some(evidence_kind),
        ),
        goal_delta_fingerprints: Vec::new(),
        successful_tool_evidence: 1,
        verified_postcondition_evidence: 0,
        grounded_completion_digest: "3".repeat(64),
        outcome_ledger_digest: "2".repeat(64),
        grounded_completion_basis: "evidence_visible".to_string(),
        influence: Some(influence_kind),
        evidence: Some(evidence_kind),
    };
    let key = MemoryExperienceKey {
        memory_id: recall.record.id.clone(),
        project_id: "project-a".to_string(),
        session_id: "session-a".to_string(),
        logical_run_id: "logical-a".to_string(),
        steer_epoch: 2,
    };
    let influence = MemoryInfluenceReceipt {
        schema: MEMORY_INFLUENCE_RECEIPT_SCHEMA.to_string(),
        key,
        memory_sha256: seed.memories[0].memory_sha256.clone(),
        task_condition_sha256: seed.task_condition_sha256.clone(),
        environment_sha256: seed.environment_sha256.clone(),
        action_sha256: channels.action_sha256.clone(),
        influence: influence_kind,
        recorded_at_ms: RECALL_AT_MS,
    };
    let effect = MemoryEffectReceipt {
        schema: MEMORY_EFFECT_RECEIPT_SCHEMA.to_string(),
        key: influence.key.clone(),
        influence_receipt_sha256: memory_influence_receipt_sha256(&influence, ATTRIBUTION_AT_MS)
            .expect("historical influence should validate"),
        evidence_sha256: memory_event_reference_sha256(&terminal_event, &"2".repeat(64)),
        outcome_sha256: "2".repeat(64),
        effect: MemoryEffectKind::Inconclusive,
        evidence: evidence_kind,
        recorded_at_ms: TERMINAL_AT_MS,
        valid_until_ms: Some(TERMINAL_AT_MS + MAX_MEMORY_UTILITY_VALIDITY_MS),
    };
    let persisted = PersistedMemoryAttributionEvent {
        schema: MEMORY_ATTRIBUTION_EVENT_SCHEMA.to_string(),
        project_id: "project-a".to_string(),
        session_id: "session-a".to_string(),
        logical_run_id: "logical-a".to_string(),
        recall_agent_run_id: "attempt-a".to_string(),
        terminal_agent_run_id: "attempt-c".to_string(),
        steer_epoch: 2,
        recall_event_id: recall_event.id.0.clone(),
        recall_event_sha256: memory_event_reference_sha256(
            &recall_event,
            recall_event
                .metadata
                .get(MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY)
                .expect("recall digest should exist"),
        ),
        terminal_event_id: terminal_event.id.0.clone(),
        terminal_event_sha256: effect.evidence_sha256.clone(),
        observed_channels: channels,
        attributions: vec![PersistedMemoryAttribution {
            memory_id: recall.record.id.clone(),
            influence: Some(influence.clone()),
            effect: Some(effect.clone()),
        }],
    };
    let (encoded, digest) = encode_with_digest(&persisted).expect("event should encode");
    let attribution_event = Event {
        id: EventId("attribution-event".to_string()),
        task_id,
        sequence: 3,
        timestamp_ms: ATTRIBUTION_AT_MS,
        kind: EventKind::RetrievalPerformed,
        summary: MEMORY_ATTRIBUTION_EVENT_SUMMARY.to_string(),
        metadata: metadata_with_context(
            [
                ("action".to_string(), "memory_attribution".to_string()),
                ("memory_utility".to_string(), "unknown".to_string()),
                ("selected_count".to_string(), "1".to_string()),
                (MEMORY_ATTRIBUTION_EVENT_METADATA_KEY.to_string(), encoded),
                (
                    MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY.to_string(),
                    digest,
                ),
            ]
            .into_iter()
            .collect(),
            &terminal_context,
        ),
    };

    let mut cached = MemoryLedger::new("project-a");
    cached.records.push(recall.record.clone());
    assert!(record_memory_utility(
        &mut cached.records[0],
        &influence,
        &effect,
        ATTRIBUTION_AT_MS,
    )
    .expect("historical cache should accept attribution"));
    let mut rebuilt = MemoryLedger::new("project-a");
    rebuilt.records.push(recall.record);
    assert_eq!(
        replay_project_memory_attribution(&store, &mut rebuilt, &attribution_event, "project-a",)
            .expect("historical attribution should replay"),
        1
    );
    assert_eq!(rebuilt, cached);

    let now_ms = crate::runtime_values::current_time_millis();
    assert!(now_ms > TERMINAL_AT_MS + MAX_MEMORY_UTILITY_VALIDITY_MS);
    assert!(!rebuilt.records[0]
        .utility
        .has_active_attribution_for_at(&rebuilt.records[0], now_ms));
    assert_eq!(
        rebuilt.records[0]
            .utility
            .disposition_for_at(&rebuilt.records[0], now_ms),
        MemoryUtilityDisposition::Unknown
    );
}
