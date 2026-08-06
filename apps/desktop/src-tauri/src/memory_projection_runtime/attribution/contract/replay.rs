use super::{
    decode_with_digest, is_sha256_hex, memory_event_has_scope, memory_event_reference_sha256,
    sha256_hex, sha256_json, strict_run_scope, valid_id, valid_observed_channels,
    MemoryRecallAttributionSeed, PersistedMemoryAttributionEvent, AGENT_MEMORY_RECALL_LIMIT,
    MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY, MEMORY_ATTRIBUTION_EVENT_METADATA_KEY,
    MEMORY_ATTRIBUTION_EVENT_SCHEMA, MEMORY_ATTRIBUTION_EVENT_SUMMARY,
    MEMORY_ATTRIBUTION_HASH_DOMAIN, MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY,
    MEMORY_ATTRIBUTION_SOURCE_METADATA_KEY, MEMORY_RECALL_ATTRIBUTION_SOURCE_SCHEMA,
    MEMORY_RECALL_EVENT_SUMMARY, MEMORY_TERMINAL_EVENT_SUMMARY,
};
use crate::desktop_prelude::{
    Event, EventKind, MemoryKind, MemoryLedger, SqliteStore, StorageError,
};
use agent_core::{AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_memory::{record_memory_utility, MemoryEffectKind, MemoryTrust};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn replay_project_memory_attribution(
    store: &SqliteStore,
    ledger: &mut MemoryLedger,
    event: &Event,
    project_id: &str,
) -> Result<usize, StorageError> {
    if event.kind != EventKind::RetrievalPerformed
        || event.summary != MEMORY_ATTRIBUTION_EVENT_SUMMARY
        || event.metadata.get("action").map(String::as_str) != Some("memory_attribution")
        || event.metadata.get("project_id").map(String::as_str) != Some(project_id)
    {
        return Ok(0);
    }
    let Some(persisted) = decode_attribution_event(event) else {
        return Ok(0);
    };
    if persisted.schema != MEMORY_ATTRIBUTION_EVENT_SCHEMA
        || persisted.project_id != project_id
        || !persisted_scope_matches_event(&persisted, event)
        || persisted.attributions.len() > AGENT_MEMORY_RECALL_LIMIT
        || !valid_observed_channels(&persisted.observed_channels)
    {
        return Ok(0);
    }
    let (Some(recall_event), Some(terminal_event)) = (
        store.event_by_id(&persisted.recall_event_id)?,
        store.event_by_id(&persisted.terminal_event_id)?,
    ) else {
        return Ok(0);
    };
    if !source_events_match_attribution(&persisted, event, &recall_event, &terminal_event) {
        return Ok(0);
    }
    let Some(seed) = decode_recall_seed(&recall_event) else {
        return Ok(0);
    };
    if seed.project_id != persisted.project_id
        || seed.session_id != persisted.session_id
        || seed.logical_run_id != persisted.logical_run_id
        || seed.agent_run_id != persisted.recall_agent_run_id
        || seed.steer_epoch != persisted.steer_epoch
        || seed.memories.len() != persisted.attributions.len()
    {
        return Ok(0);
    }
    let seed_memories = seed
        .memories
        .iter()
        .map(|item| {
            (
                item.memory_id.as_str(),
                (item.memory_sha256.as_str(), item.durable_user_requirement),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut replayed_ids = BTreeSet::new();
    let mut staged_records = Vec::new();
    let mut updated = 0usize;
    for attribution in &persisted.attributions {
        let Some((memory_sha256, durable_user_requirement)) =
            seed_memories.get(attribution.memory_id.as_str())
        else {
            return Ok(0);
        };
        if !replayed_ids.insert(attribution.memory_id.as_str()) {
            return Ok(0);
        }
        match (&attribution.influence, &attribution.effect) {
            (None, None) if persisted.observed_channels.influence.is_none() => continue,
            (Some(influence), Some(effect))
                if persisted.observed_channels.influence == Some(influence.influence)
                    && persisted.observed_channels.evidence == Some(effect.evidence)
                    && effect.effect == MemoryEffectKind::Inconclusive
                    && influence.key.memory_id == attribution.memory_id
                    && influence.key.project_id == persisted.project_id
                    && influence.key.session_id == persisted.session_id
                    && influence.key.logical_run_id == persisted.logical_run_id
                    && influence.key.steer_epoch == persisted.steer_epoch
                    && influence.memory_sha256 == *memory_sha256
                    && influence.task_condition_sha256 == seed.task_condition_sha256
                    && influence.environment_sha256 == seed.environment_sha256
                    && influence.action_sha256 == persisted.observed_channels.action_sha256
                    && influence.recorded_at_ms == recall_event.timestamp_ms
                    && effect.evidence_sha256 == persisted.terminal_event_sha256
                    && effect.outcome_sha256
                        == persisted.observed_channels.outcome_ledger_digest
                    && effect.recorded_at_ms == terminal_event.timestamp_ms =>
            {
                let Some(record_index) = ledger
                    .records
                    .iter()
                    .position(|record| record.id == attribution.memory_id)
                else {
                    return Ok(0);
                };
                let mut candidate = ledger.records[record_index].clone();
                if *durable_user_requirement
                    != (candidate.kind == MemoryKind::Requirement
                        && candidate.trust == MemoryTrust::UserStated)
                {
                    return Ok(0);
                }
                match record_memory_utility(&mut candidate, influence, effect, event.timestamp_ms) {
                    Ok(changed) => {
                        updated = updated.saturating_add(usize::from(changed));
                        staged_records.push((record_index, candidate));
                    }
                    Err(_) => return Ok(0),
                }
            }
            _ => return Ok(0),
        }
    }
    for (record_index, candidate) in staged_records {
        ledger.records[record_index] = candidate;
    }
    Ok(updated)
}

fn source_events_match_attribution(
    persisted: &PersistedMemoryAttributionEvent,
    attribution_event: &Event,
    recall_event: &Event,
    terminal_event: &Event,
) -> bool {
    memory_event_has_scope(
        recall_event,
        &attribution_event.task_id,
        &persisted.project_id,
        &persisted.session_id,
        &persisted.logical_run_id,
        &persisted.recall_agent_run_id,
        persisted.steer_epoch,
    ) && memory_event_has_scope(
        terminal_event,
        &attribution_event.task_id,
        &persisted.project_id,
        &persisted.session_id,
        &persisted.logical_run_id,
        &persisted.terminal_agent_run_id,
        persisted.steer_epoch,
    ) && recall_event.kind == EventKind::RetrievalPerformed
        && recall_event.summary == MEMORY_RECALL_EVENT_SUMMARY
        && terminal_event.kind == EventKind::TaskStatusChanged
        && terminal_event.summary == MEMORY_TERMINAL_EVENT_SUMMARY
        && recall_event.sequence < terminal_event.sequence
        && terminal_event.sequence < attribution_event.sequence
        && recall_event.timestamp_ms <= terminal_event.timestamp_ms
        && terminal_event.timestamp_ms <= attribution_event.timestamp_ms
        && memory_event_reference_sha256(
            recall_event,
            recall_event
                .metadata
                .get(MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY)
                .map(String::as_str)
                .unwrap_or_default(),
        ) == persisted.recall_event_sha256
        && memory_event_reference_sha256(
            terminal_event,
            terminal_event
                .metadata
                .get(agent_runtime::OUTCOME_LEDGER_DIGEST_METADATA_KEY)
                .map(String::as_str)
                .unwrap_or_default(),
        ) == persisted.terminal_event_sha256
        && terminal_event
            .metadata
            .get(agent_runtime::GROUNDED_COMPLETION_DIGEST_METADATA_KEY)
            == Some(&persisted.observed_channels.grounded_completion_digest)
        && terminal_event
            .metadata
            .get(agent_runtime::OUTCOME_LEDGER_DIGEST_METADATA_KEY)
            == Some(&persisted.observed_channels.outcome_ledger_digest)
}

fn persisted_scope_matches_event(
    persisted: &PersistedMemoryAttributionEvent,
    event: &Event,
) -> bool {
    event.metadata.get("project_id") == Some(&persisted.project_id)
        && event.metadata.get("session_id") == Some(&persisted.session_id)
        && event.metadata.get(LOGICAL_AGENT_RUN_ID_METADATA_KEY) == Some(&persisted.logical_run_id)
        && event.metadata.get(AGENT_RUN_ID_METADATA_KEY) == Some(&persisted.terminal_agent_run_id)
        && event
            .metadata
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            == Some(persisted.steer_epoch)
        && event.metadata.get("memory_utility").map(String::as_str) == Some("unknown")
        && event
            .metadata
            .get("selected_count")
            .and_then(|value| value.parse::<usize>().ok())
            == Some(persisted.attributions.len())
}

pub(super) fn decode_recall_seed(event: &Event) -> Option<MemoryRecallAttributionSeed> {
    let encoded = event.metadata.get(MEMORY_ATTRIBUTION_SOURCE_METADATA_KEY)?;
    let digest = event
        .metadata
        .get(MEMORY_ATTRIBUTION_SOURCE_DIGEST_METADATA_KEY)?;
    let seed: MemoryRecallAttributionSeed = decode_with_digest(encoded, digest)?;
    if seed.schema != MEMORY_RECALL_ATTRIBUTION_SOURCE_SCHEMA
        || seed.memories.is_empty()
        || seed.memories.len() > AGENT_MEMORY_RECALL_LIMIT
        || !is_sha256_hex(&seed.recall_projection_sha256)
        || !is_sha256_hex(&seed.task_condition_sha256)
        || !is_sha256_hex(&seed.environment_sha256)
    {
        return None;
    }
    let (project_id, session_id, logical_run_id, agent_run_id, steer_epoch) =
        strict_run_scope(&event.metadata)?;
    let task_condition = event
        .metadata
        .get("effective_prompt_objective")
        .or_else(|| event.metadata.get("prompt_objective"))
        .or_else(|| event.metadata.get("query"))
        .filter(|value| !value.trim().is_empty())?;
    let expected_environment_sha256 = sha256_json(&(
        MEMORY_ATTRIBUTION_HASH_DOMAIN,
        "recall_environment",
        project_id,
        session_id,
        logical_run_id,
        seed.recall_projection_sha256.as_str(),
    ));
    if seed.project_id != project_id
        || seed.session_id != session_id
        || seed.logical_run_id != logical_run_id
        || seed.agent_run_id != agent_run_id
        || seed.steer_epoch != steer_epoch
        || seed.task_condition_sha256 != sha256_hex(task_condition.as_bytes())
        || seed.environment_sha256 != expected_environment_sha256
        || event
            .metadata
            .get("selected_count")
            .and_then(|value| value.parse::<usize>().ok())
            != Some(seed.memories.len())
    {
        return None;
    }
    let mut ids = BTreeSet::new();
    if seed.memories.iter().any(|item| {
        !valid_id(&item.memory_id)
            || !is_sha256_hex(&item.memory_sha256)
            || !ids.insert(item.memory_id.as_str())
    }) {
        return None;
    }
    let selected_ids = event
        .metadata
        .get("memory_ids")?
        .split(',')
        .collect::<Vec<_>>();
    (selected_ids.len() == seed.memories.len()
        && selected_ids
            .iter()
            .zip(&seed.memories)
            .all(|(id, item)| *id == item.memory_id))
    .then_some(seed)
}

pub(super) fn decode_attribution_event(event: &Event) -> Option<PersistedMemoryAttributionEvent> {
    decode_with_digest(
        event.metadata.get(MEMORY_ATTRIBUTION_EVENT_METADATA_KEY)?,
        event
            .metadata
            .get(MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY)?,
    )
}

#[cfg(test)]
mod tests {
    use super::super::persistence::append_project_memory_attribution;
    use super::super::{
        encode_with_digest, insert_memory_recall_attribution_source, observed_channels_sha256,
        CompletionMemoryAttributionObservation, MemoryObservedChannels,
        MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY, MEMORY_ATTRIBUTION_EVENT_METADATA_KEY,
        MEMORY_ATTRIBUTION_EVENT_SUMMARY, MEMORY_OBSERVED_CHANNELS_SCHEMA,
        MEMORY_RECALL_EVENT_SUMMARY, MEMORY_TERMINAL_EVENT_SUMMARY,
    };
    use super::{decode_attribution_event, replay_project_memory_attribution};
    use crate::{
        desktop_prelude::{EventKind, Metadata, SqliteStore, TaskId},
        event_persistence::append_event,
        project_session_persistence::metadata_with_context,
    };
    use agent_core::{AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
    use agent_memory::{
        MemoryEvidenceKind, MemoryInfluenceKind, MemoryKind, MemoryLedger, MemoryProvenance,
        MemoryRecall, MemoryRecord, MemoryTrust, MemoryUtilitySummary,
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
                "Verify atomic memory attribution replay".to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    fn recalled_memory(memory_id: &str) -> MemoryRecall {
        MemoryRecall {
            record: MemoryRecord {
                id: memory_id.to_string(),
                fingerprint: format!("fingerprint-{memory_id}"),
                kind: MemoryKind::Evidence,
                trust: MemoryTrust::ToolVerified,
                content: format!("Evidence retained for {memory_id}"),
                importance: 80,
                provenance: MemoryProvenance {
                    project_id: "project-a".to_string(),
                    session_id: "source-session".to_string(),
                    event_id: format!("source-{memory_id}"),
                    agent_run_id: Some("source-run".to_string()),
                    sequence: 1,
                    timestamp_ms: 1,
                },
                source_event_ids: vec![format!("source-{memory_id}")],
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
    fn malformed_later_attribution_cannot_partially_apply_an_earlier_one() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-atomic-memory-attribution".to_string());
        let recall_context = run_context("attempt-a");
        let recalls = vec![recalled_memory("memory-a"), recalled_memory("memory-b")];
        let mut recall_metadata = [
            ("action".to_string(), "memory_recall".to_string()),
            ("selected_count".to_string(), recalls.len().to_string()),
            (
                "memory_ids".to_string(),
                recalls
                    .iter()
                    .map(|recall| recall.record.id.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            (
                "query".to_string(),
                "Verify atomic memory attribution replay".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        assert!(insert_memory_recall_attribution_source(
            &mut recall_metadata,
            &recall_context,
            &"1".repeat(64),
            &recalls,
        ));
        append_event(
            &mut store,
            &task_id,
            EventKind::RetrievalPerformed,
            MEMORY_RECALL_EVENT_SUMMARY,
            metadata_with_context(recall_metadata, &recall_context),
        )
        .expect("recall should append");

        let terminal_context = run_context("attempt-c");
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            MEMORY_TERMINAL_EVENT_SUMMARY,
            metadata_with_context(
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
        )
        .expect("terminal should append");
        let influence = Some(MemoryInfluenceKind::ToolObservedInRecalledRun);
        let evidence = Some(MemoryEvidenceKind::ArtifactReceipt);
        let observation = CompletionMemoryAttributionObservation {
            channels: MemoryObservedChannels {
                schema: MEMORY_OBSERVED_CHANNELS_SCHEMA.to_string(),
                action_sha256: observed_channels_sha256(
                    &[],
                    1,
                    0,
                    &"3".repeat(64),
                    &"2".repeat(64),
                    "evidence_visible",
                    influence,
                    evidence,
                ),
                goal_delta_fingerprints: Vec::new(),
                successful_tool_evidence: 1,
                verified_postcondition_evidence: 0,
                grounded_completion_digest: "3".repeat(64),
                outcome_ledger_digest: "2".repeat(64),
                grounded_completion_basis: "evidence_visible".to_string(),
                influence,
                evidence,
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
            2
        );

        let mut attribution_event = store
            .list_by_task_and_metadata(&task_id, LOGICAL_AGENT_RUN_ID_METADATA_KEY, "logical-a")
            .expect("logical events should load")
            .into_iter()
            .find(|event| event.summary == MEMORY_ATTRIBUTION_EVENT_SUMMARY)
            .expect("attribution event should exist");
        let mut persisted = decode_attribution_event(&attribution_event)
            .expect("valid attribution envelope should decode");
        persisted.attributions[1].memory_id = "missing-memory".to_string();
        let (encoded, digest) =
            encode_with_digest(&persisted).expect("tampered envelope should encode");
        attribution_event
            .metadata
            .insert(MEMORY_ATTRIBUTION_EVENT_METADATA_KEY.to_string(), encoded);
        attribution_event.metadata.insert(
            MEMORY_ATTRIBUTION_EVENT_DIGEST_METADATA_KEY.to_string(),
            digest,
        );

        let mut ledger = MemoryLedger::new("project-a");
        ledger.records = recalls.into_iter().map(|recall| recall.record).collect();
        let before = ledger.clone();
        assert_eq!(
            replay_project_memory_attribution(
                &store,
                &mut ledger,
                &attribution_event,
                "project-a",
            )
            .expect("malformed event should fail closed"),
            0
        );
        assert_eq!(ledger, before);
    }
}

#[cfg(test)]
mod expiry_tests;
