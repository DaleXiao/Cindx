use crate::{
    configuration_models::ProviderConfig,
    memory_vector_generation_runtime::{
        memory_vector_project_key, memory_vector_projection_sha256, open_memory_vector_snapshot,
    },
    memory_vector_refresh_generation::{
        prepare_project_memory_vector_refresh, publish_prepared_memory_vector_refresh,
    },
    persistence_runtime::memory_lancedb_root_for,
    runtime_constants::{MEMORY_VECTOR_MANIFEST_SCHEMA, MEMORY_VECTOR_REFRESH_INFLIGHT},
};
use agent_harness::ExclusiveKeyRegistry;
use agent_memory::{MemoryLedger, MEMORY_LEDGER_SCHEMA};
use agent_rag::lancedb_index_exists;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

#[derive(Clone)]
struct PendingMemoryVectorRefresh {
    workspace_root: PathBuf,
    config: ProviderConfig,
    ledger: MemoryLedger,
}

static MEMORY_VECTOR_REFRESH_PENDING: OnceLock<
    Mutex<BTreeMap<String, PendingMemoryVectorRefresh>>,
> = OnceLock::new();
static MEMORY_VECTOR_REFRESH_GATES: OnceLock<Mutex<BTreeMap<String, Weak<Mutex<()>>>>> =
    OnceLock::new();
static MEMORY_VECTOR_REFRESH_LATEST_REVISIONS: OnceLock<Mutex<BTreeMap<String, u64>>> =
    OnceLock::new();
static DELETED_MEMORY_VECTOR_PROJECTS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn memory_vector_refresh_gate(key: &str) -> Result<Arc<Mutex<()>>, String> {
    memory_vector_refresh_gate_inner(key)
}

fn memory_vector_refresh_gate_inner(key: &str) -> Result<Arc<Mutex<()>>, String> {
    let mut gates = MEMORY_VECTOR_REFRESH_GATES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| format!("memory vector refresh gate registry poisoned: {error}"))?;
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(key).and_then(Weak::upgrade) {
        return Ok(gate);
    }
    let gate = Arc::new(Mutex::new(()));
    gates.insert(key.to_string(), Arc::downgrade(&gate));
    Ok(gate)
}

#[cfg(test)]
pub(crate) fn memory_vector_project_is_deleted(key: &str) -> bool {
    memory_vector_project_is_deleted_inner(key)
}

fn memory_vector_project_is_deleted_inner(key: &str) -> bool {
    DELETED_MEMORY_VECTOR_PROJECTS
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .map_or(true, |projects| projects.contains(key))
}

pub(crate) fn delete_project_memory_vector_index(
    workspace_root: &Path,
    project_id: &str,
) -> Result<(), String> {
    let key = memory_vector_project_key(workspace_root, project_id);
    DELETED_MEMORY_VECTOR_PROJECTS
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .map_err(|error| format!("deleted memory vector registry poisoned: {error}"))?
        .insert(key.clone());
    MEMORY_VECTOR_REFRESH_PENDING
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| format!("memory vector refresh queue poisoned: {error}"))?
        .remove(&key);
    MEMORY_VECTOR_REFRESH_LATEST_REVISIONS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| format!("memory vector revision registry poisoned: {error}"))?
        .remove(&key);

    let gate = memory_vector_refresh_gate_inner(&key)?;
    let _guard = gate
        .lock()
        .map_err(|error| format!("memory vector refresh gate poisoned: {error}"))?;
    remove_memory_vector_root(workspace_root, project_id, "remove project memory vectors")?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn invalidate_stale_project_memory_vector_index(
    workspace_root: &Path,
    ledger: &MemoryLedger,
) -> Result<bool, String> {
    reset_project_memory_vector_index(workspace_root, ledger, false)
}

pub(crate) fn purge_project_memory_vector_history(
    workspace_root: &Path,
    ledger: &MemoryLedger,
) -> Result<bool, String> {
    reset_project_memory_vector_index(workspace_root, ledger, true)
}

fn reset_project_memory_vector_index(
    workspace_root: &Path,
    ledger: &MemoryLedger,
    purge_history: bool,
) -> Result<bool, String> {
    if ledger.schema != MEMORY_LEDGER_SCHEMA {
        return Err("cannot refresh vectors from an untrusted memory ledger".to_string());
    }
    let key = memory_vector_project_key(workspace_root, &ledger.project_id);
    {
        let mut latest = MEMORY_VECTOR_REFRESH_LATEST_REVISIONS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|error| format!("memory vector revision registry poisoned: {error}"))?;
        let current = latest.entry(key.clone()).or_default();
        *current = (*current).max(ledger.revision);
    }
    {
        let mut pending = MEMORY_VECTOR_REFRESH_PENDING
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|error| format!("memory vector refresh queue poisoned: {error}"))?;
        if pending
            .get(&key)
            .is_some_and(|refresh| refresh.ledger.revision <= ledger.revision)
        {
            pending.remove(&key);
        }
    }

    let gate = memory_vector_refresh_gate_inner(&key)?;
    let _guard = gate
        .lock()
        .map_err(|error| format!("memory vector refresh gate poisoned: {error}"))?;
    if !memory_vector_refresh_revision_is_current(&key, ledger.revision) {
        return Ok(false);
    }
    if !purge_history {
        let expected_projection = memory_vector_projection_sha256(ledger);
        let expected_count = ledger
            .records
            .iter()
            .filter(|record| ledger.record_is_active_for_recall(record))
            .count();
        let snapshot = open_memory_vector_snapshot(workspace_root, &ledger.project_id)?;
        let is_current = snapshot.manifest.as_ref().is_some_and(|manifest| {
            manifest.schema == MEMORY_VECTOR_MANIFEST_SCHEMA
                && snapshot
                    .generation_id
                    .as_deref()
                    .is_none_or(|generation| generation == manifest.generation_id)
                && manifest.projection_sha256 == expected_projection
                && manifest.record_count == expected_count
                && (expected_count == 0 || lancedb_index_exists(&snapshot.database_path))
        });
        drop(snapshot);
        if is_current {
            return Ok(false);
        }
    }

    let vector_root = memory_lancedb_root_for(workspace_root, &ledger.project_id);
    let existed = vector_root.exists();
    remove_memory_vector_root(
        workspace_root,
        &ledger.project_id,
        "invalidate stale project memory vectors",
    )?;
    Ok(existed)
}

fn remove_memory_vector_root(
    workspace_root: &Path,
    project_id: &str,
    action: &str,
) -> Result<(), String> {
    let vector_root = memory_lancedb_root_for(workspace_root, project_id);
    if let Err(error) = fs::remove_dir_all(&vector_root) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!(
                "failed to {action} at {}: {error}",
                vector_root.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn refresh_project_memory_vector_index(
    workspace_root: &Path,
    config: &ProviderConfig,
    ledger: &MemoryLedger,
) -> Result<Option<String>, String> {
    let key = memory_vector_project_key(workspace_root, &ledger.project_id);
    let inflight = MEMORY_VECTOR_REFRESH_INFLIGHT
        .get_or_init(|| ExclusiveKeyRegistry::new("memory vector refresh inflight"));
    let Some(_inflight_lease) = inflight
        .try_acquire(key)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Some(prepared) = prepare_project_memory_vector_refresh(workspace_root, config, ledger)?
    else {
        return Ok(None);
    };
    let gate = memory_vector_refresh_gate_inner(&memory_vector_project_key(
        workspace_root,
        &ledger.project_id,
    ))?;
    let _guard = gate
        .lock()
        .map_err(|error| format!("memory vector refresh gate poisoned: {error}"))?;
    publish_prepared_memory_vector_refresh(workspace_root, ledger, prepared)
}

fn memory_vector_refresh_revision_is_current(key: &str, revision: u64) -> bool {
    MEMORY_VECTOR_REFRESH_LATEST_REVISIONS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .is_ok_and(|latest| latest.get(key).is_none_or(|current| revision >= *current))
}

pub(crate) fn schedule_project_memory_vector_refresh(
    workspace_root: PathBuf,
    config: ProviderConfig,
    ledger: MemoryLedger,
) {
    let key = memory_vector_project_key(&workspace_root, &ledger.project_id);
    if memory_vector_project_is_deleted_inner(&key) {
        return;
    }
    let latest_revisions =
        MEMORY_VECTOR_REFRESH_LATEST_REVISIONS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let Ok(mut latest_revisions) = latest_revisions.lock() else {
        return;
    };
    if latest_revisions
        .get(&key)
        .is_some_and(|revision| ledger.revision < *revision)
    {
        return;
    }
    latest_revisions.insert(key.clone(), ledger.revision);
    drop(latest_revisions);
    let pending = MEMORY_VECTOR_REFRESH_PENDING.get_or_init(|| Mutex::new(BTreeMap::new()));
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    pending.insert(
        key.clone(),
        PendingMemoryVectorRefresh {
            workspace_root,
            config,
            ledger,
        },
    );
    drop(pending);
    let inflight = MEMORY_VECTOR_REFRESH_INFLIGHT
        .get_or_init(|| ExclusiveKeyRegistry::new("memory vector refresh inflight"));
    let Ok(Some(inflight_lease)) = inflight.try_acquire(key.clone()) else {
        return;
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut inflight_lease = Some(inflight_lease);
        loop {
            let pending = MEMORY_VECTOR_REFRESH_PENDING.get_or_init(|| Mutex::new(BTreeMap::new()));
            let Ok(mut pending) = pending.lock() else {
                break;
            };
            let Some(refresh) = pending.remove(&key) else {
                drop(inflight_lease.take());
                drop(pending);
                break;
            };
            drop(pending);
            if memory_vector_project_is_deleted_inner(&key)
                || !memory_vector_refresh_revision_is_current(&key, refresh.ledger.revision)
            {
                continue;
            }
            let prepared = match prepare_project_memory_vector_refresh(
                &refresh.workspace_root,
                &refresh.config,
                &refresh.ledger,
            ) {
                Ok(Some(prepared)) => prepared,
                Ok(None) => continue,
                Err(error) => {
                    eprintln!("project memory vector refresh unavailable: {error}");
                    continue;
                }
            };
            let gate = match memory_vector_refresh_gate_inner(&key) {
                Ok(gate) => gate,
                Err(error) => {
                    eprintln!("project memory vector refresh gate unavailable: {error}");
                    break;
                }
            };
            let Ok(_guard) = gate.lock() else {
                eprintln!("project memory vector refresh gate poisoned");
                break;
            };
            if memory_vector_project_is_deleted_inner(&key)
                || !memory_vector_refresh_revision_is_current(&key, refresh.ledger.revision)
            {
                continue;
            }
            match publish_prepared_memory_vector_refresh(
                &refresh.workspace_root,
                &refresh.ledger,
                prepared,
            ) {
                Ok(Some(error)) => {
                    eprintln!(
                        "project memory cloud embedding unavailable; using local vectors: {error}"
                    )
                }
                Ok(None) => {}
                Err(error) => eprintln!("project memory vector refresh unavailable: {error}"),
            }
        }
    });
}
