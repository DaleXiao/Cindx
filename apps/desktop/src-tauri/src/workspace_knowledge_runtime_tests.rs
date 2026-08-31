use super::test_support::temp_test_root;
use super::*;

#[test]
fn workspace_index_falls_back_to_local_embeddings_when_cloud_fails() {
    struct FailingEmbedder;

    impl RagEmbedder for FailingEmbedder {
        fn embed_texts(&mut self, _texts: &[String]) -> Result<EmbeddingBatch, RagError> {
            Err(RagError::new("configured embedding model is unavailable"))
        }
    }

    let root = temp_test_root("phase7-cloud-fallback");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "# Cindx\n\nLocal indexing remains available when cloud embeddings fail.",
    )
    .expect("fixture should write");

    let (index, backend, model, fallback_error) = index_workspace_with_cloud_fallback(
        &root,
        IndexOptions::default(),
        &mut FailingEmbedder,
        "missing-cloud-model",
    )
    .expect("local fallback should build the index");

    assert_eq!(backend, "local-fallback");
    assert!(model.starts_with("local-hash-"));
    assert_eq!(
        fallback_error.as_deref(),
        Some("configured embedding model is unavailable")
    );
    assert!(!index.chunks.is_empty());
    assert!(index
        .chunks
        .iter()
        .all(|chunk| chunk.embedding_provider == "local"));
}

#[test]
fn incremental_workspace_index_reuses_embeddings_for_unchanged_files() {
    struct RecordingEmbedder {
        provider: String,
        embedded_texts: Vec<String>,
    }

    impl RagEmbedder for RecordingEmbedder {
        fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, RagError> {
            self.embedded_texts.extend(texts.iter().cloned());
            Ok(EmbeddingBatch {
                provider: self.provider.clone(),
                model: "test-embedding".to_string(),
                vectors: texts
                    .iter()
                    .map(|text| vec![text.len() as f32, 1.0])
                    .collect(),
            })
        }
    }

    let root = temp_test_root("phase7-incremental-reuse");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "alpha evidence lines").expect("a should write");
    fs::write(root.join("b.md"), "bravo evidence lines").expect("b should write");
    let empty_base = RagIndex {
        chunks: Vec::new(),
        stats: RagIndexStats::default(),
    };
    let mut first_embedder = RecordingEmbedder {
        provider: "first-cloud".to_string(),
        embedded_texts: Vec::new(),
    };
    let (base, backend, _, _) = index_workspace_with_cloud_fallback_cancellable(
        &root,
        IndexOptions::default(),
        &empty_base,
        &mut first_embedder,
        "test-embedding",
        || false,
    )
    .expect("base index should build");
    assert_eq!(backend, "cloud");
    assert_eq!(first_embedder.embedded_texts.len(), 2);
    let base_b = base
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .cloned()
        .expect("b chunk should exist");

    fs::write(root.join("a.md"), "alpha evidence lines extended").expect("a should update");
    let mut second_embedder = RecordingEmbedder {
        provider: "second-cloud".to_string(),
        embedded_texts: Vec::new(),
    };
    let (incremental, backend, _, _) = index_workspace_with_cloud_fallback_cancellable(
        &root,
        IndexOptions::default(),
        &base,
        &mut second_embedder,
        "test-embedding",
        || false,
    )
    .expect("incremental index should build");

    assert_eq!(backend, "cloud");
    assert_eq!(
        second_embedder.embedded_texts,
        vec!["alpha evidence lines extended".to_string()]
    );
    let reused_b = incremental
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .expect("b chunk should remain");
    assert_eq!(reused_b, &base_b);
    let updated_a = incremental
        .chunks
        .iter()
        .find(|chunk| chunk.path == "a.md")
        .expect("a chunk should remain");
    assert_eq!(updated_a.embedding_provider, "second-cloud");
}

#[test]
fn incremental_workspace_index_fallback_restores_the_local_profile() {
    struct RecordingEmbedder;

    impl RagEmbedder for RecordingEmbedder {
        fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, RagError> {
            Ok(EmbeddingBatch {
                provider: "first-cloud".to_string(),
                model: "test-embedding".to_string(),
                vectors: texts
                    .iter()
                    .map(|text| vec![text.len() as f32, 1.0])
                    .collect(),
            })
        }
    }

    struct FailingEmbedder;

    impl RagEmbedder for FailingEmbedder {
        fn embed_texts(&mut self, _texts: &[String]) -> Result<EmbeddingBatch, RagError> {
            Err(RagError::new("configured embedding model is unavailable"))
        }
    }

    let root = temp_test_root("phase7-incremental-fallback");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "alpha evidence lines").expect("a should write");
    fs::write(root.join("b.md"), "bravo evidence lines").expect("b should write");
    let empty_base = RagIndex {
        chunks: Vec::new(),
        stats: RagIndexStats::default(),
    };
    let (base, backend, _, _) = index_workspace_with_cloud_fallback_cancellable(
        &root,
        IndexOptions::default(),
        &empty_base,
        &mut RecordingEmbedder,
        "test-embedding",
        || false,
    )
    .expect("base index should build");
    assert_eq!(backend, "cloud");

    fs::write(root.join("a.md"), "alpha evidence lines extended").expect("a should update");
    let (fallback, backend, model, fallback_error) =
        index_workspace_with_cloud_fallback_cancellable(
            &root,
            IndexOptions::default(),
            &base,
            &mut FailingEmbedder,
            "test-embedding",
            || false,
        )
        .expect("local fallback should build the index");

    assert_eq!(backend, "local-fallback");
    assert_eq!(
        fallback_error.as_deref(),
        Some("configured embedding model is unavailable")
    );
    assert!(model.starts_with("local-hash-"));
    assert_eq!(fallback.chunks.len(), 2);
    assert!(fallback
        .chunks
        .iter()
        .all(|chunk| chunk.embedding_provider == "local"));
    let full_local = index_workspace(&root, IndexOptions::default()).expect("local index");
    let semantic = |chunk: &RagChunk| {
        (
            chunk.id.clone(),
            chunk.embedding.clone(),
            chunk.embedding_provider.clone(),
            chunk.embedding_model.clone(),
            chunk.embedding_dimensions,
        )
    };
    assert_eq!(
        fallback.chunks.iter().map(semantic).collect::<Vec<_>>(),
        full_local.chunks.iter().map(semantic).collect::<Vec<_>>(),
    );
}

#[test]
fn phase7_state_reports_rag_stats() {
    let root = temp_test_root("phase7-stats");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "# Cindx\n\nThe RAG index stores line-level provenance.",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "Workspace indexed for RAG",
        [
            ("action".to_string(), "index".to_string()),
            ("files_indexed".to_string(), "1".to_string()),
            ("chunks_indexed".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase7_state(
        &store,
        adapter.stats(),
        MemoryStatsView::default(),
        Vec::new(),
        None,
        empty_graph_state(),
        None,
        None,
        None,
    )
    .expect("state should load");

    assert_eq!(state.stats.files_indexed, 1);
    assert_eq!(state.stats.chunks_indexed, 1);
    assert!(state
        .timeline
        .iter()
        .any(|entry| entry.label == "Retrieval"));
}

#[test]
fn phase7_graph_counts_bind_to_the_latest_exact_index_path() {
    let root = temp_test_root("phase7-graph-count-summary");
    let active_path = root.join("generation-new").join("rag-index.tsv");
    let stale_path = root.join("generation-old").join("rag-index.tsv");
    let other_workspace_path = root.join("other-workspace").join("rag-index.tsv");
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (path, nodes, edges) in [
        (&active_path, 3, 2),
        (&other_workspace_path, 99, 88),
        (&active_path, 7, 6),
        (&stale_path, 55, 44),
    ] {
        append_event(
            &mut store,
            &phase7_task_id(),
            EventKind::RetrievalPerformed,
            "Workspace indexed for RAG",
            [
                ("action".to_string(), "index".to_string()),
                ("index_path".to_string(), path.display().to_string()),
                ("graph_nodes".to_string(), nodes.to_string()),
                ("graph_edges".to_string(), edges.to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("index event should append");
    }
    let events = store
        .list_by_task(&phase7_task_id())
        .expect("phase 7 events should load");

    let graph = graph_count_summary_for_index_events(&events, &active_path);

    assert_eq!(graph.total_nodes, 7);
    assert_eq!(graph.total_edges, 6);
    assert!(graph.nodes.is_empty());
    assert!(graph.edges.is_empty());
    assert_eq!(
        graph_count_summary_for_index_events(
            &events,
            &root.join("never-indexed").join("rag-index.tsv")
        )
        .total_nodes,
        0
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rag_sources_include_line_ranges() {
    let root = temp_test_root("phase7-sources");
    fs::create_dir_all(root.join("docs")).expect("temp docs should exist");
    fs::write(
        root.join("docs").join("rag.md"),
        "Intro\nSemantic retrieval should cite exact source lines.\nDone",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");

    let results = adapter
        .search("semantic retrieval source lines", 3)
        .expect("search should run");
    let sources = rag_sources_from_results(&results);

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].path, "docs/rag.md");
    assert_eq!(sources[0].start_line, 1);
    assert_eq!(sources[0].end_line, 3);
    assert!(sources[0].score > 0.0);
}

#[test]
fn coding_retrieval_mode_skips_graph_channels() {
    let root = temp_test_root("phase7-selective-retrieval");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("notes.md"), "Cindx selective retrieval source")
        .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    replace_lancedb_index(lancedb_database_path_for(&root), adapter.index())
        .expect("LanceDB index should persist");
    let cancellation = Arc::new(AgentRunControl::new("auto"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "selective retrieval source",
        4,
        "semantic_literal_parallel",
        None,
        &cancellation,
        None,
    )
    .expect("retrieval should run");

    assert_eq!(
        retrieval
            .trace
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>(),
        vec!["semantic_rag", "file_search"]
    );
}

#[test]
fn retrieval_keeps_file_evidence_when_semantic_channel_is_unavailable() {
    let root = temp_test_root("phase7-independent-channel-failure");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "independent lexical fallback evidence",
    )
    .expect("fixture should write");
    let mut index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    for chunk in &mut index.chunks {
        chunk.embedding_provider = "cloud".to_string();
        chunk.embedding_model = "cloud-embedding".to_string();
    }
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let cancellation = Arc::new(AgentRunControl::new("auto"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "independent lexical fallback evidence",
        4,
        "semantic_literal_parallel",
        None,
        &cancellation,
        None,
    )
    .expect("independent channels should degrade without failing the retrieval");

    let semantic = retrieval
        .trace
        .channels
        .iter()
        .find(|channel| channel.name == "semantic_rag")
        .expect("semantic channel");
    let file = retrieval
        .trace
        .channels
        .iter()
        .find(|channel| channel.name == "file_search")
        .expect("file channel");
    assert!(semantic.error.is_some());
    assert!(file.error.is_none());
    assert!(file.result_count > 0);
    assert!(!retrieval.results.is_empty());
}

#[test]
fn workspace_cache_ttl_advances_only_after_validation_or_index_change() {
    assert!(!workspace_knowledge_cache_needs_refresh(true, false));
    assert!(workspace_knowledge_cache_needs_refresh(false, false));
    assert!(workspace_knowledge_cache_needs_refresh(true, true));
}

#[test]
fn workspace_knowledge_snapshot_preserves_ttl_and_generation_validation() {
    let root = temp_test_root("phase7-shared-graph-validation");
    fs::create_dir_all(&root).expect("temp root should exist");
    let adapter = open_rag_adapter_for(&root).expect("adapter should open");
    let mut entry = workspace_knowledge_cache_entry(&adapter).expect("cache entry should build");

    assert!(entry.snapshot_if_current(adapter.path()).is_some());
    assert!(entry
        .snapshot_if_current(&root.join(".cindx").join("different-generation.tsv"))
        .is_none());
    entry.validated_at = Instant::now() - WORKSPACE_KNOWLEDGE_CACHE_TTL - Duration::from_millis(1);
    assert!(entry.snapshot_if_current(adapter.path()).is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cold_knowledge_state_reads_stats_without_opening_the_adapter_or_graph() {
    let root = temp_test_root("phase7-lightweight-cold-state");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("notes.md"), "LightweightColdStateMarker").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let expected_stats = index.stats.clone();
    index_graph_chunks(&root, &index.chunks).expect("graph should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let cache = Mutex::new(BTreeMap::new());

    reset_rag_adapter_open_count();
    reset_graph_store_open_count();
    let cold = active_workspace_knowledge_state_snapshot_in(&cache, &root)
        .expect("cold state should load lightweight stats");

    assert!(cold.full.is_none());
    assert_eq!(cold.active_index_path, adapter.path());
    assert_eq!(cold.stats, expected_stats);
    assert_eq!(rag_adapter_open_count(), 0);
    assert_eq!(graph_store_open_count(), 0);

    let entry = workspace_knowledge_cache_entry(&adapter).expect("full cache entry should build");
    cache
        .lock()
        .expect("cache should lock")
        .insert(workspace_knowledge_cache_key(&root), entry);
    reset_rag_adapter_open_count();
    reset_graph_store_open_count();
    let warm = active_workspace_knowledge_state_snapshot_in(&cache, &root)
        .expect("warm state should reuse the exact active generation");

    assert_eq!(warm.active_index_path, adapter.path());
    assert_eq!(
        warm.full
            .as_ref()
            .expect("warm state should retain the full snapshot")
            .adapter
            .path(),
        adapter.path()
    );
    assert_eq!(warm.stats, expected_stats);
    assert_eq!(rag_adapter_open_count(), 0);
    assert_eq!(graph_store_open_count(), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn malformed_lightweight_header_falls_back_to_full_adapter_stats_without_graph_open() {
    let root = temp_test_root("phase7-lightweight-stats-fallback");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("notes.md"), "LegacyStatsFallbackMarker").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let expected_chunks = index.chunks.len();
    let index_path = root.join(".cindx").join("rag-index.tsv");
    let mut adapter = FileRagAdapter::open(&index_path).expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let persisted = fs::read_to_string(&index_path).expect("index fixture should read");
    let (_, tail) = persisted
        .split_once('\n')
        .expect("non-empty index should have a chunk tail");
    fs::write(&index_path, format!("legacy-stats-header\n{tail}"))
        .expect("legacy header fixture should write");
    let cache = Mutex::new(BTreeMap::new());

    reset_rag_adapter_open_count();
    reset_graph_store_open_count();
    let state = active_workspace_knowledge_state_snapshot_in(&cache, &root)
        .expect("legacy stats should fall back to the full adapter");

    assert!(state.full.is_none());
    assert_eq!(state.stats.chunks_indexed, expected_chunks);
    assert_eq!(rag_adapter_open_count(), 1);
    assert_eq!(graph_store_open_count(), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_generation_cannot_overwrite_the_active_knowledge_cache() {
    let root = temp_test_root("phase7-stale-generation-cache");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("first.md"), "first generation cache evidence")
        .expect("first fixture should write");
    let first_index =
        index_workspace(&root, IndexOptions::default()).expect("first index should build");
    let first =
        crate::knowledge_generation_runtime::build_and_publish_knowledge_generation_cancellable(
            &root,
            first_index,
            || false,
        )
        .expect("first generation should publish");

    let cache = Arc::new(Mutex::new(BTreeMap::new()));
    let old_ready = Arc::new(std::sync::Barrier::new(2));
    let release_old = Arc::new(std::sync::Barrier::new(2));
    let old_cache = Arc::clone(&cache);
    let old_root = root.clone();
    let old_adapter = first.adapter.clone();
    let old_ready_worker = Arc::clone(&old_ready);
    let release_old_worker = Arc::clone(&release_old);
    let old_writer = std::thread::spawn(move || {
        old_ready_worker.wait();
        release_old_worker.wait();
        cache_rag_adapter_in(&old_cache, &old_root, &old_adapter)
    });
    old_ready.wait();

    fs::write(
        root.join("second.rs"),
        "struct SecondGenerationCacheMarker; fn second_generation_cache_marker() {}",
    )
    .expect("second fixture should write");
    let second_index =
        index_workspace(&root, IndexOptions::default()).expect("second index should build");
    let second =
        crate::knowledge_generation_runtime::build_and_publish_knowledge_generation_cancellable(
            &root,
            second_index,
            || false,
        )
        .expect("second generation should publish");

    reset_graph_store_open_count();
    assert!(cache_rag_adapter_in(&cache, &root, &second.adapter)
        .expect("active generation should enter the cache"));
    assert_eq!(graph_store_open_count(), 1);
    release_old.wait();
    assert!(!old_writer
        .join()
        .expect("old cache writer should join")
        .expect("old cache writer should remain non-fatal"));

    let key = workspace_knowledge_cache_key(&root);
    let cache = cache.lock().expect("knowledge cache should lock");
    let entry = cache.get(&key).expect("active cache entry should remain");
    assert_eq!(entry.adapter.path(), second.adapter.path());
    let first_snapshot = entry
        .snapshot_if_current(second.adapter.path())
        .expect("active cache entry should produce a snapshot");
    let second_snapshot = entry
        .snapshot_if_current(second.adapter.path())
        .expect("repeated cache hit should produce a snapshot");
    assert!(Arc::ptr_eq(
        first_snapshot
            .graph_store
            .as_ref()
            .expect("first snapshot should include graph"),
        second_snapshot
            .graph_store
            .as_ref()
            .expect("second snapshot should include graph"),
    ));
    assert_eq!(graph_store_open_count(), 1);
    drop(cache);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn empty_workspace_knowledge_generation_is_reused() {
    let root = temp_test_root("phase7-empty-generation");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("reference.png"), b"not indexable text").expect("fixture should write");
    let mut adapter = open_rag_adapter_for(&root).expect("adapter should open");
    let initial_index_path = adapter.path().to_path_buf();
    let cancellation = Arc::new(AgentRunControl::new("auto"));
    let expected_epoch = cancellation.steer_epoch();

    let first = ensure_workspace_knowledge_index(WorkspaceKnowledgeIndexRequest {
        workspace_root: &root,
        adapter: &mut adapter,
        cache_hit: false,
        config: &ProviderConfig::default(),
        cancellation: &cancellation,
        expected_epoch,
        resource_checkpoint: None,
        rag_operation: None,
    })
    .expect("empty generation should publish");
    assert_eq!(
        first
            .expect("first ensure should index")
            .stats
            .chunks_indexed,
        0
    );
    let first_index_path = adapter.path().to_path_buf();
    assert_ne!(first_index_path, initial_index_path);

    let second = ensure_workspace_knowledge_index(WorkspaceKnowledgeIndexRequest {
        workspace_root: &root,
        adapter: &mut adapter,
        cache_hit: true,
        config: &ProviderConfig::default(),
        cancellation: &cancellation,
        expected_epoch,
        resource_checkpoint: None,
        rag_operation: None,
    })
    .expect("fresh empty generation should validate");
    assert!(second.is_none());
    assert_eq!(adapter.path(), first_index_path);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn interactive_observation_tools_do_not_invalidate_workspace_knowledge() {
    assert!(!tool_may_mutate_workspace(
        "browser.click",
        &ToolRisk::UsesNetwork
    ));
    assert!(!tool_may_mutate_workspace(
        "computer.key",
        &ToolRisk::Destructive
    ));
    assert!(!tool_may_mutate_workspace("file.read", &ToolRisk::ReadOnly));
    assert!(tool_may_mutate_workspace(
        "file.write",
        &ToolRisk::WritesWorkspace
    ));
    assert!(tool_may_mutate_workspace(
        "shell.run",
        &ToolRisk::ExecutesProcess
    ));
}

#[test]
fn graph_walk_seed_fusion_includes_semantic_and_file_evidence() {
    let root = temp_test_root("phase7-graph-seeds");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "semantic source alpha").expect("fixture should write");
    fs::write(root.join("b.md"), "direct source beta").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let semantic = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.9,
    };
    let file = RagSearchResult {
        chunk: index.chunks[1].clone(),
        score: 0.8,
    };
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![semantic.clone()],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: Vec::new(),
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![file.clone()],
            error: None,
        },
    ];

    let seeds = graph_walk_seed_results(&channels, 8);

    assert_eq!(seeds.len(), 2);
    assert_eq!(seeds[0].chunk.id, semantic.chunk.id);
    assert_eq!(seeds[1].chunk.id, file.chunk.id);
}

#[test]
fn complex_retrieval_runs_parallel_seed_channels_before_graph_walk() {
    let root = temp_test_root("phase7-four-way-retrieval");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "Cindx graph retrieval connects workspace evidence",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("graph should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    replace_lancedb_index(lancedb_database_path_for(&root), adapter.index())
        .expect("LanceDB index should persist");
    let cancellation = Arc::new(AgentRunControl::new("pro"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "graph retrieval workspace evidence",
        4,
        "four_way_parallel",
        None,
        &cancellation,
        None,
    )
    .expect("retrieval should run");

    let channel_names = retrieval
        .trace
        .channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        channel_names.iter().copied().collect::<BTreeSet<_>>(),
        ["semantic_rag", "graph_recall", "file_search", "graph_walk"]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(channel_names.last().copied(), Some("graph_walk"));
    assert!(retrieval
        .trace
        .channels
        .iter()
        .all(|channel| channel.error.is_none()));
    assert!(!retrieval.results.is_empty());
}

#[test]
fn graph_index_cancellation_preserves_previous_cache() {
    let root = temp_test_root("phase7-graph-cancel");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "graph source").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("initial graph should build");
    let graph_path = graph_store_path_for(&root);
    let before = fs::read(&graph_path).expect("initial graph should persist");

    fs::write(root.join("b.md"), "replacement graph source")
        .expect("replacement fixture should write");
    let replacement =
        index_workspace(&root, IndexOptions::default()).expect("replacement index should build");

    let error = index_graph_chunks_cancellable(&root, &replacement.chunks, || true)
        .expect_err("graph indexing should cancel");

    assert_eq!(error, MODEL_REQUEST_CANCELLED);
    assert_eq!(
        fs::read(&graph_path).expect("previous graph should remain available"),
        before
    );
    assert!(
        fs::read_dir(graph_path.parent().expect("graph parent should exist"))
            .expect("graph directory should list")
            .all(|entry| !entry
                .expect("graph entry should load")
                .file_name()
                .to_string_lossy()
                .contains("graph-index"))
    );
}

#[test]
fn retrieval_fusion_deduplicates_and_preserves_channel_reasons() {
    let root = temp_test_root("phase7-fusion");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "fusion source alpha").expect("fixture should write");
    fs::write(root.join("b.md"), "fusion source beta").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let first = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.9,
    };
    let second = RagSearchResult {
        chunk: index.chunks[1].clone(),
        score: 0.8,
    };
    let mut overlapping = first.clone();
    overlapping.chunk.id = "direct-file-evidence".to_string();
    overlapping.score = 1.2;
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 2,
            results: vec![first.clone(), second],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![overlapping],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, "", 8);

    assert_eq!(results.len(), 2);
    assert_eq!(sources.len(), 2);
    assert!(sources[0].reason.contains("semantic_rag"));
    assert!(sources[0].reason.contains("file_search"));
}

#[test]
fn retrieval_fusion_prefers_independent_consensus_over_one_channel_outlier() {
    let root = temp_test_root("phase7-calibrated-consensus");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "single channel outlier").expect("fixture should write");
    fs::write(root.join("b.md"), "independently corroborated evidence")
        .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let outlier = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "a.md")
        .expect("outlier chunk should exist")
        .clone();
    let corroborated = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .expect("corroborated chunk should exist")
        .clone();
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![
                RagSearchResult {
                    chunk: outlier,
                    score: 100.0,
                },
                RagSearchResult {
                    chunk: corroborated.clone(),
                    score: 0.4,
                },
            ],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: corroborated,
                score: 0.5,
            }],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, "", 4);

    assert_eq!(results[0].chunk.path, "b.md");
    assert!(sources[0].reason.contains("consensus:2"));
    assert!(results.iter().all(|result| result.score.is_finite()));
}

#[test]
fn retrieval_fusion_does_not_double_count_correlated_graph_routes() {
    let root = temp_test_root("phase7-calibrated-graph-family");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "graph-only evidence").expect("fixture should write");
    fs::write(root.join("b.md"), "semantic evidence").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let graph = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "a.md")
        .expect("graph chunk should exist")
        .clone();
    let semantic = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .expect("semantic chunk should exist")
        .clone();
    let channels = vec![
        RetrievalChannelOutcome {
            name: "graph_recall".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: graph.clone(),
                score: 1.0,
            }],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: graph,
                score: 1.0,
            }],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: semantic,
                score: 1.0,
            }],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, "", 4);

    assert_eq!(results[0].chunk.path, "b.md");
    let graph_source = sources
        .iter()
        .find(|source| source.path == "a.md")
        .expect("graph source should remain available");
    assert!(!graph_source.reason.contains("consensus:"));
}
