use crate::desktop_prelude::*;
use crate::{
    agent_query_commands::append_agent_progress_event,
    app_state::AppState,
    configuration_models::ProviderConfig,
    event_projection::write_private_file_atomically,
    knowledge_runtime::{
        prepare_agent_knowledge_context, should_run_agent_knowledge_retrieval, CloudRagEmbedder,
    },
    persistence_runtime::{
        current_time_millis, memory_lancedb_database_path_for, memory_lancedb_manifest_path_for,
        metadata_with_context, open_app_read_store, phase16_task_id, skill_catalog_for_root,
    },
    runtime_constants::{
        AGENT_MEMORY_MAX_RECORDS, AGENT_MEMORY_READ_MODEL_NAMESPACE, AGENT_MEMORY_RECALL_LIMIT,
        MEMORY_VECTOR_FALLBACK_RETRY_MS, MEMORY_VECTOR_MANIFEST_SCHEMA,
        MEMORY_VECTOR_REFRESH_INFLIGHT,
    },
    tool_execution::append_event,
    view_models::MemoryStatsView,
};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryVectorManifest {
    pub(crate) schema: String,
    pub(crate) projection_sha256: String,
    pub(crate) record_count: usize,
    pub(crate) embedding_backend: String,
    pub(crate) embedding_provider: String,
    pub(crate) embedding_model: String,
    pub(crate) embedding_dimensions: usize,
    pub(crate) generated_at_ms: u64,
}

pub(crate) fn load_project_memory_ledger(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<MemoryLedger, StorageError> {
    load_project_memory_ledger_inner(store, project_id, true)
}

pub(crate) fn load_project_memory_ledger_snapshot(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<MemoryLedger, StorageError> {
    load_project_memory_ledger_inner(store, project_id, false)
}

fn load_project_memory_ledger_inner(
    store: &mut SqliteStore,
    project_id: &str,
    persist: bool,
) -> Result<MemoryLedger, StorageError> {
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
    let mut rebuilding = stored.is_none();
    let mut ledger = stored.unwrap_or_else(|| MemoryLedger::new(project_id));
    let mut delta = store.list_by_task_and_metadata_after(
        &task_id,
        "project_id",
        project_id,
        ledger.revision,
    )?;
    if ledger.event_count.saturating_add(delta.len() as u64) != revision.event_count {
        rebuilding = true;
        ledger = MemoryLedger::new(project_id);
        delta = store.list_by_task_and_metadata_after(&task_id, "project_id", project_id, 0)?;
    }

    let checkpointed_runs = if rebuilding {
        let mut runs = BTreeMap::<String, Vec<Event>>::new();
        for event in &delta {
            if event.metadata.get("project_id").map(String::as_str) != Some(project_id) {
                continue;
            }
            if let Some(run_id) = event.metadata.get("agent_run_id") {
                runs.entry(run_id.clone()).or_default().push(event.clone());
            }
        }
        runs.into_iter()
            .filter(|(_, events)| events.iter().any(is_memory_checkpoint_event))
            .map(|(run_id, events)| (run_id, Some(events)))
            .collect::<Vec<_>>()
    } else {
        delta
            .iter()
            .filter(|event| is_memory_checkpoint_event(event))
            .filter(|event| {
                event.metadata.get("project_id").map(String::as_str) == Some(project_id)
            })
            .filter_map(|event| {
                event
                    .metadata
                    .get("agent_run_id")
                    .cloned()
                    .map(|run_id| (run_id, None))
            })
            .collect::<Vec<_>>()
    };
    for (run_id, cached_events) in checkpointed_runs {
        let events = match cached_events {
            Some(events) => events,
            None => store.list_by_task_and_metadata(&task_id, "agent_run_id", &run_id)?,
        };
        let Some(session_id) = events
            .iter()
            .find_map(|event| event.metadata.get("session_id"))
        else {
            continue;
        };
        merge_memory_records(
            &mut ledger,
            extract_durable_memories(&events, project_id, session_id),
            AGENT_MEMORY_MAX_RECORDS,
        );
    }
    ledger.revision = revision.latest_sequence;
    ledger.event_count = revision.event_count;
    if persist {
        save_project_memory_ledger(store, &ledger)?;
    }
    Ok(ledger)
}

fn is_memory_checkpoint_event(event: &Event) -> bool {
    matches!(
        event.summary.as_str(),
        "Agent task completed" | "Agent task paused" | "Agent task failed" | "Agent task cancelled"
    )
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
    )
}

fn refresh_project_memory_ledger_revision(
    store: &mut SqliteStore,
    task_id: &TaskId,
    ledger: &mut MemoryLedger,
) -> Result<(), StorageError> {
    let revision = store.event_revision_by_metadata(task_id, "project_id", &ledger.project_id)?;
    ledger.revision = revision.latest_sequence;
    ledger.event_count = revision.event_count;
    Ok(())
}

pub(crate) fn active_project_id_for_memory(
    state: &tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    Ok(state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .active_project()
        .map(|project| project.id.clone()))
}

pub(crate) fn project_memory_stats(
    store: &mut SqliteStore,
    project_id: Option<&str>,
) -> Result<MemoryStatsView, StorageError> {
    let Some(project_id) = project_id else {
        return Ok(MemoryStatsView::default());
    };
    let ledger = load_project_memory_ledger(store, project_id)?;
    Ok(MemoryStatsView {
        records: ledger.records.len(),
        requirements: ledger
            .records
            .iter()
            .filter(|record| record.kind == MemoryKind::Requirement)
            .count(),
        outcomes: ledger
            .records
            .iter()
            .filter(|record| record.kind == MemoryKind::Outcome)
            .count(),
        evidence: ledger
            .records
            .iter()
            .filter(|record| record.kind == MemoryKind::Evidence)
            .count(),
        recalls: ledger
            .records
            .iter()
            .map(|record| record.recall_count)
            .sum(),
        observed_uses: ledger
            .records
            .iter()
            .map(|record| record.observed_use_count)
            .sum(),
        updated_at_ms: ledger
            .records
            .iter()
            .map(|record| record.updated_at_ms)
            .max()
            .unwrap_or_default(),
    })
}

pub(crate) fn should_recall_agent_memory(context: &RoutingContext, prompt: &str) -> bool {
    if context.needs_tools
        || context.needs_retrieval
        || context.needs_multi_model
        || context.complexity_score >= 2
    {
        return true;
    }
    let normalized = prompt.to_ascii_lowercase();
    [
        "remember",
        "previous",
        "last time",
        "continue",
        "之前",
        "上次",
        "刚才",
        "继续",
        "还记得",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[derive(Debug, Default)]
pub(crate) struct PreparedRunKnowledgeContexts {
    pub(crate) memory: Option<Message>,
    pub(crate) workspace: Option<Message>,
}

pub(crate) fn append_prepared_memory_context(
    run_context: &mut Metadata,
    history: &mut Vec<Message>,
    memory_context: Option<Message>,
) {
    let Some(memory_context) = memory_context else {
        return;
    };
    if let Some(memory_ids) = memory_context.metadata.get("memory_ids") {
        run_context.insert("memory_ids".to_string(), memory_ids.clone());
    }
    if let Some(selected_count) = memory_context.metadata.get("selected_count") {
        run_context.insert("memory_selected_count".to_string(), selected_count.clone());
    }
    history.push(memory_context);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_run_knowledge_contexts(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    workspace_root: &Path,
    config: &ProviderConfig,
    prompt: &str,
    routing_context: &RoutingContext,
    retrieval_mode: &str,
    cancellation: &Arc<AgentRunControl>,
) -> Result<PreparedRunKnowledgeContexts, String> {
    let recall_memory = should_recall_agent_memory(routing_context, prompt);
    let retrieve_workspace = should_run_agent_knowledge_retrieval(routing_context);
    if !recall_memory && !retrieve_workspace {
        return Ok(PreparedRunKnowledgeContexts::default());
    }
    if recall_memory {
        cancellation.mark_progress("memory", "Recalling relevant project memory");
    }
    if retrieve_workspace {
        cancellation.mark_progress("retrieval", "Preparing workspace knowledge");
        append_agent_progress_event(state, task_id, run_context, "Preparing workspace knowledge")?;
    }

    let (memory_result, workspace_result) = std::thread::scope(|scope| {
        let memory_handle = recall_memory.then(|| {
            scope.spawn(|| {
                recall_project_memory_for_prompt(
                    state,
                    task_id,
                    run_context,
                    workspace_root,
                    config,
                    prompt,
                    cancellation,
                )
            })
        });
        let workspace_handle = retrieve_workspace.then(|| {
            scope.spawn(|| {
                prepare_agent_knowledge_context(
                    state,
                    config,
                    task_id,
                    run_context,
                    workspace_root,
                    prompt,
                    retrieval_mode,
                    cancellation,
                )
            })
        });
        (
            memory_handle.map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("project memory worker panicked".to_string()))
            }),
            workspace_handle.map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("workspace retrieval worker panicked".to_string()))
            }),
        )
    });

    let memory = match memory_result {
        Some(Ok(context)) => context,
        Some(Err(error)) if error == MODEL_REQUEST_CANCELLED => return Err(error),
        Some(Err(error)) => {
            eprintln!("project memory recall unavailable: {error}");
            None
        }
        None => None,
    };
    let workspace = match workspace_result {
        Some(Ok(context)) => context,
        Some(Err(error)) if error == MODEL_REQUEST_CANCELLED => return Err(error),
        Some(Err(error)) => {
            let mut store = state
                .store
                .lock()
                .map_err(|lock_error| format!("store lock poisoned: {lock_error}"))?;
            append_event(
                &mut store,
                task_id,
                EventKind::Error,
                "Workspace knowledge retrieval unavailable",
                metadata_with_context(
                    [("error".to_string(), error)].into_iter().collect(),
                    run_context,
                ),
            )
            .map_err(|store_error| store_error.to_string())?;
            None
        }
        None => None,
    };

    Ok(PreparedRunKnowledgeContexts { memory, workspace })
}

pub(crate) fn append_skill_context_for_run(
    workspace_root: &Path,
    prompt: &str,
    history: &mut Vec<Message>,
) -> Result<(), String> {
    let Some(skill_context) = skill_catalog_for_root(workspace_root)
        .context_for_prompt(prompt)
        .map_err(|error| format!("skill context preparation failed: {error}"))?
    else {
        return Ok(());
    };
    history.push(Message {
        role: MessageRole::System,
        content: skill_context,
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), "skill_context".to_string()),
        ]
        .into_iter()
        .collect(),
    });
    Ok(())
}

pub(crate) fn memory_vector_projection_sha256(ledger: &MemoryLedger) -> String {
    let mut records = ledger
        .records
        .iter()
        .map(|record| format!("{}:{}", record.id, record.fingerprint))
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

pub(crate) fn memory_rag_index(ledger: &MemoryLedger) -> RagIndex {
    let indexed_at_ms = current_time_millis();
    let chunks = ledger
        .records
        .iter()
        .map(|record| {
            let embedding = local_query_embedding(&record.content);
            RagChunk {
                id: record.id.clone(),
                path: format!(
                    "memory://{}/{}/{}",
                    ledger.project_id,
                    record.kind.label(),
                    record.id
                ),
                file_hash: record.fingerprint.clone(),
                modified_time_ms: record.updated_at_ms,
                start_line: 1,
                end_line: 1,
                indexed_at_ms,
                text: record.content.clone(),
                embedding_dimensions: embedding.len(),
                embedding,
                embedding_provider: "local".to_string(),
                embedding_model: "local-hash".to_string(),
            }
        })
        .collect::<Vec<_>>();
    RagIndex {
        stats: RagIndexStats {
            files_indexed: chunks.len(),
            chunks_indexed: chunks.len(),
            indexed_at_ms,
        },
        chunks,
    }
}

pub(crate) fn refresh_project_memory_vector_index(
    workspace_root: &Path,
    config: &ProviderConfig,
    ledger: &MemoryLedger,
) -> Result<Option<String>, String> {
    let projection_sha256 = memory_vector_projection_sha256(ledger);
    let database_path = memory_lancedb_database_path_for(workspace_root, &ledger.project_id);
    let manifest_path = memory_lancedb_manifest_path_for(workspace_root, &ledger.project_id);
    let now_ms = current_time_millis();
    if lancedb_index_exists(&database_path) {
        if let Some(manifest) = load_memory_vector_manifest(&manifest_path)? {
            if memory_vector_manifest_matches(&manifest, &projection_sha256, config, now_ms) {
                return Ok(None);
            }
        }
    }

    let mut index = memory_rag_index(ledger);
    let mut embedding_backend = "local".to_string();
    let mut fallback_error = None;
    if config.is_ready() && !index.chunks.is_empty() {
        let mut embedder = CloudRagEmbedder {
            config: config.clone(),
            cancellation: None,
        };
        match apply_embeddings_to_index_cancellable(&mut index, &mut embedder, || false) {
            Ok(()) => embedding_backend = "cloud".to_string(),
            Err(error) => {
                embedding_backend = "local-fallback".to_string();
                fallback_error = Some(error.to_string());
            }
        }
    }
    replace_lancedb_index(&database_path, &index).map_err(|error| error.to_string())?;
    let (embedding_provider, embedding_model, embedding_dimensions) = index
        .chunks
        .first()
        .map(|chunk| {
            (
                chunk.embedding_provider.clone(),
                chunk.embedding_model.clone(),
                chunk.embedding_dimensions,
            )
        })
        .unwrap_or_else(|| ("local".to_string(), "local-hash".to_string(), 0));
    let manifest = MemoryVectorManifest {
        schema: MEMORY_VECTOR_MANIFEST_SCHEMA.to_string(),
        projection_sha256,
        record_count: index.chunks.len(),
        embedding_backend,
        embedding_provider,
        embedding_model,
        embedding_dimensions,
        generated_at_ms: now_ms,
    };
    let payload = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("failed to encode memory vector manifest: {error}"))?;
    write_private_file_atomically(&manifest_path, &payload, "memory vector manifest")?;
    Ok(fallback_error)
}

pub(crate) fn schedule_project_memory_vector_refresh(
    workspace_root: PathBuf,
    config: ProviderConfig,
    ledger: MemoryLedger,
) {
    if ledger.records.is_empty() {
        return;
    }
    let projection_sha256 = memory_vector_projection_sha256(&ledger);
    let target_model = if config.is_ready() {
        config.model_for_role(&ModelRole::Embedder)
    } else {
        "local-hash".to_string()
    };
    let key = format!(
        "{}:{}:{}",
        workspace_root.display(),
        projection_sha256,
        target_model
    );
    let inflight = MEMORY_VECTOR_REFRESH_INFLIGHT.get_or_init(|| Mutex::new(BTreeSet::new()));
    let Ok(Some(inflight_lease)) =
        ExclusiveKeyLease::try_acquire(inflight, key, "memory vector refresh inflight")
    else {
        return;
    };
    tauri::async_runtime::spawn_blocking(move || {
        let _inflight_lease = inflight_lease;
        match refresh_project_memory_vector_index(&workspace_root, &config, &ledger) {
            Ok(Some(error)) => {
                eprintln!(
                    "project memory cloud embedding unavailable; using local vectors: {error}"
                )
            }
            Ok(None) => {}
            Err(error) => eprintln!("project memory vector refresh unavailable: {error}"),
        }
    });
}

pub(crate) fn project_memory_semantic_scores(
    workspace_root: &Path,
    project_id: &str,
    ledger: &MemoryLedger,
    config: &ProviderConfig,
    prompt: &str,
    cancellation: &Arc<AgentRunControl>,
) -> Result<(BTreeMap<String, f64>, MemoryVectorManifest), String> {
    let database_path = memory_lancedb_database_path_for(workspace_root, project_id);
    let manifest_path = memory_lancedb_manifest_path_for(workspace_root, project_id);
    let manifest = load_memory_vector_manifest(&manifest_path)?
        .ok_or_else(|| "memory vector index is not ready".to_string())?;
    if !lancedb_index_exists(&database_path)
        || !memory_vector_manifest_matches(
            &manifest,
            &memory_vector_projection_sha256(ledger),
            config,
            current_time_millis(),
        )
    {
        return Err("memory vector index is stale and is rebuilding".to_string());
    }
    let query_embedding = if manifest.embedding_backend == "cloud" {
        let mut embedder = CloudRagEmbedder {
            config: config.clone(),
            cancellation: Some(cancellation.clone()),
        };
        embedder
            .embed_texts(&[prompt.to_string()])
            .map_err(|error| error.to_string())?
            .vectors
            .into_iter()
            .next()
            .ok_or_else(|| "memory embedding provider returned no query vector".to_string())?
    } else {
        local_query_embedding(prompt)
    };
    if query_embedding.len() != manifest.embedding_dimensions {
        return Err(format!(
            "memory query embedding has {} dimensions, expected {}",
            query_embedding.len(),
            manifest.embedding_dimensions
        ));
    }
    let results = search_lancedb_index(
        &database_path,
        &query_embedding,
        AGENT_MEMORY_RECALL_LIMIT.saturating_mul(4),
    )
    .map_err(|error| error.to_string())?;
    Ok((
        results
            .into_iter()
            .map(|result| (result.chunk.id, f64::from(result.score)))
            .collect(),
        manifest,
    ))
}

pub(crate) fn recall_project_memory_for_prompt(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    workspace_root: &Path,
    config: &ProviderConfig,
    prompt: &str,
    cancellation: &Arc<AgentRunControl>,
) -> Result<Option<Message>, String> {
    let Some(project_id) = run_context.get("project_id") else {
        return Ok(None);
    };
    let session_id = run_context.get("session_id").map(String::as_str);
    let started_at = Instant::now();
    let now_ms = current_time_millis();
    let ledger = {
        let mut store = open_app_read_store()?;
        load_project_memory_ledger_snapshot(&mut store, project_id)
            .map_err(|error| error.to_string())?
    };
    if ledger.records.is_empty() {
        return Ok(None);
    }
    schedule_project_memory_vector_refresh(
        workspace_root.to_path_buf(),
        config.clone(),
        ledger.clone(),
    );
    let lexical_recalls = recall_memories_at(
        &ledger,
        prompt,
        session_id,
        AGENT_MEMORY_RECALL_LIMIT.saturating_mul(2),
        now_ms,
    );
    let (semantic_scores, vector_manifest, vector_error) = match project_memory_semantic_scores(
        workspace_root,
        project_id,
        &ledger,
        config,
        prompt,
        cancellation,
    ) {
        Ok((scores, manifest)) => (scores, Some(manifest), None),
        Err(error) => (BTreeMap::new(), None, Some(error)),
    };
    let mut recalls = fuse_memory_recalls_at(
        &ledger,
        lexical_recalls,
        &semantic_scores,
        session_id,
        AGENT_MEMORY_RECALL_LIMIT.saturating_mul(2),
        now_ms,
    );
    if let Some(session_id) = session_id {
        recalls.retain(|recall| {
            recall
                .record
                .source_session_ids
                .iter()
                .any(|source| source != session_id)
        });
    }
    recalls.truncate(AGENT_MEMORY_RECALL_LIMIT);
    if recalls.is_empty() {
        return Ok(None);
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let mut ledger =
        load_project_memory_ledger(&mut store, project_id).map_err(|error| error.to_string())?;
    recalls.retain_mut(|recall| {
        let Some(current) = ledger
            .records
            .iter()
            .find(|record| record.id == recall.record.id)
        else {
            return false;
        };
        recall.record = current.clone();
        true
    });
    if recalls.is_empty() {
        return Ok(None);
    }
    record_memory_recalls(&mut ledger, &recalls, now_ms);
    let mut metadata = [
        ("action".to_string(), "memory_recall".to_string()),
        ("query".to_string(), prompt.to_string()),
        (
            "retrieval_mode".to_string(),
            if semantic_scores.is_empty() {
                "project_memory_lexical"
            } else {
                "project_memory_hybrid"
            }
            .to_string(),
        ),
        ("selected_count".to_string(), recalls.len().to_string()),
        (
            "semantic_candidate_count".to_string(),
            semantic_scores.len().to_string(),
        ),
        (
            "memory_ids".to_string(),
            recalls
                .iter()
                .map(|recall| recall.record.id.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ),
        (
            "memory_reasons".to_string(),
            recalls
                .iter()
                .map(|recall| format!("{}={}", recall.record.id, recall.reasons.join("+")))
                .collect::<Vec<_>>()
                .join(";"),
        ),
        (
            "duration_ms".to_string(),
            started_at.elapsed().as_millis().to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(manifest) = vector_manifest {
        metadata.insert(
            "memory_embedding_backend".to_string(),
            manifest.embedding_backend,
        );
        metadata.insert(
            "memory_embedding_model".to_string(),
            manifest.embedding_model,
        );
    }
    if let Some(error) = vector_error {
        metadata.insert(
            "memory_vector_fallback_error".to_string(),
            truncate_for_collaboration(&error, 320),
        );
    }
    metadata = metadata_with_context(metadata, run_context);
    append_event(
        &mut store,
        task_id,
        EventKind::RetrievalPerformed,
        "Project memory recalled",
        metadata,
    )
    .map_err(|error| error.to_string())?;
    refresh_project_memory_ledger_revision(&mut store, task_id, &mut ledger)
        .map_err(|error| error.to_string())?;
    save_project_memory_ledger(&mut store, &ledger).map_err(|error| error.to_string())?;

    Ok(Some(Message {
        role: MessageRole::System,
        content: memory_recalls_to_markdown(&recalls),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), "project_memory".to_string()),
            (
                "context_source_schema".to_string(),
                agent_runtime::CONTEXT_SOURCE_SCHEMA.to_string(),
            ),
            ("selected_count".to_string(), recalls.len().to_string()),
            (
                "memory_ids".to_string(),
                recalls
                    .iter()
                    .map(|recall| recall.record.id.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        ]
        .into_iter()
        .collect(),
    }))
}

pub(crate) fn refresh_project_memory_after_run(
    store: &mut SqliteStore,
    run_context: &Metadata,
) -> Result<Option<MemoryLedger>, StorageError> {
    let Some(project_id) = run_context.get("project_id") else {
        return Ok(None);
    };
    load_project_memory_ledger(store, project_id).map(Some)
}

pub(crate) fn record_project_memory_observed_use(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    output: &str,
) -> Result<usize, StorageError> {
    let (Some(project_id), Some(memory_ids)) =
        (run_context.get("project_id"), run_context.get("memory_ids"))
    else {
        return Ok(0);
    };
    let memory_ids = memory_ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if memory_ids.is_empty() {
        return Ok(0);
    }
    let observed_at_ms = current_time_millis();
    let mut ledger = load_project_memory_ledger(store, project_id)?;
    let used_ids = record_memory_observed_uses(&mut ledger, &memory_ids, output, observed_at_ms);
    append_event(
        store,
        task_id,
        EventKind::RetrievalPerformed,
        "Project memory utilization measured",
        metadata_with_context(
            [
                ("action".to_string(), "memory_use".to_string()),
                ("selected_count".to_string(), memory_ids.len().to_string()),
                ("used_count".to_string(), used_ids.len().to_string()),
                ("used_memory_ids".to_string(), used_ids.join(",")),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )?;
    refresh_project_memory_ledger_revision(store, task_id, &mut ledger)?;
    save_project_memory_ledger(store, &ledger)?;
    Ok(used_ids.len())
}
