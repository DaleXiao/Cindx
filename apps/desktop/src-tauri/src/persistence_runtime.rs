use super::*;
#[cfg(test)]
use crate::knowledge_generation_runtime::legacy_knowledge_paths;
#[cfg(test)]
use crate::knowledge_generation_runtime::with_active_knowledge_paths;
use crate::knowledge_generation_runtime::{
    active_knowledge_paths, knowledge_paths_for_rag_index, open_active_knowledge_adapter,
    with_workspace_generation_read,
};

pub(crate) fn validate_workspace_root(path: &str) -> Result<PathBuf, String> {
    let path = normalized_config_value(path);
    if path.is_empty() {
        return Err("workspace path is empty".to_string());
    }
    let candidate = PathBuf::from(path);
    let root = if candidate.is_absolute() {
        candidate
    } else {
        workspace_root().join(candidate)
    };
    let canonical =
        fs::canonicalize(&root).map_err(|error| format!("failed to resolve workspace: {error}"))?;
    if !canonical.is_dir() {
        return Err("workspace path must be a directory".to_string());
    }

    Ok(canonical)
}

pub(crate) fn active_workspace_root(state: &tauri::State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .workspace_config
        .lock()
        .map(|config| config.root.clone())
        .map_err(|error| format!("workspace config lock poisoned: {error}"))
}

pub(crate) fn project_root_for_session(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<PathBuf, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let session = config
        .sessions
        .iter()
        .find(|session| session.id == session_id && session.archived_at_ms.is_none())
        .ok_or_else(|| "session not found".to_string())?;
    let project = config
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| "project not found for session".to_string())?;
    validate_workspace_root(&project.root)
}

pub(crate) fn safe_attachment_name(value: &str) -> String {
    let file_name = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment");
    let sanitized = file_name
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| match character {
            '/' | '\\' | ':' => '_',
            other => other,
        })
        .take(120)
        .collect::<String>();
    if sanitized.trim().is_empty() {
        "attachment".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn normalized_attachment_mime(value: &str, path: &Path) -> String {
    let value = normalized_config_value(value).to_ascii_lowercase();
    if value.starts_with("image/") || value.starts_with("text/") {
        return value;
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "avif" => "image/avif",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_string()
}

pub(crate) fn attachment_storage_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("attachments")
}

pub(crate) fn validated_attachment_path(
    workspace_root: &Path,
    value: &str,
) -> Result<PathBuf, String> {
    let attachment_root = attachment_storage_root(workspace_root);
    let canonical_root = fs::canonicalize(&attachment_root)
        .map_err(|error| format!("failed to resolve attachment directory: {error}"))?;
    let canonical_path = fs::canonicalize(value)
        .map_err(|error| format!("failed to resolve attachment: {error}"))?;
    if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
        return Err("attachment must be a staged file for this project".to_string());
    }
    Ok(canonical_path)
}

pub(crate) fn tool_registry_for_state(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<ToolRegistry, String> {
    let generation = state.tool_registry_generation.load(Ordering::Acquire);
    if let Some(registry) = state
        .tool_registry_cache
        .lock()
        .map_err(|error| format!("tool registry cache lock poisoned: {error}"))?
        .get(workspace_root, generation)
    {
        return Ok(registry);
    }

    let registry = build_tool_registry_for_state(state, workspace_root)?;
    if state.tool_registry_generation.load(Ordering::Acquire) == generation {
        state
            .tool_registry_cache
            .lock()
            .map_err(|error| format!("tool registry cache lock poisoned: {error}"))?
            .insert(workspace_root.to_path_buf(), generation, registry.clone());
    }
    Ok(registry)
}

pub(crate) fn build_tool_registry_for_state(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<ToolRegistry, String> {
    let web_search_config = state
        .web_search_config
        .lock()
        .map_err(|error| format!("web search config lock poisoned: {error}"))?
        .clone();
    let provider_config = state
        .provider_config
        .lock()
        .map_err(|error| format!("provider config lock poisoned: {error}"))?
        .clone();
    let image_endpoint = if provider_config.image_endpoint.trim().is_empty() {
        provider_config.base_url.clone()
    } else {
        provider_config.image_endpoint.clone()
    };
    let image_generation_config =
        (!provider_config.image_model.trim().is_empty()).then_some(ImageGenerationConfig {
            base_url: image_endpoint,
            api_key: provider_config.api_key,
            model: provider_config.image_model,
            timeout_seconds: 300,
        });
    let mut registry = ToolRegistry::with_workspace_tools_and_services_and_process_manager(
        workspace_root.to_path_buf(),
        web_search_config,
        image_generation_config,
        Arc::clone(&state.process_manager),
    );
    let catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    for tool in catalog.cached_tools() {
        if let Err(error) = registry.try_register(tool) {
            eprintln!("skipping invalid MCP tool: {error}");
        }
    }
    drop(catalog);
    for tool in skill_catalog_for_root(workspace_root).tools() {
        if let Err(error) = registry.try_register(tool) {
            eprintln!("skipping invalid skill tool: {error}");
        }
    }
    registry.install_meta_tools();
    Ok(registry)
}

pub(crate) fn invalidate_tool_registry_cache(state: &AppState) -> Result<(), String> {
    state
        .tool_registry_generation
        .fetch_add(1, Ordering::AcqRel);
    state
        .tool_registry_cache
        .lock()
        .map_err(|error| format!("tool registry cache lock poisoned: {error}"))?
        .clear();
    Ok(())
}

pub(crate) fn skill_catalog_for_root(workspace_root: &Path) -> SkillCatalog {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.to_path_buf());
    SkillCatalog::load(
        home.join(".cindx/skills"),
        workspace_root,
        home.join(".cindx/skill-preferences.json"),
    )
}

pub(crate) fn open_app_store() -> Result<SqliteStore, StorageError> {
    let database_path = database_path();
    open_app_store_at(&database_path)
}

pub(crate) fn open_app_store_at(database_path: &Path) -> Result<SqliteStore, StorageError> {
    if let Some(parent) = database_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            StorageError::new(format!(
                "failed to create Cindx data directory {}: {error}",
                parent.display()
            ))
        })?;
        secure_directory(parent).map_err(|error| {
            StorageError::new(format!(
                "failed to secure Cindx data directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let store = SqliteStore::open(database_path).map_err(|error| {
        StorageError::new(format!(
            "failed to open Cindx state database {}: {error}",
            database_path.display()
        ))
    })?;
    #[cfg(unix)]
    fs::set_permissions(database_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        StorageError::new(format!(
            "failed to secure Cindx state database {}: {error}",
            database_path.display()
        ))
    })?;

    Ok(store)
}

pub(crate) fn open_app_read_store() -> Result<SqliteStore, String> {
    SqliteStore::open_read_only(database_path()).map_err(|error| error.to_string())
}

pub(crate) fn database_path() -> PathBuf {
    app_data_root().join("state.sqlite3")
}

pub(crate) fn provider_config_path() -> PathBuf {
    app_data_root().join("provider.conf")
}

pub(crate) fn personalization_config_path() -> PathBuf {
    app_data_root().join("personalization.json")
}

pub(crate) fn project_instructions_config_path() -> PathBuf {
    app_data_root().join("project_instructions.json")
}

pub(crate) fn workspace_config_path() -> PathBuf {
    app_data_root().join("workspace.conf")
}

pub(crate) fn project_session_config_path() -> PathBuf {
    app_data_root().join("projects.conf")
}

pub(crate) fn schedule_config_path() -> PathBuf {
    app_data_root().join("schedules.json")
}

pub(crate) fn sidecar_config_path() -> PathBuf {
    app_data_root().join("sidecars.conf")
}

pub(crate) fn web_search_config_path() -> PathBuf {
    app_data_root().join("web-search.conf")
}

pub(crate) fn mcp_config_path() -> PathBuf {
    app_data_root().join("mcp-servers.json")
}

pub(crate) fn mcp_catalog_cache_path() -> PathBuf {
    app_data_root().join("mcp-catalog.json")
}

pub(crate) fn rag_index_path_for(workspace_root: &Path) -> PathBuf {
    active_knowledge_paths(workspace_root).rag_index
}

#[cfg(test)]
pub(crate) fn lancedb_database_path_for(workspace_root: &Path) -> PathBuf {
    active_knowledge_paths(workspace_root).lancedb_database
}

pub(crate) fn memory_lancedb_root_for(workspace_root: &Path, project_id: &str) -> PathBuf {
    let digest = sha256_hex(project_id.as_bytes());
    workspace_root
        .join(".cindx")
        .join("memory-lancedb")
        .join(&digest[..24])
}

pub(crate) fn memory_lancedb_database_path_for(workspace_root: &Path, project_id: &str) -> PathBuf {
    memory_lancedb_root_for(workspace_root, project_id).join("index")
}

pub(crate) fn memory_lancedb_manifest_path_for(workspace_root: &Path, project_id: &str) -> PathBuf {
    memory_lancedb_root_for(workspace_root, project_id).join("manifest.json")
}

#[cfg(test)]
pub(crate) fn graph_store_path_for(workspace_root: &Path) -> PathBuf {
    active_knowledge_paths(workspace_root).graph_store
}

#[cfg(test)]
pub(crate) fn graph_state_for(
    workspace_root: &Path,
    focus_paths: &[String],
) -> Result<GraphStateView, String> {
    with_active_knowledge_paths(workspace_root, |paths| {
        graph_state_at_path(&paths.graph_store, focus_paths)
    })
}

#[cfg(test)]
pub(crate) fn graph_state_for_adapter(
    adapter: &FileRagAdapter,
    focus_paths: &[String],
) -> Result<GraphStateView, String> {
    let paths = knowledge_paths_for_rag_index(adapter.path());
    graph_state_at_path(&paths.graph_store, focus_paths)
}

#[cfg(test)]
pub(crate) fn graph_state_at_path(
    graph_path: &Path,
    focus_paths: &[String],
) -> Result<GraphStateView, String> {
    let store = open_graph_store_at_path(graph_path)?;
    Ok(graph_state_from_store(&store, focus_paths))
}

pub(crate) fn graph_state_from_store(
    store: &FileGraphStore,
    focus_paths: &[String],
) -> GraphStateView {
    let total_nodes = store.node_count();
    let total_edges = store.edge_count();
    let focus_paths = focus_paths
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut selected_ids = store
        .nodes_iter()
        .filter(|node| {
            if focus_paths.is_empty() {
                node.kind.label() == "file"
            } else {
                focus_paths.contains(node.label.as_str())
                    || focus_paths.contains(node.provenance.source_path.as_str())
            }
        })
        .take(if focus_paths.is_empty() { 18 } else { 40 })
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    if selected_ids.is_empty() {
        selected_ids.extend(store.nodes_iter().take(24).map(|node| node.id.clone()));
    }
    for _ in 0..2 {
        let frontier = selected_ids.clone();
        'edges: for edge in store
            .edges_iter()
            .filter(|edge| frontier.contains(&edge.from) || frontier.contains(&edge.to))
        {
            for id in [&edge.from, &edge.to] {
                if selected_ids.len() >= 80 {
                    break 'edges;
                }
                if !selected_ids.contains(id) {
                    selected_ids.insert(id.clone());
                }
            }
        }
    }
    let mut nodes = store
        .nodes_iter()
        .filter(|node| selected_ids.contains(&node.id))
        .map(|node| GraphNodeView {
            focused: focus_paths.contains(node.label.as_str())
                || focus_paths.contains(node.provenance.source_path.as_str()),
            id: node.id.clone(),
            kind: node.kind.label().to_string(),
            label: node.label.clone(),
            source_path: node.provenance.source_path.clone(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        right
            .focused
            .cmp(&left.focused)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.label.cmp(&right.label))
    });
    let visible_ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut edges = store
        .edges_iter()
        .filter(|edge| {
            visible_ids.contains(edge.from.as_str()) && visible_ids.contains(edge.to.as_str())
        })
        .map(|edge| GraphEdgeView {
            id: edge.id.clone(),
            from: edge.from.clone(),
            to: edge.to.clone(),
            kind: edge.kind.label().to_string(),
        })
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    edges.truncate(140);
    GraphStateView {
        total_nodes,
        total_edges,
        nodes,
        edges,
    }
}

pub(crate) fn empty_graph_state() -> GraphStateView {
    GraphStateView {
        total_nodes: 0,
        total_edges: 0,
        nodes: Vec::new(),
        edges: Vec::new(),
    }
}

pub(crate) fn graph_state_for_snapshot(
    snapshot: &WorkspaceKnowledgeSnapshot,
    focus_paths: &[String],
) -> GraphStateView {
    snapshot
        .graph_store
        .as_deref()
        .map(|store| graph_state_from_store(store, focus_paths))
        .unwrap_or_else(empty_graph_state)
}

pub(crate) fn context_checkpoint_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("context-checkpoint.md")
}

pub(crate) fn context_checkpoint_path_for_session(
    workspace_root: &Path,
    session_id: Option<&str>,
) -> PathBuf {
    let segment = session_id
        .filter(|value| !value.trim().is_empty())
        .map(safe_path_segment)
        .unwrap_or_else(|| "project".to_string());
    workspace_root
        .join(".cindx")
        .join("context")
        .join(format!("{segment}.md"))
}

pub(crate) fn context_checkpoint_manifest_path_for_session(
    workspace_root: &Path,
    session_id: Option<&str>,
) -> PathBuf {
    context_checkpoint_path_for_session(workspace_root, session_id).with_extension("coverage.json")
}

pub(crate) fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .take(160)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn agent_trace_export_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("agent-trace.jsonl")
}

pub(crate) fn open_rag_adapter_for(workspace_root: &Path) -> Result<FileRagAdapter, String> {
    #[cfg(test)]
    RAG_ADAPTER_OPEN_COUNT.with(|count| count.set(count.get().saturating_add(1)));
    open_active_knowledge_adapter(workspace_root)
}

pub(crate) fn workspace_knowledge_cache_key(workspace_root: &Path) -> String {
    fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf())
        .display()
        .to_string()
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceKnowledgeSnapshot {
    pub(crate) adapter: FileRagAdapter,
    pub(crate) graph_store: Option<Arc<FileGraphStore>>,
    pub(crate) cache_hit: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceKnowledgeStateSnapshot {
    pub(crate) active_index_path: PathBuf,
    pub(crate) stats: RagIndexStats,
    pub(crate) full: Option<WorkspaceKnowledgeSnapshot>,
}

impl WorkspaceKnowledgeCacheEntry {
    pub(crate) fn snapshot_if_current(
        &self,
        active_index_path: &Path,
    ) -> Option<WorkspaceKnowledgeSnapshot> {
        (self.validated_at.elapsed() <= WORKSPACE_KNOWLEDGE_CACHE_TTL
            && self.adapter.path() == active_index_path)
            .then(|| WorkspaceKnowledgeSnapshot {
                adapter: self.adapter.clone(),
                graph_store: self.graph_store.clone(),
                cache_hit: true,
            })
    }
}

#[cfg(test)]
std::thread_local! {
    static GRAPH_STORE_OPEN_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static RAG_ADAPTER_OPEN_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn open_graph_store_at_path(graph_path: &Path) -> Result<FileGraphStore, String> {
    #[cfg(test)]
    GRAPH_STORE_OPEN_COUNT.with(|count| count.set(count.get().saturating_add(1)));
    FileGraphStore::open(graph_path).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn reset_graph_store_open_count() {
    GRAPH_STORE_OPEN_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn graph_store_open_count() -> u64 {
    GRAPH_STORE_OPEN_COUNT.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn reset_rag_adapter_open_count() {
    RAG_ADAPTER_OPEN_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn rag_adapter_open_count() -> u64 {
    RAG_ADAPTER_OPEN_COUNT.with(std::cell::Cell::get)
}

fn graph_store_for_adapter(
    adapter: &FileRagAdapter,
) -> Result<Option<Arc<FileGraphStore>>, String> {
    let graph_path = knowledge_paths_for_rag_index(adapter.path()).graph_store;
    graph_path
        .exists()
        .then(|| open_graph_store_at_path(&graph_path).map(Arc::new))
        .transpose()
}

pub(crate) fn workspace_knowledge_cache_entry(
    adapter: &FileRagAdapter,
) -> Result<WorkspaceKnowledgeCacheEntry, String> {
    Ok(WorkspaceKnowledgeCacheEntry {
        adapter: adapter.clone(),
        graph_store: graph_store_for_adapter(adapter)?,
        validated_at: Instant::now(),
    })
}

pub(crate) fn cached_workspace_knowledge_snapshot_for(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<WorkspaceKnowledgeSnapshot, String> {
    let key = workspace_knowledge_cache_key(workspace_root);
    let active_index_path = rag_index_path_for(workspace_root);
    let cached = {
        let cache = state
            .workspace_knowledge_cache
            .lock()
            .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?;
        cache
            .get(&key)
            .and_then(|entry| entry.snapshot_if_current(&active_index_path))
    };
    if let Some(snapshot) = cached {
        return Ok(snapshot);
    }
    let adapter = open_rag_adapter_for(workspace_root)?;
    let graph_store = graph_store_for_adapter(&adapter)?;
    Ok(WorkspaceKnowledgeSnapshot {
        adapter,
        graph_store,
        cache_hit: false,
    })
}

pub(crate) fn active_workspace_knowledge_state_snapshot_for(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<WorkspaceKnowledgeStateSnapshot, String> {
    active_workspace_knowledge_state_snapshot_in(&state.workspace_knowledge_cache, workspace_root)
}

pub(crate) fn active_workspace_knowledge_state_snapshot_in(
    cache: &Mutex<BTreeMap<String, WorkspaceKnowledgeCacheEntry>>,
    workspace_root: &Path,
) -> Result<WorkspaceKnowledgeStateSnapshot, String> {
    with_workspace_generation_read(workspace_root, || {
        let active_index_path = active_knowledge_paths(workspace_root).rag_index;
        let full = cache
            .lock()
            .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?
            .get(&workspace_knowledge_cache_key(workspace_root))
            .and_then(|entry| entry.snapshot_if_current(&active_index_path));
        let stats = match full.as_ref() {
            Some(snapshot) => snapshot.adapter.stats().clone(),
            None => match read_file_rag_stats(&active_index_path)
                .map_err(|error| error.to_string())?
            {
                Some(stats) => stats,
                None => {
                    #[cfg(test)]
                    RAG_ADAPTER_OPEN_COUNT.with(|count| count.set(count.get().saturating_add(1)));
                    FileRagAdapter::open(&active_index_path)
                        .map_err(|error| error.to_string())?
                        .stats()
                        .clone()
                }
            },
        };
        Ok(WorkspaceKnowledgeStateSnapshot {
            active_index_path,
            stats,
            full,
        })
    })
}

pub(crate) fn cache_rag_adapter(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    adapter: &FileRagAdapter,
) -> Result<(), String> {
    cache_rag_adapter_in(&state.workspace_knowledge_cache, workspace_root, adapter).map(|_| ())
}

pub(crate) fn cache_rag_adapter_in(
    cache: &Mutex<BTreeMap<String, WorkspaceKnowledgeCacheEntry>>,
    workspace_root: &Path,
    adapter: &FileRagAdapter,
) -> Result<bool, String> {
    let key = workspace_knowledge_cache_key(workspace_root);
    with_workspace_generation_read(workspace_root, || {
        if adapter.path() != rag_index_path_for(workspace_root) {
            return Ok(false);
        }
        let entry = workspace_knowledge_cache_entry(adapter)?;
        let mut cache = cache
            .lock()
            .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?;
        cache.retain(|_, entry| entry.validated_at.elapsed() <= WORKSPACE_KNOWLEDGE_CACHE_TTL);
        if !cache.contains_key(&key) && cache.len() >= WORKSPACE_KNOWLEDGE_CACHE_MAX_ENTRIES {
            let oldest = cache
                .iter()
                .max_by_key(|(_, entry)| entry.validated_at.elapsed())
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                cache.remove(&oldest);
            }
        }
        cache.insert(key, entry);
        Ok(true)
    })
}

pub(crate) fn invalidate_workspace_knowledge_cache(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<(), String> {
    state
        .workspace_knowledge_cache
        .lock()
        .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?
        .remove(&workspace_knowledge_cache_key(workspace_root));
    Ok(())
}

#[cfg(test)]
pub(crate) fn index_graph_chunks(
    workspace_root: &Path,
    chunks: &[RagChunk],
) -> Result<(usize, usize), String> {
    index_graph_chunks_cancellable(workspace_root, chunks, || false)
}

#[cfg(test)]
pub(crate) fn index_graph_chunks_cancellable(
    workspace_root: &Path,
    chunks: &[RagChunk],
    mut should_cancel: impl FnMut() -> bool,
) -> Result<(usize, usize), String> {
    let graph_path = legacy_knowledge_paths(workspace_root).graph_store;
    let file_name = graph_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("graph.tsv");
    let temporary_path =
        graph_path.with_file_name(format!(".{file_name}.{}.tmp", unique_id("graph-index")));
    let result = (|| {
        let mut graph_store =
            FileGraphStore::open(&temporary_path).map_err(|error| error.to_string())?;
        let mut cancelled = false;
        {
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
        }
        if cancelled || should_cancel() {
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        let counts = (graph_store.node_count(), graph_store.edge_count());
        drop(graph_store);
        fs::rename(&temporary_path, &graph_path)
            .map_err(|error| format!("failed to replace graph store: {error}"))?;
        Ok(counts)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(crate) fn workspace_root() -> PathBuf {
    if let Some(root) = runtime_path_from_env("CINDX_DEFAULT_WORKSPACE") {
        if root.is_dir() {
            return root;
        }
    }
    if let Some(home) = user_home_directory() {
        if home.is_dir() {
            return home;
        }
    }
    std::env::current_dir()
        .ok()
        .filter(|path| path.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

pub(crate) fn app_data_root() -> PathBuf {
    app_data_root_for(
        runtime_path_from_env("CINDX_DATA_DIR"),
        user_home_directory(),
    )
}

pub(crate) fn app_data_root_for(override_root: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    if let Some(root) = override_root {
        return root;
    }
    if let Some(home) = home {
        #[cfg(target_os = "macos")]
        return home
            .join("Library")
            .join("Application Support")
            .join("Cindx");
        #[cfg(not(target_os = "macos"))]
        return home.join(".cindx");
    }
    std::env::temp_dir().join("Cindx")
}

pub(crate) fn runtime_path_from_env(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir()
            .ok()
            .map(|current| current.join(path))
    }
}

pub(crate) fn user_home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn migrate_legacy_app_data() -> Result<(), std::io::Error> {
    if runtime_path_from_env("CINDX_DATA_DIR").is_some() {
        return Ok(());
    }
    let source = development_repo_root().join(".cindx");
    let destination = app_data_root();
    if source == destination || !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(&destination)?;
    secure_directory(&destination)?;
    for file_name in [
        "state.sqlite3",
        "state.sqlite3-shm",
        "state.sqlite3-wal",
        "provider.conf",
        "workspace.conf",
        "projects.conf",
        "schedules.json",
        "sidecars.conf",
        "mcp-servers.json",
        "mcp-catalog.json",
    ] {
        let source_path = source.join(file_name);
        let destination_path = destination.join(file_name);
        if source_path.is_file() && !destination_path.exists() {
            fs::copy(&source_path, &destination_path)?;
            secure_private_file(&destination_path)?;
        }
    }
    Ok(())
}

pub(crate) fn secure_directory(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(crate) fn secure_private_file(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub(crate) fn append_startup_log(message: &str) {
    let root = app_data_root();
    if fs::create_dir_all(&root).is_err() {
        return;
    }
    let _ = secure_directory(&root);
    let path = root.join("startup.log");
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let Ok(mut file) = options.open(&path) else {
        return;
    };
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let _ = writeln!(file, "{timestamp_ms} {message}");
    let _ = secure_private_file(&path);
}

pub(crate) fn install_startup_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        append_startup_log(&format!("panic: {info}"));
        previous(info);
    }));
}
