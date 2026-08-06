use super::replay::decode_recall_seed;
use super::{
    encode_with_digest, is_sha256_hex, memory_event_has_logical_scope,
    memory_event_reference_sha256, strict_run_scope, valid_observed_channels,
    CompletionMemoryAttributionObservation, PersistedMemoryAttribution,
    PersistedMemoryAttributionEvent, MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY,
    MEMORY_ATTRIBUTION_EVENT_METADATA_KEY, MEMORY_ATTRIBUTION_EVENT_SCHEMA,
    MEMORY_ATTRIBUTION_EVENT_SUMMARY, MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY,
    MEMORY_RECALL_EVENT_SUMMARY, MEMORY_TERMINAL_EVENT_SUMMARY,
};
use crate::{
    desktop_prelude::{EventKind, Metadata, SqliteStore, StorageError, TaskId},
    event_persistence::append_event,
    project_session_persistence::metadata_with_context,
};
use agent_core::{AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_memory::{
    memory_influence_receipt_sha256, MemoryEffectKind, MemoryEffectReceipt, MemoryExperienceKey,
    MemoryInfluenceReceipt, MAX_MEMORY_UTILITY_VALIDITY_MS, MEMORY_EFFECT_RECEIPT_SCHEMA,
    MEMORY_INFLUENCE_RECEIPT_SCHEMA,
};

pub(crate) fn append_project_memory_attribution(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    steer_epoch: u64,
    observation: &CompletionMemoryAttributionObservation,
) -> Result<usize, StorageError> {
    let Some((project_id, session_id, logical_run_id, terminal_agent_run_id, context_epoch)) =
        strict_run_scope(run_context)
    else {
        return Ok(0);
    };
    if context_epoch != steer_epoch || !valid_observed_channels(&observation.channels) {
        return Ok(0);
    }
    let logical_run_events = store.list_by_task_and_metadata(
        task_id,
        LOGICAL_AGENT_RUN_ID_METADATA_KEY,
        logical_run_id,
    )?;
    if logical_run_events.iter().any(|event| {
        memory_event_has_logical_scope(
            event,
            task_id,
            project_id,
            session_id,
            logical_run_id,
            steer_epoch,
        ) && event.kind == EventKind::RetrievalPerformed
            && event.summary == MEMORY_ATTRIBUTION_EVENT_SUMMARY
            && event.metadata.get("action").map(String::as_str) == Some("memory_attribution")
    }) {
        return Ok(0);
    }
    let terminal_events = logical_run_events
        .iter()
        .filter(|event| {
            memory_event_has_logical_scope(
                event,
                task_id,
                project_id,
                session_id,
                logical_run_id,
                steer_epoch,
            ) && event.kind == EventKind::TaskStatusChanged
                && event.summary == MEMORY_TERMINAL_EVENT_SUMMARY
                && event
                    .metadata
                    .get(AGENT_RUN_ID_METADATA_KEY)
                    .map(String::as_str)
                    == Some(terminal_agent_run_id)
                && event
                    .metadata
                    .get("outcome_ledger_status")
                    .map(String::as_str)
                    == Some("recorded")
        })
        .collect::<Vec<_>>();
    if terminal_events.len() != 1 {
        return Ok(0);
    }
    let terminal_event = terminal_events[0];
    let recall_event = logical_run_events
        .iter()
        .filter(|event| {
            memory_event_has_logical_scope(
                event,
                task_id,
                project_id,
                session_id,
                logical_run_id,
                steer_epoch,
            ) && event.kind == EventKind::RetrievalPerformed
                && event.summary == MEMORY_RECALL_EVENT_SUMMARY
                && event.metadata.get("action").map(String::as_str) == Some("memory_recall")
                && event.sequence < terminal_event.sequence
        })
        .max_by_key(|event| event.sequence);
    let Some(recall_event) = recall_event else {
        return Ok(0);
    };
    if recall_event.sequence >= terminal_event.sequence
        || recall_event.timestamp_ms > terminal_event.timestamp_ms
    {
        return Ok(0);
    }
    let Some(seed) = decode_recall_seed(recall_event) else {
        return Ok(0);
    };
    if seed.project_id != project_id
        || seed.session_id != session_id
        || seed.logical_run_id != logical_run_id
        || seed.steer_epoch != steer_epoch
    {
        return Ok(0);
    }
    let Some(outcome_sha256) = terminal_event
        .metadata
        .get(agent_runtime::OUTCOME_LEDGER_DIGEST_METADATA_KEY)
        .filter(|digest| is_sha256_hex(digest))
    else {
        return Ok(0);
    };
    if observation.channels.outcome_ledger_digest != *outcome_sha256
        || terminal_event
            .metadata
            .get(agent_runtime::GROUNDED_COMPLETION_DIGEST_METADATA_KEY)
            != Some(&observation.channels.grounded_completion_digest)
    {
        return Ok(0);
    }
    let recall_event_sha256 = memory_event_reference_sha256(
        recall_event,
        recall_event
            .metadata
            .get(MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY)
            .map(String::as_str)
            .unwrap_or_default(),
    );
    let terminal_event_sha256 = memory_event_reference_sha256(terminal_event, outcome_sha256);
    let mut attributions = Vec::with_capacity(seed.memories.len());
    for memory in &seed.memories {
        let key = MemoryExperienceKey {
            memory_id: memory.memory_id.clone(),
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            logical_run_id: logical_run_id.to_string(),
            steer_epoch,
        };
        let (influence, effect) = match (
            observation.channels.influence,
            observation.channels.evidence,
        ) {
            (Some(influence_kind), Some(evidence_kind)) => {
                let influence = MemoryInfluenceReceipt {
                    schema: MEMORY_INFLUENCE_RECEIPT_SCHEMA.to_string(),
                    key,
                    memory_sha256: memory.memory_sha256.clone(),
                    task_condition_sha256: seed.task_condition_sha256.clone(),
                    environment_sha256: seed.environment_sha256.clone(),
                    action_sha256: observation.channels.action_sha256.clone(),
                    influence: influence_kind,
                    recorded_at_ms: recall_event.timestamp_ms,
                };
                let Some(influence_receipt_sha256) =
                    memory_influence_receipt_sha256(&influence, terminal_event.timestamp_ms)
                else {
                    return Ok(0);
                };
                let effect = MemoryEffectReceipt {
                    schema: MEMORY_EFFECT_RECEIPT_SCHEMA.to_string(),
                    key: influence.key.clone(),
                    influence_receipt_sha256,
                    evidence_sha256: terminal_event_sha256.clone(),
                    outcome_sha256: outcome_sha256.clone(),
                    effect: MemoryEffectKind::Inconclusive,
                    evidence: evidence_kind,
                    recorded_at_ms: terminal_event.timestamp_ms,
                    valid_until_ms: if memory.durable_user_requirement {
                        None
                    } else {
                        terminal_event
                            .timestamp_ms
                            .checked_add(MAX_MEMORY_UTILITY_VALIDITY_MS)
                    },
                };
                (Some(influence), Some(effect))
            }
            (None, None) => (None, None),
            _ => return Ok(0),
        };
        attributions.push(PersistedMemoryAttribution {
            memory_id: memory.memory_id.clone(),
            influence,
            effect,
        });
    }
    let persisted = PersistedMemoryAttributionEvent {
        schema: MEMORY_ATTRIBUTION_EVENT_SCHEMA.to_string(),
        project_id: project_id.to_string(),
        session_id: session_id.to_string(),
        logical_run_id: logical_run_id.to_string(),
        recall_agent_run_id: seed.agent_run_id.clone(),
        terminal_agent_run_id: terminal_agent_run_id.to_string(),
        steer_epoch,
        recall_event_id: recall_event.id.0.clone(),
        recall_event_sha256,
        terminal_event_id: terminal_event.id.0.clone(),
        terminal_event_sha256,
        observed_channels: observation.channels.clone(),
        attributions,
    };
    let Some((encoded, digest)) = encode_with_digest(&persisted) else {
        return Ok(0);
    };
    let metadata = [
        ("action".to_string(), "memory_attribution".to_string()),
        (
            "memory_attribution_schema".to_string(),
            MEMORY_ATTRIBUTION_EVENT_SCHEMA.to_string(),
        ),
        ("memory_utility".to_string(), "unknown".to_string()),
        (
            "selected_count".to_string(),
            persisted.attributions.len().to_string(),
        ),
        (
            "attributed_count".to_string(),
            persisted
                .attributions
                .iter()
                .filter(|item| item.effect.is_some())
                .count()
                .to_string(),
        ),
        (MEMORY_ATTRIBUTION_EVENT_METADATA_KEY.to_string(), encoded),
        (
            MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY.to_string(),
            digest,
        ),
    ]
    .into_iter()
    .collect();
    append_event(
        store,
        task_id,
        EventKind::RetrievalPerformed,
        MEMORY_ATTRIBUTION_EVENT_SUMMARY,
        metadata_with_context(metadata, run_context),
    )?;
    Ok(persisted.attributions.len())
}

#[cfg(test)]
mod tests {
    use super::{
        append_project_memory_attribution, CompletionMemoryAttributionObservation, EventKind,
        Metadata, SqliteStore, TaskId, AGENT_RUN_ID_METADATA_KEY,
        LOGICAL_AGENT_RUN_ID_METADATA_KEY, MEMORY_ATTRIBUTION_EVENT_SUMMARY,
        MEMORY_RECALL_EVENT_SUMMARY, MEMORY_TERMINAL_EVENT_SUMMARY,
    };
    use crate::{
        event_persistence::append_event, project_session_persistence::metadata_with_context,
    };
    use agent_memory::{
        MemoryKind, MemoryProvenance, MemoryRecord, MemoryTrust, MemoryUtilitySummary,
    };

    use super::super::replay::decode_attribution_event;
    use super::super::{
        insert_memory_recall_attribution_source, observed_channels_sha256, MemoryObservedChannels,
        MEMORY_OBSERVED_CHANNELS_SCHEMA,
    };

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
                "Verify the recovered run".to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    fn recalled_memory() -> agent_memory::MemoryRecall {
        agent_memory::MemoryRecall {
            record: MemoryRecord {
                id: "memory-a".to_string(),
                fingerprint: "fingerprint-a".to_string(),
                kind: MemoryKind::Evidence,
                trust: MemoryTrust::ToolVerified,
                content: "Recovered runs retain the logical attribution scope".to_string(),
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
    fn recovery_attempts_join_once_by_logical_run_and_keep_physical_audit_ids() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-memory-attribution".to_string());
        let recall_context = run_context("attempt-a");
        let recall = recalled_memory();
        let mut recall_metadata = [
            ("action".to_string(), "memory_recall".to_string()),
            ("selected_count".to_string(), "1".to_string()),
            ("memory_ids".to_string(), recall.record.id.clone()),
            ("query".to_string(), "Verify the recovered run".to_string()),
        ]
        .into_iter()
        .collect();
        assert!(insert_memory_recall_attribution_source(
            &mut recall_metadata,
            &recall_context,
            &"1".repeat(64),
            std::slice::from_ref(&recall),
        ));
        append_event(
            &mut store,
            &task_id,
            EventKind::RetrievalPerformed,
            MEMORY_RECALL_EVENT_SUMMARY,
            metadata_with_context(recall_metadata, &recall_context),
        )
        .expect("recall should append");

        let middle_context = run_context("attempt-b");
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent recovery resumed",
            middle_context,
        )
        .expect("middle attempt should append");

        let terminal_context = run_context("attempt-c");
        let terminal_metadata = metadata_with_context(
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
                (
                    "grounded_completion_basis".to_string(),
                    "evidence_visible".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            &terminal_context,
        );
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            MEMORY_TERMINAL_EVENT_SUMMARY,
            terminal_metadata.clone(),
        )
        .expect("terminal should append");
        let action_sha256 = observed_channels_sha256(
            &[],
            0,
            0,
            &"3".repeat(64),
            &"2".repeat(64),
            "evidence_visible",
            None,
            None,
        );
        let observation = CompletionMemoryAttributionObservation {
            channels: MemoryObservedChannels {
                schema: MEMORY_OBSERVED_CHANNELS_SCHEMA.to_string(),
                action_sha256,
                goal_delta_fingerprints: Vec::new(),
                successful_tool_evidence: 0,
                verified_postcondition_evidence: 0,
                grounded_completion_digest: "3".repeat(64),
                outcome_ledger_digest: "2".repeat(64),
                grounded_completion_basis: "evidence_visible".to_string(),
                influence: None,
                evidence: None,
            },
        };

        assert_eq!(
            append_project_memory_attribution(
                &mut store,
                &task_id,
                &terminal_context,
                2,
                &observation,
            )
            .expect("attribution should append"),
            1
        );
        assert_eq!(
            append_project_memory_attribution(
                &mut store,
                &task_id,
                &terminal_context,
                2,
                &observation,
            )
            .expect("duplicate attribution should be ignored"),
            0
        );

        let events = store
            .list_by_task_and_metadata(&task_id, LOGICAL_AGENT_RUN_ID_METADATA_KEY, "logical-a")
            .expect("logical run should load");
        let attributions = events
            .iter()
            .filter(|event| event.summary == MEMORY_ATTRIBUTION_EVENT_SUMMARY)
            .collect::<Vec<_>>();
        assert_eq!(attributions.len(), 1);
        let persisted =
            decode_attribution_event(attributions[0]).expect("attribution envelope should decode");
        assert_eq!(persisted.recall_agent_run_id, "attempt-a");
        assert_eq!(persisted.terminal_agent_run_id, "attempt-c");
        assert_eq!(persisted.logical_run_id, "logical-a");
    }
}
