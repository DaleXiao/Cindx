use crate::{
    memory_management_runtime::replay_project_memory_management,
    memory_measurement_runtime::replay_project_memory_measurement,
    memory_migration_runtime::{
        load_cached_memory_for_migration, memory_ledger_is_intrinsically_valid,
        seed_legacy_memory_quarantine,
    },
    memory_record_persistence_runtime::replay_project_memory_record,
    runtime_constants::{AGENT_MEMORY_MAX_RECORDS, AGENT_MEMORY_READ_MODEL_NAMESPACE},
    runtime_values::phase16_task_id,
};
use agent_application::AgentRunEvent;
use agent_core::{
    AgentRunLineage, Event, EventKind, EVENT_TYPE_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use agent_memory::{
    extract_durable_memories, is_memory_user_confirmation_event, merge_memory_records, MemoryLedger,
};
use agent_storage::{SqliteStore, StorageError};
use std::collections::BTreeMap;

pub(crate) mod attribution;
use attribution::replay_project_memory_attribution;

mod run_identity_projection;

use run_identity_projection::{
    group_memory_run_events, memory_run_scope, select_memory_run_events, BridgedMemoryRunEvents,
    MemoryRunScope,
};

pub(crate) fn is_memory_checkpoint_event(event: &Event) -> bool {
    AgentRunEvent::from_event(event)
        .is_some_and(|event| event.status().is_terminal() || event == AgentRunEvent::Paused)
        || (event.summary == "Semantic memory candidates accepted"
            && !event.metadata.contains_key(EVENT_TYPE_METADATA_KEY))
        || is_memory_user_confirmation_event(event)
}

pub(crate) fn memory_events_for_terminal_steer_epoch(mut events: Vec<Event>) -> Vec<Event> {
    let terminal_epoch = events.iter().rev().find_map(|event| {
        AgentRunEvent::from_event(event)
            .is_some_and(|run_event| {
                run_event.status().is_terminal() || run_event == AgentRunEvent::Paused
            })
            .then(|| {
                event
                    .metadata
                    .get("steer_epoch")
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .flatten()
    });
    let Some(terminal_epoch) = terminal_epoch else {
        return events;
    };
    events.retain(|event| {
        let event_epoch = event
            .metadata
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let accepted_user_intent = event.kind == EventKind::MessageAdded
            && event.metadata.get("role").map(String::as_str) == Some("user")
            && event.metadata.get("internal").map(String::as_str) != Some("true")
            && event_epoch <= terminal_epoch;
        accepted_user_intent || event_epoch == terminal_epoch
    });
    events
}

pub(crate) fn save_project_memory_ledger(
    store: &mut SqliteStore,
    ledger: &MemoryLedger,
) -> Result<(), StorageError> {
    let payload = serde_json::to_string(ledger)
        .map_err(|error| StorageError::new(format!("memory serialization failed: {error}")))?;
    store.save_read_model(
        AGENT_MEMORY_READ_MODEL_NAMESPACE,
        &ledger.project_id,
        ledger.revision,
        &payload,
    )?;
    Ok(())
}

pub(crate) struct LoadedProjectMemoryLedger {
    pub(crate) ledger: MemoryLedger,
    pub(crate) needs_persist: bool,
    pub(crate) quarantine_needs_persistence: bool,
    pub(crate) vector_reset_required: bool,
}

pub(crate) fn load_project_memory_ledger_inner(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<LoadedProjectMemoryLedger, StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision_by_metadata(&task_id, "project_id", project_id)?;
    let migration = load_cached_memory_for_migration(
        store,
        project_id,
        revision.latest_sequence,
        revision.event_count,
    )?;
    let mut vector_reset_required = migration.vector_reset_required;
    let mut rebuilding = migration.ledger.is_none();
    let mut changed = rebuilding || migration.migrated;
    let mut ledger = migration
        .ledger
        .unwrap_or_else(|| MemoryLedger::new(project_id));
    vector_reset_required |= ledger.vector_history_reset_required;
    if rebuilding {
        seed_legacy_memory_quarantine(&mut ledger, &migration.quarantine_candidates);
    }
    let mut delta = store.list_by_task_and_metadata_after(
        &task_id,
        "project_id",
        project_id,
        ledger.revision,
    )?;
    changed |= !delta.is_empty();
    if ledger.event_count.saturating_add(delta.len() as u64) != revision.event_count {
        rebuilding = true;
        changed = true;
        vector_reset_required = true;
        ledger = MemoryLedger::new(project_id);
        seed_legacy_memory_quarantine(&mut ledger, &migration.quarantine_candidates);
        delta = store.list_by_task_and_metadata_after(&task_id, "project_id", project_id, 0)?;
    }

    let has_legacy_checkpoint = delta.iter().any(|event| {
        event.metadata.get("project_id").map(String::as_str) == Some(project_id)
            && is_memory_checkpoint_event(event)
            && !event
                .metadata
                .contains_key(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
    });
    let loaded_lineage_events;
    let lineage_events = if rebuilding || !has_legacy_checkpoint {
        delta.as_slice()
    } else {
        loaded_lineage_events =
            store.list_by_task_and_metadata(&task_id, "project_id", project_id)?;
        loaded_lineage_events.as_slice()
    };
    let lineage = AgentRunLineage::from_events(lineage_events).ok();
    let scoped_run_events = if rebuilding || has_legacy_checkpoint {
        Some(group_memory_run_events(
            lineage_events,
            project_id,
            lineage.as_ref(),
        ))
    } else {
        None
    };
    let latest_checkpoints = delta
        .iter()
        .filter(|event| {
            event.metadata.get("project_id").map(String::as_str) == Some(project_id)
                && is_memory_checkpoint_event(event)
        })
        .filter_map(|event| {
            memory_run_scope(event, lineage.as_ref()).map(|scope| (scope, event.sequence))
        })
        .fold(
            BTreeMap::<MemoryRunScope, u64>::new(),
            |mut checkpoints, (scope, sequence)| {
                checkpoints
                    .entry(scope)
                    .and_modify(|latest| *latest = (*latest).max(sequence))
                    .or_insert(sequence);
                checkpoints
            },
        );
    let mut bridged_run_events = BridgedMemoryRunEvents::new();
    for event in &delta {
        if event.metadata.get("project_id").map(String::as_str) != Some(project_id) {
            continue;
        }
        if is_memory_checkpoint_event(event) {
            let Some(scope) = memory_run_scope(event, lineage.as_ref()) else {
                continue;
            };
            if latest_checkpoints.get(&scope).copied() == Some(event.sequence) {
                let events = select_memory_run_events(
                    store,
                    &task_id,
                    project_id,
                    &scope,
                    scoped_run_events.as_ref(),
                    &mut bridged_run_events,
                )?;
                let events = events
                    .into_iter()
                    .filter(|candidate| candidate.sequence <= event.sequence)
                    .collect::<Vec<_>>();
                let events = memory_events_for_terminal_steer_epoch(events);
                if let Some(session_id) = events
                    .iter()
                    .find_map(|candidate| candidate.metadata.get("session_id"))
                {
                    merge_memory_records(
                        &mut ledger,
                        extract_durable_memories(&events, project_id, session_id),
                        AGENT_MEMORY_MAX_RECORDS,
                    );
                }
            }
        }
        replay_project_memory_record(&mut ledger, event, project_id);
        replay_project_memory_management(&mut ledger, event, project_id);
        replay_project_memory_measurement(&mut ledger, event, project_id);
        replay_project_memory_attribution(store, &mut ledger, event, project_id)?;
    }
    let quarantine_needs_persistence =
        !ledger.quarantine_authoritative && !ledger.quarantined_records.is_empty();
    if ledger.quarantined_records.is_empty() && !ledger.quarantine_authoritative {
        ledger.quarantine_authoritative = true;
        changed = true;
    }
    ledger.revision = revision.latest_sequence;
    ledger.event_count = revision.event_count;
    ledger.vector_history_reset_required = vector_reset_required;
    Ok(LoadedProjectMemoryLedger {
        ledger,
        needs_persist: changed,
        quarantine_needs_persistence,
        vector_reset_required,
    })
}

pub(crate) fn persist_project_memory_snapshot_if_current(
    store: &mut SqliteStore,
    ledger: &MemoryLedger,
) -> Result<bool, StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision_by_metadata(&task_id, "project_id", &ledger.project_id)?;
    if revision.latest_sequence != ledger.revision
        || revision.event_count != ledger.event_count
        || !memory_ledger_is_intrinsically_valid(&ledger.project_id, ledger)
    {
        return Ok(false);
    }
    save_project_memory_ledger(store, ledger)?;
    Ok(true)
}
