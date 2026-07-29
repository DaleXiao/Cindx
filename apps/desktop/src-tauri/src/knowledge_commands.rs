use super::*;
use crate::knowledge_generation_runtime::{
    build_and_publish_knowledge_generation_cancellable, knowledge_paths_for_rag_index,
    with_workspace_knowledge_index_lock, PublishedKnowledgeGeneration,
};

#[tauri::command]
pub(crate) fn get_phase7_state(state: tauri::State<'_, AppState>) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let project_id = active_project_id_for_memory(&state)?;
    let (adapter, _) = cached_rag_adapter_for(&state, &root)?;
    let graph = graph_state_at_path(
        &knowledge_paths_for_rag_index(adapter.path()).graph_store,
        &[],
    )?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let memory = project_memory_stats(&mut store, project_id.as_deref())
        .map_err(|error| error.to_string())?;

    phase7_state(
        &store,
        &adapter,
        memory,
        Vec::new(),
        None,
        graph,
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
    let (mut adapter, cache_hit) = cached_rag_adapter_for(&state, &root)?;
    let cancellation = Arc::new(AgentRunControl::new("auto"));
    let expected_epoch = cancellation.steer_epoch();
    let indexed = ensure_workspace_knowledge_index(
        &root,
        &mut adapter,
        cache_hit,
        &config,
        &cancellation,
        expected_epoch,
        None,
    )?;
    if workspace_knowledge_cache_needs_refresh(cache_hit, indexed.is_some()) {
        cache_rag_adapter(&state, &root, &adapter)?;
    }

    let graph = graph_state_at_path(
        &knowledge_paths_for_rag_index(adapter.path()).graph_store,
        &[],
    )?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let memory = project_memory_stats(&mut store, project_id.as_deref())
        .map_err(|error| error.to_string())?;

    phase7_state(
        &store,
        &adapter,
        memory,
        Vec::new(),
        None,
        graph,
        None,
        None,
    )
    .map_err(|error| error.to_string())
}

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
pub(crate) async fn index_workspace_rag(app: tauri::AppHandle) -> Result<Phase7State, String> {
    tauri::async_runtime::spawn_blocking(move || {
        index_workspace_rag_blocking(app.state::<AppState>())
    })
    .await
    .map_err(|error| format!("workspace indexing failed to join: {error}"))?
}

pub(crate) fn index_workspace_rag_blocking(
    state: tauri::State<'_, AppState>,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let project_id = active_project_id_for_memory(&state)?;
    let config = clone_provider_config(&state)?;
    let (published, embedding_backend, embedding_model, embedding_fallback_error) =
        with_workspace_knowledge_index_lock(&root, || {
            let (index, backend, model, fallback_error) = if config.is_ready() {
                let configured_model = config.model_for_role(&ModelRole::Embedder);
                let mut embedder = CloudRagEmbedder {
                    config: config.clone(),
                    cancellation: None,
                    expected_steer_epoch: None,
                    resource_checkpoint: None,
                };
                index_workspace_with_cloud_fallback(
                    &root,
                    IndexOptions::default(),
                    &mut embedder,
                    &configured_model,
                )
                .map_err(|error| error.to_string())?
            } else {
                let index = index_workspace(&root, IndexOptions::default())
                    .map_err(|error| error.to_string())?;
                let model = index
                    .chunks
                    .first()
                    .map(|chunk| chunk.embedding_model.clone())
                    .unwrap_or_else(|| "local-hash".to_string());
                (index, "local".to_string(), model, None)
            };
            let published =
                build_and_publish_knowledge_generation_cancellable(&root, index, || false)?;
            Ok((published, backend, model, fallback_error))
        })?;
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
    cache_rag_adapter(&state, &root, &adapter)?;
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

    let graph = graph_state_at_path(&paths.graph_store, &[])?;
    let memory = project_memory_stats(&mut store, project_id.as_deref())
        .map_err(|error| error.to_string())?;
    phase7_state(
        &store,
        &adapter,
        memory,
        Vec::new(),
        None,
        graph,
        None,
        None,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn search_rag(
    state: tauri::State<'_, AppState>,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let project_id = active_project_id_for_memory(&state)?;
    let query = input.query.trim().to_string();
    if query.is_empty() {
        return phase7_state_with_error(&state, "RAG query is empty", Vec::new(), None);
    }

    let (adapter, index_cache_hit) = cached_rag_adapter_for(&state, &root)?;
    let graph_store = cached_graph_store_for_adapter(&state, &root, &adapter)?;
    let config = clone_provider_config(&state)?;
    let cancellation = Arc::new(AgentRunControl::new("auto"));
    let mut retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &config,
        &query,
        input.limit.unwrap_or(6),
        "four_way_parallel",
        graph_store.as_ref(),
        &cancellation,
        None,
    )?;
    retrieval.trace.index_cache_hit = index_cache_hit;
    let focus_paths = retrieval
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let graph = graph_state_at_path(
        &knowledge_paths_for_rag_index(adapter.path()).graph_store,
        &focus_paths,
    )?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_rag_retrieval_event(
        &mut store,
        "search",
        &query,
        &retrieval.results,
        Some(&retrieval.trace),
    )
    .map_err(|error| error.to_string())?;
    let memory = project_memory_stats(&mut store, project_id.as_deref())
        .map_err(|error| error.to_string())?;

    phase7_state(
        &store,
        &adapter,
        memory,
        retrieval.sources,
        Some(retrieval.trace),
        graph,
        None,
        None,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn answer_with_rag(
    state: tauri::State<'_, AppState>,
    input: RagSearchInput,
) -> Result<Phase7State, String> {
    let root = active_workspace_root(&state)?;
    let project_id = active_project_id_for_memory(&state)?;
    let query = input.query.trim().to_string();
    if query.is_empty() {
        return phase7_state_with_error(&state, "RAG query is empty", Vec::new(), None);
    }

    let (adapter, index_cache_hit) = cached_rag_adapter_for(&state, &root)?;
    let graph_store = cached_graph_store_for_adapter(&state, &root, &adapter)?;
    let config = clone_provider_config(&state)?;
    let cancellation = Arc::new(AgentRunControl::new("auto"));
    let mut retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &config,
        &query,
        input.limit.unwrap_or(6),
        "four_way_parallel",
        graph_store.as_ref(),
        &cancellation,
        None,
    )?;
    retrieval.trace.index_cache_hit = index_cache_hit;
    let selected_results = retrieval.results.clone();
    let sources = retrieval.sources.clone();
    let focus_paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let graph = graph_state_at_path(
        &knowledge_paths_for_rag_index(adapter.path()).graph_store,
        &focus_paths,
    )?;

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_rag_retrieval_event(
            &mut store,
            "answer",
            &query,
            &selected_results,
            Some(&retrieval.trace),
        )
        .map_err(|error| error.to_string())?;
    }

    if selected_results.is_empty() {
        return phase7_state_with_error(
            &state,
            "No indexed sources matched the RAG query",
            sources,
            None,
        );
    }

    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return phase7_state_with_error(&state, "Provider config is incomplete", sources, None);
    }

    let prompt = build_grounded_answer_prompt(&query, &selected_results);
    let request_id = unique_id("rag-model");
    let model = config.model_for_role(&ModelRole::Executor);
    let task_id = phase7_task_id();
    let started_at_ms = current_time_millis();

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
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
        .map_err(|error| error.to_string())?;
    }

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
    if let Err(reason) = cancellation.begin_stage_model_call("rag_answer", RunStageClass::Finalizer)
    {
        return phase7_state_with_error(
            &state,
            format!("RAG answer budget unavailable: {}", reason.code()),
            sources,
            None,
        );
    }
    let model_attempt = match crate::model_resource_runtime::ControlledModelAttempt::reserve_at(
        &cancellation,
        cancellation.steer_epoch(),
        &model,
        &request,
        RunStageClass::Finalizer,
    ) {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            cancellation.finish_model_call();
            return phase7_state_with_error(
                &state,
                "RAG answer request was superseded".to_string(),
                sources,
                None,
            );
        }
        Err(reason) => {
            cancellation.finish_model_call();
            return phase7_state_with_error(
                &state,
                format!("RAG answer budget unavailable: {}", reason.code()),
                sources,
                None,
            );
        }
    };

    match provider.complete_streaming(request, |_| {}) {
        Ok(response) => {
            let _ = model_attempt.settle_response(&response);
            cancellation.finish_model_call();
            let answer = response.message.content;
            let latency_ms = current_time_millis().saturating_sub(started_at_ms);
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
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
                &cancellation,
            );
            append_event(
                &mut store,
                &task_id,
                EventKind::ModelRequestFinished,
                format!("RAG answer received from {model}"),
                metadata,
            )
            .map_err(|error| error.to_string())?;
            let memory = project_memory_stats(&mut store, project_id.as_deref())
                .map_err(|error| error.to_string())?;

            phase7_state(
                &store,
                &adapter,
                memory,
                sources,
                Some(retrieval.trace),
                graph,
                Some(answer),
                None,
            )
            .map_err(|error| error.to_string())
        }
        Err(error) => {
            let _ = model_attempt.settle_unknown();
            cancellation.finish_model_call();
            let message = error.to_string();
            phase7_state_with_error(&state, message, sources, None)
        }
    }
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

    execute_tool_invocation(&mut store, invocation, &root, Some(&registry))
        .map_err(|error| error.to_string())?;
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
        execute_tool_invocation(&mut store, invocation, &root, Some(&registry))
            .map_err(|error| error.to_string())?;
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
