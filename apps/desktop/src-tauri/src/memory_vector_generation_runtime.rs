use crate::desktop_prelude::*;
use crate::{
    configuration_models::ProviderConfig,
    event_projection::write_private_file_atomically,
    persistence_runtime::{
        memory_lancedb_database_path_for, memory_lancedb_manifest_path_for, memory_lancedb_root_for,
    },
    runtime_constants::{MEMORY_VECTOR_FALLBACK_RETRY_MS, MEMORY_VECTOR_MANIFEST_SCHEMA},
    runtime_values::unique_id,
};
use std::sync::{OnceLock, RwLock, Weak};

const MEMORY_VECTOR_RETAINED_PREVIOUS_GENERATIONS: usize = 2;

static MEMORY_VECTOR_PROJECT_LOCKS: OnceLock<Mutex<BTreeMap<String, Weak<RwLock<()>>>>> =
    OnceLock::new();
static MEMORY_VECTOR_GENERATION_LEASES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<()>>>> =
    OnceLock::new();

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryVectorManifest {
    pub(crate) schema: String,
    #[serde(default)]
    pub(crate) generation_id: String,
    pub(crate) projection_sha256: String,
    pub(crate) record_count: usize,
    pub(crate) embedding_backend: String,
    pub(crate) embedding_provider: String,
    pub(crate) embedding_model: String,
    pub(crate) embedding_dimensions: usize,
    pub(crate) generated_at_ms: u64,
}

pub(crate) struct MemoryVectorSnapshot {
    pub(crate) database_path: PathBuf,
    pub(crate) manifest: Option<MemoryVectorManifest>,
    pub(crate) generation_id: Option<String>,
    _generation_lease: Option<Arc<()>>,
}

pub(crate) struct PendingMemoryVectorGeneration {
    workspace_root: PathBuf,
    project_id: String,
    pub(crate) generation_id: String,
    generation_root: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) manifest_path: PathBuf,
    generation_lease: Option<Arc<()>>,
    published: bool,
}

impl PendingMemoryVectorGeneration {
    pub(crate) fn create(workspace_root: &Path, project_id: &str) -> Result<Self, String> {
        let generation_id = unique_id("memory-vector");
        if !valid_memory_vector_generation_id(&generation_id) {
            return Err("generated memory vector generation id is invalid".to_string());
        }
        let (database_path, manifest_path) =
            memory_vector_generation_paths(workspace_root, project_id, &generation_id);
        let generation_root = database_path
            .parent()
            .ok_or_else(|| "memory vector generation has no root directory".to_string())?
            .to_path_buf();
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            project_id: project_id.to_string(),
            generation_id,
            generation_root,
            database_path,
            manifest_path,
            generation_lease: None,
            published: false,
        })
    }

    pub(crate) fn acquire_lease(&mut self) -> Result<(), String> {
        self.generation_lease = Some(acquire_memory_vector_generation_lease(
            &self.generation_root,
        )?);
        Ok(())
    }

    pub(crate) fn publish(&mut self) -> Result<(), String> {
        publish_memory_vector_generation(
            &self.workspace_root,
            &self.project_id,
            &self.generation_id,
        )?;
        self.published = true;
        Ok(())
    }
}

impl Drop for PendingMemoryVectorGeneration {
    fn drop(&mut self) {
        if self.published || !valid_memory_vector_generation_id(&self.generation_id) {
            return;
        }
        let expected_root = memory_lancedb_root_for(&self.workspace_root, &self.project_id)
            .join("generations")
            .join(&self.generation_id);
        if self.generation_root == expected_root && self.generation_root.is_dir() {
            let _ = fs::remove_dir_all(&self.generation_root);
        }
    }
}

pub(crate) fn memory_vector_projection_sha256(ledger: &MemoryLedger) -> String {
    let mut records = ledger
        .records
        .iter()
        .filter(|record| ledger.record_is_active_for_recall(record))
        .map(|record| format!("{}:{}", record.id, record.fingerprint))
        .collect::<Vec<_>>();
    records.sort();
    sha256_hex(format!("{}\n{}", ledger.project_id, records.join("\n")).as_bytes())
}

pub(crate) fn memory_recall_projection_sha256(ledger: &MemoryLedger) -> String {
    let mut records = ledger
        .records
        .iter()
        .map(|record| {
            format!(
                "{}:{}:{}:{}:{}:{}:{}",
                record.id,
                record.fingerprint,
                record.importance,
                record.provenance.sequence,
                ledger.record_is_active_for_recall(record),
                ledger.is_pinned(&record.id),
                record.superseded_by.as_deref().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    records.sort();
    sha256_hex(format!("{}\n{}", ledger.project_id, records.join("\n")).as_bytes())
}

pub(crate) fn memory_vector_manifest_matches(
    manifest: &MemoryVectorManifest,
    projection_sha256: &str,
    config: &ProviderConfig,
    now_ms: u64,
) -> bool {
    if manifest.schema != MEMORY_VECTOR_MANIFEST_SCHEMA
        || manifest.projection_sha256 != projection_sha256
    {
        return false;
    }
    if manifest.record_count == 0 {
        return true;
    }
    if config.is_ready() {
        let configured_model = config.model_for_role(&ModelRole::Embedder);
        (manifest.embedding_backend == "cloud" && manifest.embedding_model == configured_model)
            || (manifest.embedding_backend == "local-fallback"
                && now_ms.saturating_sub(manifest.generated_at_ms)
                    < MEMORY_VECTOR_FALLBACK_RETRY_MS)
    } else {
        manifest.embedding_backend == "local"
    }
}

pub(crate) fn load_memory_vector_manifest(
    path: &Path,
) -> Result<Option<MemoryVectorManifest>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("failed to decode memory vector manifest: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read memory vector manifest: {error}")),
    }
}

fn valid_memory_vector_generation_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn memory_vector_generation_paths(
    workspace_root: &Path,
    project_id: &str,
    generation_id: &str,
) -> (PathBuf, PathBuf) {
    let generation_root = memory_lancedb_root_for(workspace_root, project_id)
        .join("generations")
        .join(generation_id);
    (
        generation_root.join("index"),
        generation_root.join("manifest.json"),
    )
}

fn memory_vector_paths_for_read_unlocked(
    workspace_root: &Path,
    project_id: &str,
) -> (PathBuf, PathBuf, Option<String>) {
    let current_path = memory_lancedb_root_for(workspace_root, project_id).join("CURRENT");
    if let Ok(generation_id) = fs::read_to_string(current_path) {
        let generation_id = generation_id.trim();
        if valid_memory_vector_generation_id(generation_id) {
            let (database_path, manifest_path) =
                memory_vector_generation_paths(workspace_root, project_id, generation_id);
            return (
                database_path,
                manifest_path,
                Some(generation_id.to_string()),
            );
        }
    }
    (
        memory_lancedb_database_path_for(workspace_root, project_id),
        memory_lancedb_manifest_path_for(workspace_root, project_id),
        None,
    )
}

pub(crate) fn memory_vector_project_key(workspace_root: &Path, project_id: &str) -> String {
    format!(
        "{}:{project_id}",
        fs::canonicalize(workspace_root)
            .unwrap_or_else(|_| workspace_root.to_path_buf())
            .display()
    )
}

fn memory_vector_project_lock(
    workspace_root: &Path,
    project_id: &str,
) -> Result<Arc<RwLock<()>>, String> {
    let key = memory_vector_project_key(workspace_root, project_id);
    let mut locks = MEMORY_VECTOR_PROJECT_LOCKS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| format!("memory vector project lock registry poisoned: {error}"))?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(RwLock::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    Ok(lock)
}

fn acquire_memory_vector_generation_lease(generation_root: &Path) -> Result<Arc<()>, String> {
    let generation_root =
        fs::canonicalize(generation_root).unwrap_or_else(|_| generation_root.to_path_buf());
    let mut leases = MEMORY_VECTOR_GENERATION_LEASES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| format!("memory vector generation lease registry poisoned: {error}"))?;
    leases.retain(|_, lease| lease.strong_count() > 0);
    if let Some(lease) = leases.get(&generation_root).and_then(Weak::upgrade) {
        return Ok(lease);
    }
    let lease = Arc::new(());
    leases.insert(generation_root, Arc::downgrade(&lease));
    Ok(lease)
}

pub(crate) fn open_memory_vector_snapshot(
    workspace_root: &Path,
    project_id: &str,
) -> Result<MemoryVectorSnapshot, String> {
    let project_lock = memory_vector_project_lock(workspace_root, project_id)?;
    let _read_guard = project_lock
        .read()
        .map_err(|error| format!("memory vector project read lock poisoned: {error}"))?;
    let (database_path, manifest_path, generation_id) =
        memory_vector_paths_for_read_unlocked(workspace_root, project_id);
    let generation_lease = database_path
        .parent()
        .filter(|_| generation_id.is_some())
        .map(acquire_memory_vector_generation_lease)
        .transpose()?;
    let manifest = load_memory_vector_manifest(&manifest_path)?;
    Ok(MemoryVectorSnapshot {
        database_path,
        manifest,
        generation_id,
        _generation_lease: generation_lease,
    })
}

#[cfg(test)]
pub(crate) fn memory_vector_paths_for_read(
    workspace_root: &Path,
    project_id: &str,
) -> (PathBuf, PathBuf, Option<String>) {
    let project_lock = memory_vector_project_lock(workspace_root, project_id)
        .expect("memory vector project lock should be available");
    let _read_guard = project_lock
        .read()
        .expect("memory vector project read lock should be available");
    memory_vector_paths_for_read_unlocked(workspace_root, project_id)
}

fn publish_memory_vector_generation(
    workspace_root: &Path,
    project_id: &str,
    generation_id: &str,
) -> Result<(), String> {
    let project_lock = memory_vector_project_lock(workspace_root, project_id)?;
    let _write_guard = project_lock
        .write()
        .map_err(|error| format!("memory vector project write lock poisoned: {error}"))?;
    let (database_path, manifest_path) =
        memory_vector_generation_paths(workspace_root, project_id, generation_id);
    let manifest = load_memory_vector_manifest(&manifest_path)?
        .ok_or_else(|| "memory vector generation manifest is missing".to_string())?;
    if manifest.schema != MEMORY_VECTOR_MANIFEST_SCHEMA
        || manifest.generation_id != generation_id
        || (manifest.record_count > 0 && !lancedb_index_exists(&database_path))
    {
        return Err("memory vector generation is incomplete before publication".to_string());
    }
    let current_path = memory_lancedb_root_for(workspace_root, project_id).join("CURRENT");
    write_private_file_atomically(
        &current_path,
        format!("{generation_id}\n").as_bytes(),
        "memory vector generation pointer",
    )?;
    if let Err(error) =
        garbage_collect_memory_vector_generations(workspace_root, project_id, generation_id)
    {
        eprintln!("memory vector generation cleanup unavailable: {error}");
    }
    Ok(())
}

fn garbage_collect_memory_vector_generations(
    workspace_root: &Path,
    project_id: &str,
    current_generation: &str,
) -> Result<Vec<String>, String> {
    let generations_root = memory_lancedb_root_for(workspace_root, project_id).join("generations");
    let entries = match fs::read_dir(&generations_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "failed to inspect memory vector generations directory: {error}"
            ))
        }
    };
    let mut complete = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to inspect memory vector generation: {error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("failed to inspect memory vector generation type: {error}"))?
            .is_dir()
        {
            continue;
        }
        let Some(generation_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !valid_memory_vector_generation_id(&generation_id) {
            continue;
        }
        let (database_path, manifest_path) =
            memory_vector_generation_paths(workspace_root, project_id, &generation_id);
        let Ok(Some(manifest)) = load_memory_vector_manifest(&manifest_path) else {
            continue;
        };
        if manifest.schema == MEMORY_VECTOR_MANIFEST_SCHEMA
            && manifest.generation_id == generation_id
            && (manifest.record_count == 0 || lancedb_index_exists(&database_path))
        {
            complete.push((manifest.generated_at_ms, generation_id, database_path));
        }
    }
    complete.sort_by(|left, right| (right.0, &right.1).cmp(&(left.0, &left.1)));
    let mut retained = BTreeSet::from([current_generation.to_string()]);
    retained.extend(
        complete
            .iter()
            .filter(|(_, generation_id, _)| generation_id != current_generation)
            .take(MEMORY_VECTOR_RETAINED_PREVIOUS_GENERATIONS)
            .map(|(_, generation_id, _)| generation_id.clone()),
    );
    let mut removed = Vec::new();
    for (_, generation_id, database_path) in complete {
        if retained.contains(&generation_id) {
            continue;
        }
        let generation_root = database_path
            .parent()
            .ok_or_else(|| "memory vector generation has no root directory".to_string())?;
        let canonical_root =
            fs::canonicalize(generation_root).unwrap_or_else(|_| generation_root.to_path_buf());
        let mut leases = MEMORY_VECTOR_GENERATION_LEASES
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|error| {
                format!("memory vector generation lease registry poisoned: {error}")
            })?;
        leases.retain(|_, lease| lease.strong_count() > 0);
        if leases
            .get(&canonical_root)
            .and_then(Weak::upgrade)
            .is_some()
        {
            continue;
        }
        fs::remove_dir_all(generation_root).map_err(|error| {
            format!("failed to remove memory vector generation {generation_id}: {error}")
        })?;
        removed.push(generation_id);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    fn test_workspace(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cindx-{label}-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&root).expect("test workspace should exist");
        root
    }

    fn stage_complete_generation(
        root: &Path,
        project_id: &str,
        generation_id: &str,
        generated_at_ms: u64,
    ) -> PathBuf {
        let (database_path, manifest_path) =
            memory_vector_generation_paths(root, project_id, generation_id);
        fs::create_dir_all(
            database_path
                .parent()
                .expect("generation should have a root directory"),
        )
        .expect("generation root should exist");
        let manifest = MemoryVectorManifest {
            schema: MEMORY_VECTOR_MANIFEST_SCHEMA.to_string(),
            generation_id: generation_id.to_string(),
            projection_sha256: format!("projection-{generation_id}"),
            record_count: 0,
            embedding_backend: "local".to_string(),
            embedding_provider: "local".to_string(),
            embedding_model: "local-hash".to_string(),
            embedding_dimensions: 0,
            generated_at_ms,
        };
        write_private_file_atomically(
            &manifest_path,
            &serde_json::to_vec(&manifest).expect("manifest should encode"),
            "test memory vector manifest",
        )
        .expect("manifest should write");
        database_path
            .parent()
            .expect("generation should have a root directory")
            .to_path_buf()
    }

    #[test]
    fn publication_retains_current_and_two_previous_complete_generations() {
        let root = test_workspace("memory-vector-retention");
        let project_id = "memory-vector-retention-project";
        let generations_root = memory_lancedb_root_for(&root, project_id).join("generations");
        fs::create_dir_all(generations_root.join("not a valid generation"))
            .expect("invalid generation should exist");
        fs::create_dir_all(generations_root.join("memory-vector-incomplete"))
            .expect("incomplete generation should exist");
        let generation_ids = (1..=5)
            .map(|sequence| format!("memory-vector-test-{sequence}"))
            .collect::<Vec<_>>();
        for (offset, generation_id) in generation_ids.iter().enumerate() {
            stage_complete_generation(&root, project_id, generation_id, offset as u64 + 1);
            publish_memory_vector_generation(&root, project_id, generation_id)
                .expect("generation should publish");
        }

        let retained = generation_ids
            .iter()
            .filter(|generation_id| generations_root.join(generation_id).is_dir())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(retained, generation_ids[2..].to_vec());
        assert!(generations_root.join("not a valid generation").is_dir());
        assert!(generations_root.join("memory-vector-incomplete").is_dir());
    }

    #[test]
    fn active_snapshot_is_not_collected() {
        let root = test_workspace("memory-vector-active-reader");
        let project_id = "memory-vector-active-reader-project";
        let first_id = "memory-vector-active-1";
        let first_root = stage_complete_generation(&root, project_id, first_id, 1);
        publish_memory_vector_generation(&root, project_id, first_id)
            .expect("first generation should publish");
        let snapshot =
            open_memory_vector_snapshot(&root, project_id).expect("snapshot should open");
        for sequence in 2..=4 {
            let generation_id = format!("memory-vector-active-{sequence}");
            stage_complete_generation(&root, project_id, &generation_id, sequence);
            publish_memory_vector_generation(&root, project_id, &generation_id)
                .expect("replacement generation should publish");
        }

        assert!(first_root.is_dir());
        assert_eq!(snapshot.generation_id.as_deref(), Some(first_id));
        assert_eq!(
            snapshot
                .manifest
                .as_ref()
                .map(|manifest| manifest.generation_id.as_str()),
            Some(first_id)
        );
        drop(snapshot);
        let fifth_id = "memory-vector-active-5";
        stage_complete_generation(&root, project_id, fifth_id, 5);
        publish_memory_vector_generation(&root, project_id, fifth_id)
            .expect("cleanup-triggering generation should publish");
        assert!(!first_root.exists());
    }

    #[test]
    fn concurrent_resolve_pins_database_and_manifest_generation() {
        let root = test_workspace("memory-vector-concurrent-reader");
        let project_id = "memory-vector-concurrent-reader-project";
        stage_complete_generation(&root, project_id, "memory-vector-concurrent-0", 0);
        publish_memory_vector_generation(&root, project_id, "memory-vector-concurrent-0")
            .expect("initial generation should publish");
        let barrier = Arc::new(Barrier::new(2));
        let writer_root = root.clone();
        let writer_barrier = barrier.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            for sequence in 1..=20 {
                let generation_id = format!("memory-vector-concurrent-{sequence}");
                let generation_lease = {
                    let generation_root = stage_complete_generation(
                        &writer_root,
                        project_id,
                        &generation_id,
                        sequence,
                    );
                    acquire_memory_vector_generation_lease(&generation_root)
                        .expect("staged generation should be leased")
                };
                publish_memory_vector_generation(&writer_root, project_id, &generation_id)
                    .expect("concurrent generation should publish");
                drop(generation_lease);
            }
        });
        barrier.wait();
        for _ in 0..200 {
            let snapshot = open_memory_vector_snapshot(&root, project_id)
                .expect("concurrent snapshot should open");
            let generation_id = snapshot
                .generation_id
                .as_deref()
                .expect("snapshot should have a generation");
            assert_eq!(
                snapshot
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.generation_id.as_str()),
                Some(generation_id)
            );
            assert_eq!(
                snapshot
                    .database_path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(OsStr::to_str),
                Some(generation_id)
            );
            std::thread::yield_now();
        }
        writer.join().expect("memory vector publisher should join");
    }

    #[test]
    fn unpublished_generation_drop_removes_only_its_exact_directory() {
        let root = test_workspace("memory-vector-pending-drop");
        let project_id = "memory-vector-pending-drop-project";
        let pending = PendingMemoryVectorGeneration::create(&root, project_id)
            .expect("pending generation should be created");
        fs::create_dir_all(&pending.generation_root).expect("pending root should exist");
        fs::write(pending.generation_root.join("partial"), "partial")
            .expect("partial artifact should write");
        let sibling = pending
            .generation_root
            .parent()
            .expect("pending root should have a parent")
            .join("memory-vector-unrelated");
        fs::create_dir_all(&sibling).expect("unrelated sibling should exist");
        let pending_root = pending.generation_root.clone();
        drop(pending);

        assert!(!pending_root.exists());
        assert!(sibling.is_dir());
    }
}
