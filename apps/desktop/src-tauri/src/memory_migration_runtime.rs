use crate::runtime_constants::{AGENT_MEMORY_MAX_RECORDS, AGENT_MEMORY_READ_MODEL_NAMESPACE};
use agent_memory::{
    quarantine_legacy_unverified_requirements, MemoryLedger, MemoryRecord, MEMORY_LEDGER_SCHEMA,
};
use agent_storage::{SqliteStore, StorageError};

const LEGACY_UNVERIFIED_MEMORY_LEDGER_SCHEMA: &str = "cindx.memory-ledger.v4";
const PREVIOUS_TRUSTED_MEMORY_LEDGER_SCHEMA: &str = "cindx.memory-ledger.v5";

pub(crate) struct CachedMemoryMigration {
    pub(crate) ledger: Option<MemoryLedger>,
    pub(crate) quarantine_candidates: Vec<MemoryRecord>,
    pub(crate) migrated: bool,
    pub(crate) vector_reset_required: bool,
}

pub(crate) fn load_cached_memory_for_migration(
    store: &SqliteStore,
    project_id: &str,
    latest_sequence: u64,
    event_count: u64,
) -> Result<CachedMemoryMigration, StorageError> {
    let stored = store.load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, project_id)?;
    let cached_was_present = stored.is_some();
    let cached = stored.and_then(|stored| {
        serde_json::from_str::<MemoryLedger>(&stored.payload)
            .ok()
            .map(|ledger| (stored.revision, ledger))
            .filter(|(stored_revision, ledger)| {
                ledger.project_id == project_id && ledger.revision == *stored_revision
            })
    });
    let mut migrated = false;
    let mut quarantine_candidates = Vec::new();
    let ledger = cached.and_then(|(_, mut ledger)| {
        if ledger.revision <= latest_sequence
            && ledger.event_count <= event_count
            && memory_ledger_is_intrinsically_valid(project_id, &ledger)
        {
            match ledger.schema.as_str() {
                MEMORY_LEDGER_SCHEMA => return Some(ledger),
                PREVIOUS_TRUSTED_MEMORY_LEDGER_SCHEMA => {
                    ledger.schema = MEMORY_LEDGER_SCHEMA.to_string();
                    migrated = true;
                    return Some(ledger);
                }
                _ => {}
            }
        }
        quarantine_candidates = legacy_quarantine_candidates(&ledger, latest_sequence);
        None
    });
    let vector_reset_required = cached_was_present && ledger.is_none();
    Ok(CachedMemoryMigration {
        ledger,
        quarantine_candidates,
        migrated,
        vector_reset_required,
    })
}

pub(crate) fn seed_legacy_memory_quarantine(
    ledger: &mut MemoryLedger,
    candidates: &[MemoryRecord],
) {
    quarantine_legacy_unverified_requirements(
        ledger,
        candidates.iter().cloned(),
        AGENT_MEMORY_MAX_RECORDS,
    );
}

pub(crate) fn memory_ledger_is_intrinsically_valid(
    project_id: &str,
    ledger: &MemoryLedger,
) -> bool {
    ledger.records.iter().all(|record| {
        memory_record_has_valid_envelope(project_id, ledger.revision, record)
            && !record.contains_sensitive_persisted_value()
            && record.is_recall_eligible()
    }) && ledger.quarantined_records.iter().all(|item| {
        !item.reason.trim().is_empty()
            && memory_record_has_valid_envelope(project_id, ledger.revision, &item.record)
            && !item.record.contains_sensitive_persisted_value()
            && !item.record.is_recall_eligible()
    }) && ledger.controls.iter().all(|(memory_id, control)| {
        !memory_id.is_empty()
            && memory_id.len() <= 256
            && control.revision > 0
            && control.revision <= ledger.revision
    })
}

fn memory_record_has_valid_envelope(
    project_id: &str,
    ledger_revision: u64,
    record: &MemoryRecord,
) -> bool {
    record.provenance.project_id == project_id
        && record.provenance.sequence > 0
        && record.provenance.sequence <= ledger_revision
        && record
            .source_event_ids
            .contains(&record.provenance.event_id)
        && record
            .source_session_ids
            .contains(&record.provenance.session_id)
}

fn legacy_quarantine_candidates(ledger: &MemoryLedger, latest_sequence: u64) -> Vec<MemoryRecord> {
    let mut candidates = ledger
        .quarantined_records
        .iter()
        .map(|item| item.record.clone())
        .collect::<Vec<_>>();
    if ledger.schema == LEGACY_UNVERIFIED_MEMORY_LEDGER_SCHEMA {
        candidates.extend(ledger.records.iter().cloned());
    }
    candidates.retain(|record| {
        memory_record_has_valid_envelope(&ledger.project_id, latest_sequence, record)
            && !record.contains_sensitive_persisted_value()
    });
    candidates
}
