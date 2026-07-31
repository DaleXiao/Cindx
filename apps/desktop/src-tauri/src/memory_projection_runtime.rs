use crate::{
    memory_measurement_runtime::replay_project_memory_measurement,
    run_lifecycle::AgentRunEvent,
    runtime_constants::{AGENT_MEMORY_MAX_RECORDS, AGENT_MEMORY_READ_MODEL_NAMESPACE},
    runtime_values::phase16_task_id,
};
use agent_core::{Event, EventKind, EVENT_TYPE_METADATA_KEY};
use agent_memory::{
    extract_durable_memories, merge_memory_records, MemoryLedger, MEMORY_LEDGER_SCHEMA,
};
use agent_storage::{SqliteStore, StorageError};
use std::collections::BTreeMap;

pub(crate) fn is_memory_checkpoint_event(event: &Event) -> bool {
    AgentRunEvent::from_event(event)
        .is_some_and(|event| event.status().is_terminal() || event == AgentRunEvent::Paused)
        || (event.summary == "Semantic memory candidates accepted"
            && !event.metadata.contains_key(EVENT_TYPE_METADATA_KEY))
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
}

pub(crate) fn load_project_memory_ledger_inner(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<LoadedProjectMemoryLedger, StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision_by_metadata(&task_id, "project_id", project_id)?;
    let stored = store
        .load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, project_id)?
        .and_then(|stored| {
            serde_json::from_str::<MemoryLedger>(&stored.payload)
                .ok()
                .filter(|ledger| {
                    ledger.schema == MEMORY_LEDGER_SCHEMA
                        && ledger.project_id == project_id
                        && ledger.revision == stored.revision
                        && ledger.revision <= revision.latest_sequence
                        && ledger.event_count <= revision.event_count
                })
        });
    let stored = stored.filter(|ledger| memory_ledger_is_intrinsically_valid(project_id, ledger));
    let mut rebuilding = stored.is_none();
    let mut changed = rebuilding;
    let mut ledger = stored.unwrap_or_else(|| MemoryLedger::new(project_id));
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
        ledger = MemoryLedger::new(project_id);
        delta = store.list_by_task_and_metadata_after(&task_id, "project_id", project_id, 0)?;
    }

    let rebuilding_runs = if rebuilding {
        let mut runs = BTreeMap::<String, Vec<Event>>::new();
        for event in &delta {
            if event.metadata.get("project_id").map(String::as_str) != Some(project_id) {
                continue;
            }
            if let Some(run_id) = event.metadata.get("agent_run_id") {
                runs.entry(run_id.clone()).or_default().push(event.clone());
            }
        }
        Some(runs)
    } else {
        None
    };
    for event in &delta {
        if event.metadata.get("project_id").map(String::as_str) != Some(project_id) {
            continue;
        }
        if is_memory_checkpoint_event(event) {
            let Some(run_id) = event
                .metadata
                .get("agent_run_id")
                .filter(|run_id| !run_id.is_empty())
            else {
                continue;
            };
            let events = match rebuilding_runs.as_ref() {
                Some(runs) => runs.get(run_id).cloned().unwrap_or_default(),
                None => store.list_by_task_and_metadata(&task_id, "agent_run_id", run_id)?,
            }
            .into_iter()
            .filter(|candidate| candidate.sequence <= event.sequence)
            .filter(|candidate| {
                candidate.metadata.get("project_id").map(String::as_str) == Some(project_id)
            })
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
        replay_project_memory_measurement(&mut ledger, event, project_id);
    }
    ledger.revision = revision.latest_sequence;
    ledger.event_count = revision.event_count;
    Ok(LoadedProjectMemoryLedger {
        ledger,
        needs_persist: changed,
    })
}

fn memory_ledger_is_intrinsically_valid(project_id: &str, ledger: &MemoryLedger) -> bool {
    // v5 records are created only after extraction verifies an authoritative Event source.
    // Cached reads re-check the project-bound evidence envelope without adding per-turn Event
    // lookups; older schemas are rebuilt from the authoritative event log.
    ledger.records.iter().all(|record| {
        record.provenance.project_id == project_id
            && record.provenance.sequence <= ledger.revision
            && record
                .source_event_ids
                .contains(&record.provenance.event_id)
            && record
                .source_session_ids
                .contains(&record.provenance.session_id)
            && record.is_recall_eligible()
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
