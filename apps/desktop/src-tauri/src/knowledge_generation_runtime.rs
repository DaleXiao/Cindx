use crate::desktop_prelude::*;
use std::sync::{OnceLock, RwLock, Weak};

const KNOWLEDGE_GENERATIONS_DIRECTORY: &str = "knowledge-generations";
const KNOWLEDGE_CURRENT_FILE: &str = "knowledge-current";
const KNOWLEDGE_MANIFEST_FILE: &str = "manifest";
const KNOWLEDGE_MANIFEST_SCHEMA: &str = "cindx-knowledge-generation-v1";
const KNOWLEDGE_RETAINED_PREVIOUS_GENERATIONS: usize = 2;

static WORKSPACE_INDEX_LOCKS: OnceLock<Mutex<BTreeMap<String, Weak<Mutex<()>>>>> = OnceLock::new();
static WORKSPACE_GENERATION_LOCKS: OnceLock<Mutex<BTreeMap<String, Weak<RwLock<()>>>>> =
    OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnowledgeGenerationPaths {
    pub(crate) generation_id: Option<String>,
    pub(crate) root: PathBuf,
    pub(crate) rag_index: PathBuf,
    pub(crate) graph_store: PathBuf,
    pub(crate) lancedb_export: PathBuf,
    pub(crate) lancedb_database: PathBuf,
}

#[derive(Debug)]
pub(crate) struct PublishedKnowledgeGeneration {
    pub(crate) paths: KnowledgeGenerationPaths,
    pub(crate) adapter: FileRagAdapter,
    pub(crate) graph_nodes: usize,
    pub(crate) graph_edges: usize,
    pub(crate) lancedb_export_records: usize,
    pub(crate) lancedb_records: usize,
}

pub(crate) fn legacy_knowledge_paths(workspace_root: &Path) -> KnowledgeGenerationPaths {
    let root = workspace_root.join(".cindx");
    knowledge_paths_in_root(root, None)
}

pub(crate) fn active_knowledge_paths(workspace_root: &Path) -> KnowledgeGenerationPaths {
    published_knowledge_paths(workspace_root)
        .unwrap_or_else(|| legacy_knowledge_paths(workspace_root))
}

pub(crate) fn open_active_knowledge_adapter(
    workspace_root: &Path,
) -> Result<FileRagAdapter, String> {
    with_workspace_generation_read(workspace_root, || {
        let paths = published_knowledge_paths(workspace_root)
            .unwrap_or_else(|| legacy_knowledge_paths(workspace_root));
        FileRagAdapter::open(paths.rag_index).map_err(|error| error.to_string())
    })
}

#[cfg(test)]
pub(crate) fn with_active_knowledge_paths<T>(
    workspace_root: &Path,
    operation: impl FnOnce(&KnowledgeGenerationPaths) -> Result<T, String>,
) -> Result<T, String> {
    with_workspace_generation_read(workspace_root, || {
        let paths = published_knowledge_paths(workspace_root)
            .unwrap_or_else(|| legacy_knowledge_paths(workspace_root));
        operation(&paths)
    })
}

pub(crate) fn knowledge_paths_for_rag_index(index_path: &Path) -> KnowledgeGenerationPaths {
    let root = index_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".cindx"));
    let generation_id = root
        .parent()
        .filter(|parent| {
            parent.file_name().and_then(OsStr::to_str) == Some(KNOWLEDGE_GENERATIONS_DIRECTORY)
        })
        .and_then(|_| root.file_name())
        .and_then(OsStr::to_str)
        .filter(|value| valid_generation_id(value))
        .map(str::to_string);
    knowledge_paths_in_root(root, generation_id)
}

pub(crate) fn published_knowledge_paths(workspace_root: &Path) -> Option<KnowledgeGenerationPaths> {
    let cindx_root = workspace_root.join(".cindx");
    let generation_id = fs::read_to_string(cindx_root.join(KNOWLEDGE_CURRENT_FILE))
        .ok()?
        .trim()
        .to_string();
    if !valid_generation_id(&generation_id) {
        return None;
    }
    let paths = generation_paths(workspace_root, &generation_id);
    knowledge_manifest_is_complete(&paths).then_some(paths)
}

#[cfg(test)]
pub(crate) fn with_workspace_knowledge_index_lock<T>(
    workspace_root: &Path,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    with_workspace_knowledge_index_lock_cancellable(workspace_root, || false, operation)
}

pub(crate) fn with_workspace_knowledge_index_lock_cancellable<T>(
    workspace_root: &Path,
    mut should_cancel: impl FnMut() -> bool,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let key = fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf())
        .display()
        .to_string();
    let lock = {
        let mut locks = WORKSPACE_INDEX_LOCKS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|error| format!("workspace index lock registry poisoned: {error}"))?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(Mutex::new(()));
            locks.insert(key, Arc::downgrade(&lock));
            lock
        }
    };
    let _guard = loop {
        if should_cancel() {
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        match lock.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::park_timeout(Duration::from_millis(20));
            }
            Err(std::sync::TryLockError::Poisoned(error)) => {
                return Err(format!("workspace index lock poisoned: {error}"));
            }
        }
    };
    if should_cancel() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    operation()
}

fn workspace_lock_key(workspace_root: &Path) -> String {
    fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf())
        .display()
        .to_string()
}

fn workspace_generation_lock(workspace_root: &Path) -> Result<Arc<RwLock<()>>, String> {
    let key = workspace_lock_key(workspace_root);
    let mut locks = WORKSPACE_GENERATION_LOCKS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| format!("workspace generation lock registry poisoned: {error}"))?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(RwLock::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    Ok(lock)
}

pub(crate) fn with_workspace_generation_read<T>(
    workspace_root: &Path,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock = workspace_generation_lock(workspace_root)?;
    let _guard = lock
        .read()
        .map_err(|error| format!("workspace generation read lock poisoned: {error}"))?;
    operation()
}

pub(crate) fn build_and_publish_knowledge_generation_cancellable(
    workspace_root: &Path,
    index: RagIndex,
    should_cancel: impl FnMut() -> bool,
) -> Result<PublishedKnowledgeGeneration, String> {
    build_and_publish_knowledge_generation_with_commit(
        workspace_root,
        index,
        should_cancel,
        || Ok(()),
        |()| {},
    )
}

pub(crate) fn build_and_publish_knowledge_generation_with_commit<G>(
    workspace_root: &Path,
    index: RagIndex,
    mut should_cancel: impl FnMut() -> bool,
    begin_commit: impl FnOnce() -> Result<G, String>,
    commit_succeeded: impl FnOnce(G),
) -> Result<PublishedKnowledgeGeneration, String> {
    if should_cancel() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let mut pending = PendingKnowledgeGeneration::create(workspace_root)?;
    let (graph_nodes, graph_edges) = index_graph_at_path_cancellable(
        &pending.paths.graph_store,
        &index.chunks,
        &mut should_cancel,
    )?;
    if should_cancel() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let lancedb_export_records = export_lancedb_records_jsonl_cancellable(
        &index,
        &pending.paths.lancedb_export,
        &mut should_cancel,
    )
    .map_err(rag_generation_error)?;
    if should_cancel() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let lancedb_records = replace_lancedb_index_cancellable(
        &pending.paths.lancedb_database,
        &index,
        &mut should_cancel,
    )
    .map_err(rag_generation_error)?;
    if should_cancel() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let mut adapter =
        FileRagAdapter::open(&pending.paths.rag_index).map_err(|error| error.to_string())?;
    adapter
        .replace_all_cancellable(index, &mut should_cancel)
        .map_err(rag_generation_error)?;
    if should_cancel() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let commit = begin_commit()?;
    pending.publish(lancedb_records)?;
    commit_succeeded(commit);
    Ok(PublishedKnowledgeGeneration {
        paths: pending.paths.clone(),
        adapter,
        graph_nodes,
        graph_edges,
        lancedb_export_records,
        lancedb_records,
    })
}

pub(crate) fn index_graph_at_path_cancellable(
    graph_path: &Path,
    chunks: &[RagChunk],
    mut should_cancel: impl FnMut() -> bool,
) -> Result<(usize, usize), String> {
    let mut graph_store = FileGraphStore::open(graph_path).map_err(|error| error.to_string())?;
    let mut cancelled = false;
    let extractions = chunks.iter().map_while(|chunk| {
        if should_cancel() {
            cancelled = true;
            None
        } else {
            Some(extract_graph_from_chunk(chunk))
        }
    });
    graph_store
        .upsert_all(extractions)
        .map_err(|error| error.to_string())?;
    if cancelled || should_cancel() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    Ok((graph_store.node_count(), graph_store.edge_count()))
}

fn knowledge_paths_in_root(
    root: PathBuf,
    generation_id: Option<String>,
) -> KnowledgeGenerationPaths {
    KnowledgeGenerationPaths {
        generation_id,
        rag_index: root.join("rag-index.tsv"),
        graph_store: root.join("graph.tsv"),
        lancedb_export: root.join("lancedb-records.jsonl"),
        lancedb_database: root.join("lancedb"),
        root,
    }
}

fn generation_paths(workspace_root: &Path, generation_id: &str) -> KnowledgeGenerationPaths {
    knowledge_paths_in_root(
        workspace_root
            .join(".cindx")
            .join(KNOWLEDGE_GENERATIONS_DIRECTORY)
            .join(generation_id),
        Some(generation_id.to_string()),
    )
}

fn valid_generation_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn knowledge_manifest_is_complete(paths: &KnowledgeGenerationPaths) -> bool {
    let Some(expected_id) = paths.generation_id.as_deref() else {
        return false;
    };
    let Ok(manifest) = fs::read_to_string(paths.root.join(KNOWLEDGE_MANIFEST_FILE)) else {
        return false;
    };
    let mut lines = manifest.lines();
    if lines.next() != Some(KNOWLEDGE_MANIFEST_SCHEMA) || lines.next() != Some(expected_id) {
        return false;
    }
    let Some(records) = lines.next().and_then(|value| value.parse::<usize>().ok()) else {
        return false;
    };
    paths.rag_index.is_file()
        && paths.graph_store.is_file()
        && paths.lancedb_export.is_file()
        && (records == 0 || lancedb_index_exists(&paths.lancedb_database))
}

fn garbage_collect_knowledge_generations(
    workspace_root: &Path,
    current_generation: &str,
) -> Result<Vec<String>, String> {
    let generations_root = workspace_root
        .join(".cindx")
        .join(KNOWLEDGE_GENERATIONS_DIRECTORY);
    let entries = match fs::read_dir(&generations_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "failed to inspect knowledge generations directory: {error}"
            ))
        }
    };
    let mut complete = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to inspect knowledge generation: {error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("failed to inspect knowledge generation type: {error}"))?
            .is_dir()
        {
            continue;
        }
        let Some(generation_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !valid_generation_id(&generation_id) {
            continue;
        }
        let paths = generation_paths(workspace_root, &generation_id);
        if knowledge_manifest_is_complete(&paths) {
            complete.push((generation_id, paths));
        }
    }
    complete.sort_by(|left, right| right.0.cmp(&left.0));
    let mut retained = BTreeSet::from([current_generation.to_string()]);
    retained.extend(
        complete
            .iter()
            .filter(|(generation_id, _)| generation_id != current_generation)
            .take(KNOWLEDGE_RETAINED_PREVIOUS_GENERATIONS)
            .map(|(generation_id, _)| generation_id.clone()),
    );
    let mut removed = Vec::new();
    for (generation_id, paths) in complete {
        if retained.contains(&generation_id) {
            continue;
        }
        if remove_file_rag_generation_if_unleased(&paths.rag_index, &paths.root)
            .map_err(|error| error.to_string())?
        {
            removed.push(generation_id);
        }
    }
    Ok(removed)
}

fn rag_generation_error(error: RagError) -> String {
    if error.message == RAG_INDEX_CANCELLED {
        MODEL_REQUEST_CANCELLED.to_string()
    } else {
        error.to_string()
    }
}

struct PendingKnowledgeGeneration {
    workspace_root: PathBuf,
    paths: KnowledgeGenerationPaths,
    published: bool,
}

impl PendingKnowledgeGeneration {
    fn create(workspace_root: &Path) -> Result<Self, String> {
        let generation_id = format!("gen-{}", uuid::Uuid::now_v7());
        let paths = generation_paths(workspace_root, &generation_id);
        fs::create_dir_all(
            paths
                .root
                .parent()
                .ok_or_else(|| "knowledge generation has no parent directory".to_string())?,
        )
        .map_err(|error| format!("failed to create knowledge generations directory: {error}"))?;
        fs::create_dir(&paths.root)
            .map_err(|error| format!("failed to create knowledge generation: {error}"))?;
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            paths,
            published: false,
        })
    }

    fn publish(&mut self, records: usize) -> Result<(), String> {
        let generation_id = self
            .paths
            .generation_id
            .as_deref()
            .ok_or_else(|| "knowledge generation id is missing".to_string())?;
        let manifest = format!("{KNOWLEDGE_MANIFEST_SCHEMA}\n{generation_id}\n{records}\n");
        write_synced_file(
            &self.paths.root.join(KNOWLEDGE_MANIFEST_FILE),
            manifest.as_bytes(),
        )?;
        if !knowledge_manifest_is_complete(&self.paths) {
            return Err("knowledge generation is incomplete before publication".to_string());
        }
        let cindx_root = self.workspace_root.join(".cindx");
        let pointer_path = cindx_root.join(KNOWLEDGE_CURRENT_FILE);
        let temporary_path = cindx_root.join(format!(
            ".{KNOWLEDGE_CURRENT_FILE}.{}.tmp",
            uuid::Uuid::now_v7()
        ));
        let generation_lock = workspace_generation_lock(&self.workspace_root)?;
        let _generation_guard = generation_lock
            .write()
            .map_err(|error| format!("workspace generation write lock poisoned: {error}"))?;
        let result = (|| {
            write_synced_file(&temporary_path, format!("{generation_id}\n").as_bytes())?;
            fs::rename(&temporary_path, &pointer_path)
                .map_err(|error| format!("failed to publish knowledge generation: {error}"))?;
            self.published = true;
            if let Ok(directory) = fs::File::open(&cindx_root) {
                let _ = directory.sync_all();
            }
            if let Err(error) =
                garbage_collect_knowledge_generations(&self.workspace_root, generation_id)
            {
                eprintln!("knowledge generation cleanup unavailable: {error}");
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }
}

impl Drop for PendingKnowledgeGeneration {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.paths.root);
        }
    }
}

fn write_synced_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::File::create(path)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("failed to sync {}: {error}", path.display()))
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

    fn test_index(root: &Path, text: &str) -> RagIndex {
        fs::write(root.join("notes.md"), text).expect("fixture should write");
        index_workspace(root, IndexOptions::default()).expect("index should build")
    }

    #[test]
    fn missing_or_incomplete_manifest_falls_back_to_legacy_paths() {
        let root = test_workspace("knowledge-legacy-fallback");
        let legacy = legacy_knowledge_paths(&root);
        fs::create_dir_all(&legacy.root).expect("legacy directory should exist");
        fs::write(legacy.root.join(KNOWLEDGE_CURRENT_FILE), "gen-incomplete\n")
            .expect("pointer should write");
        fs::create_dir_all(
            legacy
                .root
                .join(KNOWLEDGE_GENERATIONS_DIRECTORY)
                .join("gen-incomplete"),
        )
        .expect("incomplete generation should exist");

        assert_eq!(active_knowledge_paths(&root), legacy);
    }

    #[test]
    fn cancellation_does_not_publish_a_partial_generation() {
        let root = test_workspace("knowledge-cancelled-generation");
        let first = test_index(&root, "stable generation evidence");
        let published = build_and_publish_knowledge_generation_cancellable(&root, first, || false)
            .expect("first generation should publish");
        let first_id = published.paths.generation_id.clone();
        fs::write(root.join("notes.md"), "replacement generation evidence")
            .expect("replacement fixture should write");
        let replacement = index_workspace(&root, IndexOptions::default())
            .expect("replacement index should build");
        let mut checks = 0;
        let error = build_and_publish_knowledge_generation_cancellable(&root, replacement, || {
            checks += 1;
            checks >= 3
        })
        .expect_err("replacement should cancel");

        assert_eq!(error, MODEL_REQUEST_CANCELLED);
        assert_eq!(active_knowledge_paths(&root).generation_id, first_id);
    }

    #[test]
    fn backend_failure_does_not_replace_the_published_generation() {
        let root = test_workspace("knowledge-failed-generation");
        let first = test_index(&root, "stable generation evidence");
        let published = build_and_publish_knowledge_generation_cancellable(&root, first, || false)
            .expect("first generation should publish");
        let first_id = published.paths.generation_id.clone();
        fs::write(root.join("notes.md"), "invalid replacement evidence")
            .expect("replacement fixture should write");
        let mut replacement =
            index_workspace(&root, IndexOptions::default()).expect("replacement should build");
        replacement.chunks[0].embedding.clear();
        replacement.chunks[0].embedding_dimensions = 0;

        let error =
            build_and_publish_knowledge_generation_cancellable(&root, replacement, || false)
                .expect_err("invalid LanceDB generation should fail");

        assert!(error.contains("non-empty embedding dimension"));
        assert_eq!(active_knowledge_paths(&root).generation_id, first_id);
    }

    #[test]
    fn adapter_path_keeps_all_channels_on_one_immutable_generation() {
        let root = test_workspace("knowledge-snapshot-paths");
        let first = test_index(&root, "first generation evidence");
        let first = build_and_publish_knowledge_generation_cancellable(&root, first, || false)
            .expect("first generation should publish");
        let snapshot = knowledge_paths_for_rag_index(first.adapter.path());
        fs::write(root.join("notes.md"), "second generation evidence")
            .expect("replacement fixture should write");
        let second = index_workspace(&root, IndexOptions::default())
            .expect("replacement index should build");
        let second = build_and_publish_knowledge_generation_cancellable(&root, second, || false)
            .expect("second generation should publish");

        assert_ne!(snapshot.generation_id, second.paths.generation_id);
        assert_eq!(snapshot.graph_store.parent(), snapshot.rag_index.parent());
        assert_eq!(
            snapshot.lancedb_database.parent(),
            snapshot.rag_index.parent()
        );
        assert!(snapshot.graph_store.exists());
        assert!(lancedb_index_exists(&snapshot.lancedb_database));
    }

    #[test]
    fn graph_projection_uses_the_leased_adapter_generation() {
        let root = test_workspace("knowledge-leased-graph");
        let first_index = test_index(&root, "first graph generation evidence");
        let first =
            build_and_publish_knowledge_generation_cancellable(&root, first_index, || false)
                .expect("first generation should publish");
        let first_graph = crate::persistence_runtime::graph_state_for_adapter(&first.adapter, &[])
            .expect("first graph should load");
        fs::write(
            root.join("second.rs"),
            "struct SecondGenerationMarker; fn second_generation_marker() {}",
        )
        .expect("second graph fixture should write");
        let second_index =
            index_workspace(&root, IndexOptions::default()).expect("second index should build");
        let second =
            build_and_publish_knowledge_generation_cancellable(&root, second_index, || false)
                .expect("second generation should publish");
        let leased_graph = crate::persistence_runtime::graph_state_for_adapter(&first.adapter, &[])
            .expect("leased graph should load");
        let current_graph = crate::persistence_runtime::graph_state_for(&root, &[])
            .expect("current graph should load");

        assert_ne!(first.paths.generation_id, second.paths.generation_id);
        assert_eq!(leased_graph.total_nodes, first_graph.total_nodes);
        assert_eq!(leased_graph.total_edges, first_graph.total_edges);
        assert!(current_graph.total_nodes > leased_graph.total_nodes);
    }

    #[test]
    fn publication_retains_current_and_two_previous_complete_generations_only() {
        let root = test_workspace("knowledge-generation-retention");
        let index = test_index(&root, "bounded generation evidence");
        let generations_root = root.join(".cindx").join(KNOWLEDGE_GENERATIONS_DIRECTORY);
        fs::create_dir_all(generations_root.join("not a valid generation"))
            .expect("invalid directory should exist");
        fs::create_dir_all(generations_root.join("gen-incomplete"))
            .expect("incomplete generation should exist");
        let mut generation_ids = Vec::new();
        for _ in 0..5 {
            let published =
                build_and_publish_knowledge_generation_cancellable(&root, index.clone(), || false)
                    .expect("generation should publish");
            generation_ids.push(
                published
                    .paths
                    .generation_id
                    .clone()
                    .expect("published generation should have an id"),
            );
            drop(published);
        }

        let retained = generation_ids
            .iter()
            .filter(|generation_id| generation_paths(&root, generation_id).root.is_dir())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(retained, generation_ids[2..].to_vec());
        assert!(generations_root.join("not a valid generation").is_dir());
        assert!(generations_root.join("gen-incomplete").is_dir());
    }

    #[test]
    fn active_adapter_generation_is_not_collected() {
        let root = test_workspace("knowledge-active-generation");
        let index = test_index(&root, "leased generation evidence");
        let first =
            build_and_publish_knowledge_generation_cancellable(&root, index.clone(), || false)
                .expect("first generation should publish");
        let first_root = first.paths.root.clone();
        let active_adapter = first.adapter.clone();
        drop(first);
        for _ in 0..3 {
            let published =
                build_and_publish_knowledge_generation_cancellable(&root, index.clone(), || false)
                    .expect("replacement generation should publish");
            drop(published);
        }

        assert!(first_root.is_dir());
        assert!(!active_adapter.chunks().is_empty());
        drop(active_adapter);
        let published = build_and_publish_knowledge_generation_cancellable(&root, index, || false)
            .expect("cleanup-triggering generation should publish");
        drop(published);
        assert!(!first_root.exists());
    }

    #[test]
    fn concurrent_pointer_resolution_and_publication_keep_opened_snapshot_alive() {
        let root = test_workspace("knowledge-concurrent-generation");
        let index = test_index(&root, "concurrent generation evidence");
        let initial =
            build_and_publish_knowledge_generation_cancellable(&root, index.clone(), || false)
                .expect("initial generation should publish");
        drop(initial);
        let barrier = Arc::new(Barrier::new(2));
        let writer_root = root.clone();
        let writer_index = index.clone();
        let writer_barrier = barrier.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            for _ in 0..5 {
                let published = build_and_publish_knowledge_generation_cancellable(
                    &writer_root,
                    writer_index.clone(),
                    || false,
                )
                .expect("concurrent generation should publish");
                drop(published);
            }
        });
        barrier.wait();
        for _ in 0..100 {
            let adapter = open_active_knowledge_adapter(&root)
                .expect("active generation should resolve and open atomically");
            let paths = knowledge_paths_for_rag_index(adapter.path());
            assert!(paths.root.is_dir());
            assert!(paths.rag_index.is_file());
            assert!(!adapter.chunks().is_empty());
            std::thread::yield_now();
        }
        writer.join().expect("publisher should join");
    }

    #[test]
    fn automatic_and_manual_indexers_serialize_per_workspace() {
        let root = test_workspace("knowledge-index-lock");
        let barrier = Arc::new(Barrier::new(3));
        let active = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let barrier = barrier.clone();
            let active = active.clone();
            let peak = peak.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                with_workspace_knowledge_index_lock(&root, || {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::yield_now();
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .expect("serialized operation should finish");
            }));
        }
        barrier.wait();
        for handle in handles {
            handle.join().expect("index thread should join");
        }

        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }
}
