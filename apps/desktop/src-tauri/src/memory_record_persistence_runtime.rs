use crate::{
    event_persistence::append_event, runtime_constants::AGENT_MEMORY_MAX_RECORDS,
    runtime_values::phase16_task_id,
};
use agent_core::{Event, EventKind};
use agent_memory::{
    memory_content_sha256, merge_memory_records, quarantine_legacy_unverified_requirements,
    MemoryLedger, MemoryRecord,
};
use agent_storage::{SqliteStore, StorageError};
use std::collections::BTreeSet;

const MEMORY_RECORD_EVENT_SCHEMA: &str = "cindx.memory-record.v1";
const MEMORY_RECORD_EVENT_SUMMARY: &str = "Project memory record retained";
const MEMORY_SESSION_RETIREMENT_SCHEMA: &str = "cindx.memory-session-retirement.v1";
const MEMORY_SESSION_RETIREMENT_SUMMARY: &str = "Project session history retired";
const MAX_MEMORY_RECORD_EVENT_JSON_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PersistedMemoryRecordState {
    Active,
    Quarantined,
}

impl PersistedMemoryRecordState {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "retain_active",
            Self::Quarantined => "retain_quarantined",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "retain_active" => Some(Self::Active),
            "retain_quarantined" => Some(Self::Quarantined),
            _ => None,
        }
    }
}

struct EncodedMemoryRecord {
    content_sha256: String,
    json: String,
    memory_id: String,
    state: PersistedMemoryRecordState,
}

fn memory_record_session_id(project_id: &str) -> String {
    format!("project-memory-records:{project_id}")
}

fn encoded_memory_record(
    record: &MemoryRecord,
    state: PersistedMemoryRecordState,
) -> Result<EncodedMemoryRecord, StorageError> {
    if record.contains_sensitive_persisted_value() {
        return Err(StorageError::new(
            "memory record contains a sensitive value and cannot be retained",
        ));
    }
    let json = serde_json::to_string(record).map_err(|error| {
        StorageError::new(format!("memory record serialization failed: {error}"))
    })?;
    if json.len() > MAX_MEMORY_RECORD_EVENT_JSON_BYTES {
        return Err(StorageError::new("memory record is too large to retain"));
    }
    Ok(EncodedMemoryRecord {
        content_sha256: memory_content_sha256(&json),
        json,
        memory_id: record.id.clone(),
        state,
    })
}

fn encoded_memory_record_key(record: &EncodedMemoryRecord) -> String {
    format!(
        "{}:{}:{}",
        record.state.label(),
        record.memory_id,
        record.content_sha256
    )
}

fn decode_persisted_memory_record(
    event: &Event,
    project_id: &str,
) -> Option<(PersistedMemoryRecordState, MemoryRecord)> {
    let expected_session_id = memory_record_session_id(project_id);
    if event.kind != EventKind::RetrievalPerformed
        || event.summary != MEMORY_RECORD_EVENT_SUMMARY
        || event
            .metadata
            .get("memory_record_schema")
            .map(String::as_str)
            != Some(MEMORY_RECORD_EVENT_SCHEMA)
        || event.metadata.get("project_id").map(String::as_str) != Some(project_id)
        || event.metadata.get("session_id").map(String::as_str)
            != Some(expected_session_id.as_str())
        || event.metadata.get("actor").map(String::as_str) != Some("runtime")
        || event.metadata.get("internal").map(String::as_str) != Some("true")
    {
        return None;
    }
    let state = event
        .metadata
        .get("memory_record_action")
        .and_then(|value| PersistedMemoryRecordState::parse(value))?;
    let json = event
        .metadata
        .get("memory_record_json")
        .filter(|json| json.len() <= MAX_MEMORY_RECORD_EVENT_JSON_BYTES)?;
    let content_sha256 = memory_content_sha256(json);
    if event
        .metadata
        .get("memory_record_sha256")
        .map(String::as_str)
        != Some(content_sha256.as_str())
    {
        return None;
    }
    let record = serde_json::from_str::<MemoryRecord>(json).ok()?;
    if serde_json::to_string(&record).ok().as_deref() != Some(json.as_str())
        || record.contains_sensitive_persisted_value()
        || event.metadata.get("memory_id").map(String::as_str) != Some(record.id.as_str())
        || record.provenance.project_id != project_id
        || record.provenance.sequence == 0
        || record.provenance.sequence >= event.sequence
        || !record
            .source_event_ids
            .contains(&record.provenance.event_id)
        || !record
            .source_session_ids
            .contains(&record.provenance.session_id)
        || match state {
            PersistedMemoryRecordState::Active => !record.is_recall_eligible(),
            PersistedMemoryRecordState::Quarantined => record.is_recall_eligible(),
        }
    {
        return None;
    }
    Some((state, record))
}

fn persisted_memory_record_key(event: &Event, project_id: &str) -> Option<String> {
    let (state, record) = decode_persisted_memory_record(event, project_id)?;
    Some(format!(
        "{}:{}:{}",
        state.label(),
        record.id,
        event.metadata.get("memory_record_sha256")?
    ))
}

fn persist_memory_records(
    store: &mut SqliteStore,
    project_id: &str,
    records: impl IntoIterator<Item = (MemoryRecord, PersistedMemoryRecordState)>,
) -> Result<usize, StorageError> {
    let records = records
        .into_iter()
        .map(|(record, state)| encoded_memory_record(&record, state))
        .collect::<Result<Vec<_>, _>>()?;
    persist_encoded_memory_records(store, project_id, records)
}

fn persist_encoded_memory_records(
    store: &mut SqliteStore,
    project_id: &str,
    records: Vec<EncodedMemoryRecord>,
) -> Result<usize, StorageError> {
    if records.is_empty() {
        return Ok(0);
    }
    let mut existing = store
        .list_by_task_and_metadata(&phase16_task_id(), "project_id", project_id)?
        .iter()
        .filter_map(|event| persisted_memory_record_key(event, project_id))
        .collect::<BTreeSet<_>>();
    let mut inserted = 0;
    for record in records {
        let key = encoded_memory_record_key(&record);
        if !existing.insert(key) {
            continue;
        }
        append_event(
            store,
            &phase16_task_id(),
            EventKind::RetrievalPerformed,
            MEMORY_RECORD_EVENT_SUMMARY,
            [
                (
                    "memory_record_schema".to_string(),
                    MEMORY_RECORD_EVENT_SCHEMA.to_string(),
                ),
                (
                    "memory_record_action".to_string(),
                    record.state.label().to_string(),
                ),
                ("memory_id".to_string(), record.memory_id),
                ("memory_record_sha256".to_string(), record.content_sha256),
                ("memory_record_json".to_string(), record.json),
                ("project_id".to_string(), project_id.to_string()),
                (
                    "session_id".to_string(),
                    memory_record_session_id(project_id),
                ),
                ("actor".to_string(), "runtime".to_string()),
                ("internal".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
        )?;
        inserted += 1;
    }
    Ok(inserted)
}

pub(crate) fn persist_quarantined_memory_records(
    store: &mut SqliteStore,
    ledger: &MemoryLedger,
) -> Result<usize, StorageError> {
    persist_memory_records(
        store,
        &ledger.project_id,
        ledger
            .quarantined_records
            .iter()
            .map(|item| (item.record.clone(), PersistedMemoryRecordState::Quarantined)),
    )
}

pub(crate) fn retain_memory_records_for_deleted_sessions(
    store: &mut SqliteStore,
    ledger: &MemoryLedger,
    session_ids: &[String],
) -> Result<usize, StorageError> {
    let sessions = session_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let active = ledger.records.iter().filter(|record| {
        record
            .source_session_ids
            .iter()
            .any(|session_id| sessions.contains(session_id.as_str()))
    });
    let quarantined = ledger.quarantined_records.iter().filter(|item| {
        item.record
            .source_session_ids
            .iter()
            .any(|session_id| sessions.contains(session_id.as_str()))
    });
    let mut encoded = Vec::new();
    for (record, state) in active
        .cloned()
        .map(|record| (record, PersistedMemoryRecordState::Active))
        .chain(
            quarantined.map(|item| (item.record.clone(), PersistedMemoryRecordState::Quarantined)),
        )
    {
        match encoded_memory_record(&record, state) {
            Ok(record) => encoded.push(record),
            Err(error) => {
                eprintln!("skipping unsafe memory snapshot during session retirement: {error}")
            }
        }
    }
    persist_encoded_memory_records(store, &ledger.project_id, encoded)
}

pub(crate) fn persist_memory_session_retirements(
    store: &mut SqliteStore,
    project_id: &str,
    session_ids: &[String],
) -> Result<usize, StorageError> {
    let mut existing = store
        .list_by_task_and_metadata(&phase16_task_id(), "project_id", project_id)?
        .into_iter()
        .filter(|event| {
            event.kind == EventKind::RetrievalPerformed
                && event.summary == MEMORY_SESSION_RETIREMENT_SUMMARY
                && event
                    .metadata
                    .get("memory_session_retirement_schema")
                    .map(String::as_str)
                    == Some(MEMORY_SESSION_RETIREMENT_SCHEMA)
                && event.metadata.get("actor").map(String::as_str) == Some("runtime")
                && event.metadata.get("internal").map(String::as_str) == Some("true")
                && event.metadata.get("session_id").map(String::as_str)
                    == Some(memory_record_session_id(project_id).as_str())
        })
        .filter_map(|event| event.metadata.get("retired_session_id").cloned())
        .collect::<BTreeSet<_>>();
    let mut inserted = 0;
    for retired_session_id in session_ids {
        if retired_session_id.is_empty() || retired_session_id.len() > 256 {
            return Err(StorageError::new("invalid retired memory session identity"));
        }
        if !existing.insert(retired_session_id.clone()) {
            continue;
        }
        append_event(
            store,
            &phase16_task_id(),
            EventKind::RetrievalPerformed,
            MEMORY_SESSION_RETIREMENT_SUMMARY,
            [
                (
                    "memory_session_retirement_schema".to_string(),
                    MEMORY_SESSION_RETIREMENT_SCHEMA.to_string(),
                ),
                ("retired_session_id".to_string(), retired_session_id.clone()),
                ("project_id".to_string(), project_id.to_string()),
                (
                    "session_id".to_string(),
                    memory_record_session_id(project_id),
                ),
                ("actor".to_string(), "runtime".to_string()),
                ("internal".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
        )?;
        inserted += 1;
    }
    Ok(inserted)
}

pub(crate) fn replay_project_memory_record(
    ledger: &mut MemoryLedger,
    event: &Event,
    project_id: &str,
) {
    let Some((state, record)) = decode_persisted_memory_record(event, project_id) else {
        return;
    };
    match state {
        PersistedMemoryRecordState::Active if record.is_recall_eligible() => {
            merge_memory_records(ledger, [record], AGENT_MEMORY_MAX_RECORDS);
        }
        PersistedMemoryRecordState::Quarantined if !record.is_recall_eligible() => {
            quarantine_legacy_unverified_requirements(ledger, [record], AGENT_MEMORY_MAX_RECORDS);
            ledger.quarantine_authoritative = true;
        }
        _ => {}
    }
}
