use super::*;
use crate::knowledge_generation_runtime::{
    build_and_publish_knowledge_generation_with_commit,
    with_workspace_knowledge_index_lock_cancellable, PublishedKnowledgeGeneration,
};

#[tauri::command]
pub(crate) fn get_phase7_state(state: tauri::State<'_, AppState>) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let project_id = active_project_id_for_memory(&state)?;
    let knowledge = active_workspace_knowledge_state_snapshot_for(&state, &root)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let (graph, graph_summary_index_path) = match knowledge.full.as_ref() {
        Some(snapshot) => (graph_state_for_snapshot(snapshot, &[]), None),
        None => (
            empty_graph_state(),
            Some(knowledge.active_index_path.as_path()),
        ),
    };
    let memory = project_memory_stats(&mut store, project_id.as_deref())
        .map_err(|error| error.to_string())?;

    phase7_state(
        &store,
        &knowledge.stats,
        memory,
        Vec::new(),
        None,
        graph,
        graph_summary_index_path,
        None,
        None,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn ensure_workspace_knowledge(
    app: tauri::AppHandle,
) -> Result<Phase7State, String> {
    tauri::async_runtime::spawn_blocking(move || {
        ensure_workspace_knowledge_blocking(app.state::<AppState>())
    })
    .await
    .map_err(|error| format!("workspace knowledge refresh failed to join: {error}"))?
}

pub(crate) fn ensure_workspace_knowledge_blocking(
    state: tauri::State<'_, AppState>,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let project_id = active_project_id_for_memory(&state)?;
    let config = clone_provider_config(&state)?;
    let mut snapshot = cached_workspace_knowledge_snapshot_for(&state, &root)?;
    let cache_hit = snapshot.cache_hit;
    let cancellation = Arc::new(AgentRunControl::new("auto"));
    let expected_epoch = cancellation.steer_epoch();
    let indexed = ensure_workspace_knowledge_index(WorkspaceKnowledgeIndexRequest {
        workspace_root: &root,
        adapter: &mut snapshot.adapter,
        cache_hit,
        config: &config,
        cancellation: &cancellation,
        expected_epoch,
        resource_checkpoint: None,
        rag_operation: None,
    })?;
    if workspace_knowledge_cache_needs_refresh(cache_hit, indexed.is_some()) {
        cache_rag_adapter(&state, &root, &snapshot.adapter)?;
        snapshot = cached_workspace_knowledge_snapshot_for(&state, &root)?;
    }

    let graph = graph_state_for_snapshot(&snapshot, &[]);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let memory = project_memory_stats(&mut store, project_id.as_deref())
        .map_err(|error| error.to_string())?;

    phase7_state(
        &store,
        snapshot.adapter.stats(),
        memory,
        Vec::new(),
        None,
        graph,
        None,
        None,
        None,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn index_workspace_with_cloud_fallback(
    root: &Path,
    options: IndexOptions,
    embedder: &mut impl RagEmbedder,
    configured_model: &str,
) -> Result<(RagIndex, String, String, Option<String>), RagError> {
    let mut index = index_workspace(root, options)?;
    match apply_embeddings_to_index_cancellable(&mut index, embedder, || false) {
        Ok(()) => {
            let model = index
                .chunks
                .first()
                .map(|chunk| chunk.embedding_model.clone())
                .unwrap_or_else(|| configured_model.to_string());
            Ok((index, "cloud".to_string(), model, None))
        }
        Err(cloud_error) => {
            let model = index
                .chunks
                .first()
                .map(|chunk| chunk.embedding_model.clone())
                .unwrap_or_else(|| "local-hash".to_string());
            Ok((
                index,
                "local-fallback".to_string(),
                model,
                Some(cloud_error.to_string()),
            ))
        }
    }
}

#[tauri::command]
pub(crate) async fn index_workspace_rag(
    app: tauri::AppHandle,
    input: RagOperationInput,
) -> Result<Phase7State, String> {
    tauri::async_runtime::spawn_blocking(move || index_workspace_rag_blocking(&app, input))
        .await
        .map_err(|error| format!("workspace indexing failed to join: {error}"))?
}

pub(crate) fn index_workspace_rag_blocking(
    app: &tauri::AppHandle,
    input: RagOperationInput,
) -> Result<Phase7State, String> {
    let state = app.state::<AppState>();
    run_rag_operation(
        &state.rag_operation_controls,
        app,
        &input.operation_id,
        "index",
        4,
        |cancellation, progress| index_workspace_rag_operation(&state, cancellation, progress),
        |output| output.last_error.clone(),
    )
}

fn index_workspace_rag_operation(
    state: &tauri::State<'_, AppState>,
    cancellation: &Arc<RagOperationControl>,
    progress: &mut RagOperationProgressReporter<'_>,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(state)?;
    let project_id = active_project_id_for_memory(state)?;
    let config = clone_provider_config(state)?;
    let expected_epoch = cancellation.agent().steer_epoch();
    let (published, embedding_backend, embedding_model, embedding_fallback_error) =
        with_workspace_knowledge_index_lock_cancellable(
            &root,
            || cancellation.should_cancel(),
            || {
                rag_operation_checkpoint(cancellation)?;
                progress.advance("indexing", 0, "Indexing workspace knowledge");
                let (index, backend, model, fallback_error) = if config.is_ready() {
                    let configured_model = config.model_for_role(&ModelRole::Embedder);
                    let mut embedder = CloudRagEmbedder {
                        config: config.clone(),
                        cancellation: Some(Arc::clone(cancellation.agent())),
                        expected_steer_epoch: Some(expected_epoch),
                        resource_checkpoint: None,
                    };
                    index_workspace_with_cloud_fallback_cancellable(
                        &root,
                        IndexOptions::default(),
                        &mut embedder,
                        &configured_model,
                        || cancellation.should_cancel(),
                    )?
                } else {
                    let index = index_workspace_cancellable(&root, IndexOptions::default(), || {
                        cancellation.should_cancel()
                    })
                    .map_err(rag_index_error_for_agent)?;
                    let model = index
                        .chunks
                        .first()
                        .map(|chunk| chunk.embedding_model.clone())
                        .unwrap_or_else(|| "local-hash".to_string());
                    (index, "local".to_string(), model, None)
                };
                progress.advance("publishing", 1, "Workspace indexed; publishing knowledge");
                let published = build_and_publish_knowledge_generation_with_commit(
                    &root,
                    index,
                    || cancellation.should_cancel(),
                    || cancellation.begin_commit_window(),
                    |window| window.finalize(),
                )?;
                progress.advance("persisting", 2, "Knowledge generation published");
                Ok((published, backend, model, fallback_error))
            },
        )?;
    let PublishedKnowledgeGeneration {
        paths,
        adapter,
        graph_nodes,
        graph_edges,
        lancedb_export_records,
        lancedb_records,
    } = published;
    let stats = adapter.stats().clone();
    let lancedb_export_path = paths.lancedb_export.clone();
    let lancedb_path = paths.lancedb_database.clone();
    cache_rag_adapter(state, &root, &adapter)?;
    let snapshot = cached_workspace_knowledge_snapshot_for(state, &root)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    let mut index_metadata = [
        ("action".to_string(), "index".to_string()),
        ("files_indexed".to_string(), stats.files_indexed.to_string()),
        (
            "chunks_indexed".to_string(),
            stats.chunks_indexed.to_string(),
        ),
        ("indexed_at_ms".to_string(), stats.indexed_at_ms.to_string()),
        (
            "index_path".to_string(),
            paths.rag_index.display().to_string(),
        ),
        (
            "lancedb_path".to_string(),
            lancedb_path.display().to_string(),
        ),
        ("lancedb_records".to_string(), lancedb_records.to_string()),
        (
            "lancedb_export_path".to_string(),
            lancedb_export_path.display().to_string(),
        ),
        (
            "lancedb_export_records".to_string(),
            lancedb_export_records.to_string(),
        ),
        ("embedding_backend".to_string(), embedding_backend),
        ("embedding_model".to_string(), embedding_model),
        (
            "graph_store_path".to_string(),
            paths.graph_store.display().to_string(),
        ),
        ("graph_nodes".to_string(), graph_nodes.to_string()),
        ("graph_edges".to_string(), graph_edges.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(error) = embedding_fallback_error {
        index_metadata.insert("embedding_fallback_error".to_string(), error);
    }
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "Workspace indexed for RAG",
        index_metadata,
    )
    .map_err(|error| error.to_string())?;
    progress.advance("projecting", 3, "Knowledge index is ready");

    let graph = graph_state_for_snapshot(&snapshot, &[]);
    let memory = project_memory_stats(&mut store, project_id.as_deref())
        .map_err(|error| error.to_string())?;
    phase7_state(
        &store,
        snapshot.adapter.stats(),
        memory,
        Vec::new(),
        None,
        graph,
        None,
        None,
        None,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn search_rag(
    app: tauri::AppHandle,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    tauri::async_runtime::spawn_blocking(move || search_rag_blocking(&app, input))
        .await
        .map_err(|error| format!("RAG search failed to join: {error}"))?
}

fn search_rag_blocking(
    app: &tauri::AppHandle,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    let state = app.state::<AppState>();
    let operation_id = input.operation_id.clone();
    run_rag_operation(
        &state.rag_operation_controls,
        app,
        &operation_id,
        "search",
        4,
        |cancellation, progress| search_rag_operation(&state, input, cancellation, progress),
        |output| output.last_error.clone(),
    )
}

fn prepare_manual_rag_snapshot(
    state: &tauri::State<'_, AppState>,
    root: &Path,
    config: &ProviderConfig,
    cancellation: &Arc<RagOperationControl>,
    progress: &mut RagOperationProgressReporter<'_>,
) -> Result<WorkspaceKnowledgeSnapshot, String> {
    progress.advance("preparing_knowledge", 0, "Refreshing workspace knowledge");
    let mut snapshot = cached_workspace_knowledge_snapshot_for(state, root)?;
    let cache_hit = snapshot.cache_hit;
    let expected_epoch = cancellation.agent().steer_epoch();
    let indexed = ensure_workspace_knowledge_index(WorkspaceKnowledgeIndexRequest {
        workspace_root: root,
        adapter: &mut snapshot.adapter,
        cache_hit,
        config,
        cancellation: cancellation.agent(),
        expected_epoch,
        resource_checkpoint: None,
        rag_operation: Some(cancellation),
    })?;
    if workspace_knowledge_cache_needs_refresh(cache_hit, indexed.is_some()) {
        cache_rag_adapter(state, root, &snapshot.adapter)?;
        snapshot = cached_workspace_knowledge_snapshot_for(state, root)?;
    }
    rag_operation_checkpoint(cancellation)?;
    progress.advance("knowledge_ready", 1, "Workspace knowledge is ready");
    Ok(snapshot)
}

fn empty_manual_rag_error(snapshot: &WorkspaceKnowledgeSnapshot) -> Option<&'static str> {
    (snapshot.adapter.stats().chunks_indexed == 0).then_some(
        if snapshot.adapter.stats().indexed_at_ms > 0 {
            "No indexable workspace text was found."
        } else {
            "Workspace text knowledge has not been indexed yet."
        },
    )
}

fn phase7_state_with_operation_error(
    state: &tauri::State<'_, AppState>,
    operation: &RagOperationControl,
    message: impl Into<String>,
    sources: Vec<RagSourceView>,
    answer: Option<String>,
) -> Result<Phase7State, String> {
    let message = message.into();
    if rag_operation_was_cancelled(operation, &message) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let root = active_workspace_root(state)?;
    let project_id = active_project_id_for_memory(state)?;
    let snapshot = cached_workspace_knowledge_snapshot_for(state, &root)?;
    let focus_paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let graph = graph_state_for_snapshot(&snapshot, &focus_paths);
    with_finalizing_cancellable_mutex(&state.store, operation, "store", |store| {
        append_event(
            store,
            &phase7_task_id(),
            EventKind::Error,
            "RAG request failed",
            [("error".to_string(), message.clone())]
                .into_iter()
                .collect(),
        )
        .map_err(|error| error.to_string())?;
        let memory = project_memory_stats(store, project_id.as_deref())
            .map_err(|error| error.to_string())?;
        phase7_state(
            store,
            snapshot.adapter.stats(),
            memory,
            sources,
            None,
            graph,
            None,
            answer,
            Some(message),
        )
        .map_err(|error| error.to_string())
    })
}

fn search_rag_operation(
    state: &tauri::State<'_, AppState>,
    input: RagSearchInput,
    cancellation: &Arc<RagOperationControl>,
    progress: &mut RagOperationProgressReporter<'_>,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(state)?;
    let project_id = active_project_id_for_memory(state)?;
    let query = input.query.trim().to_string();
    if query.is_empty() {
        return phase7_state_with_operation_error(
            state,
            cancellation,
            "RAG query is empty",
            Vec::new(),
            None,
        );
    }

    let config = clone_provider_config(state)?;
    let snapshot = prepare_manual_rag_snapshot(state, &root, &config, cancellation, progress)?;
    if let Some(message) = empty_manual_rag_error(&snapshot) {
        return phase7_state_with_operation_error(state, cancellation, message, Vec::new(), None);
    }
    let index_cache_hit = snapshot.cache_hit;
    let mut retrieval = run_parallel_retrieval(
        &root,
        &snapshot.adapter,
        &config,
        &query,
        input.limit.unwrap_or(6),
        "four_way_parallel",
        snapshot.graph_store.as_deref(),
        cancellation.agent(),
        None,
    )?;
    progress.advance("retrieving", 2, "Relevant sources retrieved");
    retrieval.trace.index_cache_hit = index_cache_hit;
    let focus_paths = retrieval
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let graph = graph_state_for_snapshot(&snapshot, &focus_paths);
    with_finalizing_cancellable_mutex(&state.store, cancellation, "store", |store| {
        append_rag_retrieval_event(
            store,
            "search",
            &query,
            &retrieval.results,
            Some(&retrieval.trace),
        )
        .map_err(|error| error.to_string())?;
        let memory = project_memory_stats(store, project_id.as_deref())
            .map_err(|error| error.to_string())?;
        progress.advance("projecting", 3, "Search result is ready");

        phase7_state(
            store,
            snapshot.adapter.stats(),
            memory,
            retrieval.sources,
            Some(retrieval.trace),
            graph,
            None,
            None,
            None,
        )
        .map_err(|error| error.to_string())
    })
}

#[tauri::command]
pub(crate) async fn answer_with_rag(
    app: tauri::AppHandle,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    tauri::async_runtime::spawn_blocking(move || answer_with_rag_blocking(&app, input))
        .await
        .map_err(|error| format!("RAG answer failed to join: {error}"))?
}

fn answer_with_rag_blocking(
    app: &tauri::AppHandle,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    let state = app.state::<AppState>();
    let operation_id = input.operation_id.clone();
    run_rag_operation(
        &state.rag_operation_controls,
        app,
        &operation_id,
        "answer",
        5,
        |cancellation, progress| answer_with_rag_operation(&state, input, cancellation, progress),
        |output| output.last_error.clone(),
    )
}

fn answer_with_rag_operation(
    state: &tauri::State<'_, AppState>,
    input: RagSearchInput,
    cancellation: &Arc<RagOperationControl>,
    progress: &mut RagOperationProgressReporter<'_>,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(state)?;
    let project_id = active_project_id_for_memory(state)?;
    let query = input.query.trim().to_string();
    if query.is_empty() {
        return phase7_state_with_operation_error(
            state,
            cancellation,
            "RAG query is empty",
            Vec::new(),
            None,
        );
    }

    let config = clone_provider_config(state)?;
    let snapshot = prepare_manual_rag_snapshot(state, &root, &config, cancellation, progress)?;
    if let Some(message) = empty_manual_rag_error(&snapshot) {
        return phase7_state_with_operation_error(state, cancellation, message, Vec::new(), None);
    }
    let index_cache_hit = snapshot.cache_hit;
    let mut retrieval = run_parallel_retrieval(
        &root,
        &snapshot.adapter,
        &config,
        &query,
        input.limit.unwrap_or(6),
        "four_way_parallel",
        snapshot.graph_store.as_deref(),
        cancellation.agent(),
        None,
    )?;
    progress.advance("retrieving", 2, "Grounding sources retrieved");
    retrieval.trace.index_cache_hit = index_cache_hit;
    let selected_results = retrieval.results.clone();
    let sources = retrieval.sources.clone();
    let focus_paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let graph = graph_state_for_snapshot(&snapshot, &focus_paths);

    with_cancellable_mutex(&state.store, cancellation, "store", |store| {
        append_rag_retrieval_event(
            store,
            "answer",
            &query,
            &selected_results,
            Some(&retrieval.trace),
        )
        .map_err(|error| error.to_string())
    })?;

    if selected_results.is_empty() {
        return phase7_state_with_operation_error(
            state,
            cancellation,
            "No indexed sources matched the RAG query",
            sources,
            None,
        );
    }

    if !config.is_ready() {
        return phase7_state_with_operation_error(
            state,
            cancellation,
            "Provider config is incomplete",
            sources,
            None,
        );
    }

    let prompt = build_grounded_answer_prompt(&query, &selected_results);
    let request_id = unique_id("rag-model");
    let model = config.model_for_role(&ModelRole::Executor);
    let task_id = phase7_task_id();
    let started_at_ms = current_time_millis();

    with_cancellable_mutex(&state.store, cancellation, "store", |store| {
        append_event(
            store,
            &task_id,
            EventKind::ModelRequestStarted,
            format!("RAG answer request started for {model}"),
            [
                ("request_id".to_string(), request_id.clone()),
                ("provider".to_string(), config.provider_id.clone()),
                ("base_url".to_string(), config.base_url.clone()),
                ("model".to_string(), model.clone()),
                ("role".to_string(), "executor".to_string()),
                (
                    "source_count".to_string(),
                    selected_results.len().to_string(),
                ),
                ("prompt_length".to_string(), prompt.len().to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())
    })?;

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: 180,
    });
    let request = ModelRequest {
        role: ModelRole::Executor,
        messages: vec![Message {
            role: MessageRole::User,
            content: prompt,
            metadata: Metadata::new(),
        }],
        tools: Vec::new(),
        mode: ModelCallMode::Streaming,
        metadata: Metadata::new(),
    };
    rag_operation_checkpoint(cancellation)?;
    if let Err(reason) = cancellation
        .agent()
        .begin_stage_model_call("rag_answer", RunStageClass::Finalizer)
    {
        return phase7_state_with_operation_error(
            state,
            cancellation,
            format!("RAG answer budget unavailable: {}", reason.code()),
            sources,
            None,
        );
    }
    let model_attempt = match crate::model_resource_runtime::ControlledModelAttempt::reserve_at(
        cancellation.agent(),
        cancellation.agent().steer_epoch(),
        &model,
        &request,
        RunStageClass::Finalizer,
    ) {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            cancellation.agent().finish_model_call();
            return phase7_state_with_operation_error(
                state,
                cancellation,
                "RAG answer request was superseded".to_string(),
                sources,
                None,
            );
        }
        Err(reason) => {
            cancellation.agent().finish_model_call();
            return phase7_state_with_operation_error(
                state,
                cancellation,
                format!("RAG answer budget unavailable: {}", reason.code()),
                sources,
                None,
            );
        }
    };
    progress.advance("requesting", 3, "Requesting grounded answer");

    let mut receiving = false;
    match provider.complete_streaming_cancellable(
        request,
        |delta| {
            if !receiving && !delta.is_empty() {
                receiving = true;
                progress.advance("receiving", 4, "Receiving grounded answer");
            }
        },
        || cancellation.should_cancel(),
    ) {
        Ok(response) => {
            let _ = model_attempt.settle_response(&response);
            cancellation.agent().finish_model_call();
            rag_operation_checkpoint(cancellation)?;
            let answer = response.message.content;
            let latency_ms = current_time_millis().saturating_sub(started_at_ms);
            with_finalizing_cancellable_mutex(&state.store, cancellation, "store", |store| {
                let mut metadata = [
                    ("request_id".to_string(), request_id),
                    ("provider".to_string(), config.provider_id.clone()),
                    ("model".to_string(), model.clone()),
                    ("latency_ms".to_string(), latency_ms.to_string()),
                    (
                        "source_count".to_string(),
                        selected_results.len().to_string(),
                    ),
                    ("answer".to_string(), answer.clone()),
                    ("output_length".to_string(), answer.len().to_string()),
                ]
                .into_iter()
                .collect::<Metadata>();
                for key in [
                    "prompt_tokens",
                    "completion_tokens",
                    "total_tokens",
                    "usage_source",
                ] {
                    if let Some(value) = response.metadata.get(key) {
                        metadata.insert(key.to_string(), value.clone());
                    }
                }
                crate::model_resource_runtime::add_model_resource_metadata(
                    &mut metadata,
                    cancellation.agent(),
                );
                append_event(
                    store,
                    &task_id,
                    EventKind::ModelRequestFinished,
                    format!("RAG answer received from {model}"),
                    metadata,
                )
                .map_err(|error| error.to_string())?;
                let memory = project_memory_stats(store, project_id.as_deref())
                    .map_err(|error| error.to_string())?;

                phase7_state(
                    store,
                    snapshot.adapter.stats(),
                    memory,
                    sources,
                    Some(retrieval.trace),
                    graph,
                    None,
                    Some(answer),
                    None,
                )
                .map_err(|error| error.to_string())
            })
        }
        Err(error) => {
            let _ = model_attempt.settle_unknown();
            cancellation.agent().finish_model_call();
            if error.is_cancelled() {
                return Err(MODEL_REQUEST_CANCELLED.to_string());
            }
            let message = error.to_string();
            phase7_state_with_operation_error(state, cancellation, message, sources, None)
        }
    }
}

#[tauri::command]
pub(crate) fn cancel_rag_operation(
    state: tauri::State<'_, AppState>,
    operation_id: String,
) -> Result<bool, String> {
    cancel_rag_operation_control(&state.rag_operation_controls, operation_id.trim())
}

#[tauri::command]
pub(crate) fn get_phase8_state(state: tauri::State<'_, AppState>) -> Result<Phase8State, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase8_state(&store, None).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn get_context_state(
    app: tauri::AppHandle,
    session_id: Option<String>,
) -> Result<ContextState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let run_context = project_session_metadata_for_session(&state, session_id.as_deref())?;
        let root = run_context
            .get("project_root")
            .map(PathBuf::from)
            .unwrap_or(active_workspace_root(&state)?);
        let store = open_app_read_store()?;

        context_state(&store, &root, &run_context, None, None)
    })
    .await
    .map_err(|error| format!("context state load failed to join: {error}"))?
}

#[tauri::command]
pub(crate) fn compact_context(state: tauri::State<'_, AppState>) -> Result<ContextState, String> {
    let run_context = project_session_metadata_for_session(&state, None)?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let events = collect_context_events(&store, &run_context).map_err(|error| error.to_string())?;
    let checkpoint =
        build_session_checkpoint_at(&events, CheckpointOptions::default(), current_time_millis());
    let mut pack = build_restore_context_pack(checkpoint);
    let messages = events
        .iter()
        .filter_map(message_from_event)
        .collect::<Vec<_>>();
    pack.text.push_str(&conversation_memory_to_markdown(
        &messages,
        CONTEXT_MEMORY_MAX_ITEMS,
    ));
    let checkpoint_path = write_context_checkpoint(
        &root,
        run_context.get("session_id").map(String::as_str),
        &pack.text,
        Some(ContextCheckpointCoverage {
            history: &messages,
            covered_messages: messages.len(),
        }),
    )?;
    let mut metadata = Metadata::new();
    metadata.insert("checkpoint_id".to_string(), pack.checkpoint.id.clone());
    metadata.insert(
        "event_count".to_string(),
        pack.checkpoint.event_count.to_string(),
    );
    metadata.insert(
        "task_count".to_string(),
        pack.checkpoint.task_count.to_string(),
    );
    metadata.insert(
        "latest_event_ms".to_string(),
        pack.checkpoint.latest_event_ms.to_string(),
    );
    metadata.insert(
        "context_checkpoint_path".to_string(),
        checkpoint_path.display().to_string(),
    );
    if let Some(goal) = &pack.checkpoint.current_goal {
        metadata.insert("current_goal".to_string(), goal.clone());
    }

    append_event(
        &mut store,
        &phase15_task_id(),
        EventKind::TaskStatusChanged,
        "Context checkpoint compacted",
        metadata_with_context(metadata, &run_context),
    )
    .map_err(|error| error.to_string())?;

    context_state(&store, &root, &run_context, Some(pack), None)
}

#[tauri::command]
pub(crate) fn run_browser_tool(
    state: tauri::State<'_, AppState>,
    input: BrowserToolInput,
) -> Result<Phase8State, String> {
    let root = active_workspace_root(&state)?;
    let tool_name = input.tool_name.trim().to_string();
    if !is_phase8_tool(&tool_name) {
        return phase8_state_with_error(&state, format!("not a Phase 8 tool: {tool_name}"));
    }

    let task_id = phase8_task_id();
    let run_context = project_session_metadata_for_session(&state, None)?;
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId(unique_id("browser")),
        task_id: task_id.clone(),
        tool_name: tool_name.clone(),
        input_json: input.input,
        proposed_by_model: "local-user".to_string(),
        metadata: run_context,
    };
    let registry = tool_registry_for_state(&state, &root)?;
    let Some(tool) = registry.get(&tool_name) else {
        return phase8_state_with_error(&state, format!("unknown tool: {tool_name}"));
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_tool_proposed_event(&mut store, &invocation, None).map_err(|error| error.to_string())?;

    if let Some(mut request) = tool.permission_request(&invocation) {
        request.id = PermissionRequestId(unique_id("perm"));
        request
            .metadata
            .insert("phase".to_string(), "8".to_string());
        request
            .metadata
            .insert("tool_input".to_string(), invocation.input_json.clone());
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0.clone());
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name.clone());
        for (key, value) in &invocation.metadata {
            request
                .metadata
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        store
            .save_permission_request(request.clone(), current_time_millis())
            .map_err(|error| error.to_string())?;
        append_event(
            &mut store,
            &task_id,
            EventKind::PermissionRequested,
            format!("Permission requested for {}", request.action),
            [
                ("permission_id".to_string(), request.id.0),
                ("tool_call_id".to_string(), invocation.id.0),
                ("tool".to_string(), request.action),
                (
                    "risk".to_string(),
                    permission_risk_label(&request.risk).to_string(),
                ),
                ("scope".to_string(), request.scope),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;

        return phase8_state(&store, None).map_err(|error| error.to_string());
    }

    drop(store);
    execute_manual_tool_invocation(
        &state.manual_tool_execution_gate,
        &state.store,
        &registry,
        invocation,
        &root,
        None,
    )?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    phase8_state(&store, None).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn resolve_browser_permission(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
) -> Result<Phase8State, String> {
    let root = active_workspace_root(&state)?;
    let run_context = project_session_metadata_for_session(&state, None)?;
    let registry = tool_registry_for_state(&state, &root)?;
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;

    if request.task_id != phase8_task_id() {
        return phase8_state(
            &store,
            Some("permission does not belong to Phase 8".to_string()),
        )
        .map_err(|error| error.to_string());
    }

    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: decision.clone(),
            resolved_at_ms: current_time_millis(),
            resolved_by: "local-user".to_string(),
        })
        .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(&decision)),
        [
            ("permission_id".to_string(), request_id.0.clone()),
            (
                "decision".to_string(),
                permission_decision_label(&decision).to_string(),
            ),
            ("tool".to_string(), request.action.clone()),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(
                request
                    .metadata
                    .get("tool_call_id")
                    .cloned()
                    .unwrap_or_else(|| unique_id("browser")),
            ),
            task_id: request.task_id.clone(),
            tool_name: request
                .metadata
                .get("tool_name")
                .cloned()
                .unwrap_or(request.action),
            input_json: request
                .metadata
                .get("tool_input")
                .cloned()
                .unwrap_or_default(),
            proposed_by_model: "local-user".to_string(),
            metadata: run_context,
        };
        drop(store);
        execute_manual_tool_invocation(
            &state.manual_tool_execution_gate,
            &state.store,
            &registry,
            invocation,
            &root,
            None,
        )?;
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        return phase8_state(&store, None).map_err(|error| error.to_string());
    } else {
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Browser tool denied",
            [
                (
                    "tool_call_id".to_string(),
                    request
                        .metadata
                        .get("tool_call_id")
                        .cloned()
                        .unwrap_or_default(),
                ),
                (
                    "tool".to_string(),
                    request
                        .metadata
                        .get("tool_name")
                        .cloned()
                        .unwrap_or(request.action),
                ),
                ("status".to_string(), "denied".to_string()),
                ("output".to_string(), "denied by user".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    phase8_state(&store, None).map_err(|error| error.to_string())
}
