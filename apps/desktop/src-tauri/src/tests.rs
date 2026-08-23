use super::test_support::temp_test_root;
use super::*;

#[test]
fn tool_registry_cache_is_versioned_and_bounded() {
    let mut cache = ToolRegistryCache::default();
    let active_root = PathBuf::from("/tmp/cindx-tool-cache-active");
    cache.insert(active_root.clone(), 7, ToolRegistry::new());

    assert!(cache.get(&active_root, 7).is_some());
    assert!(cache.get(&active_root, 8).is_none());

    for index in 0..TOOL_REGISTRY_CACHE_LIMIT {
        cache.insert(
            PathBuf::from(format!("/tmp/cindx-tool-cache-{index}")),
            7,
            ToolRegistry::new(),
        );
    }
    assert_eq!(cache.entries.len(), TOOL_REGISTRY_CACHE_LIMIT);

    cache.clear();
    assert!(cache.entries.is_empty());
}

#[test]
fn transient_provider_failures_are_retryable_but_invalid_requests_are_not() {
    let broken_pipe = ModelError::new("failed to configure curl: Broken pipe (os error 32)");
    let timeout =
        ModelError::new("model stream timed out after 180 seconds without receiving data");
    let invalid = ModelError::with_status(
        400,
        "invalid_request_error: Unexpected item type in content",
    );
    assert!(broken_pipe.is_retryable());
    assert!(timeout.is_retryable());
    assert!(!invalid.is_retryable());
    assert_eq!(
        exhausted_model_transport_error_stop_reason(&broken_pipe),
        Some(RunStopReason::ProviderUnavailable)
    );
    assert_eq!(exhausted_model_transport_error_stop_reason(&invalid), None);
}

#[test]
fn quit_confirmation_preference_requires_explicit_suppression() {
    assert!(quit_confirmation_suppressed_text(
        "theme=system\nskip_quit_confirmation=true\n"
    ));
    assert!(!quit_confirmation_suppressed_text(
        "skip_quit_confirmation=false\n"
    ));
}

#[test]
fn read_only_runtime_snapshots_do_not_write_projection_caches() {
    let root = std::env::temp_dir().join(format!(
        "cindx-read-only-snapshot-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(&root).expect("snapshot test directory should exist");
    let database = root.join("state.sqlite3");
    drop(SqliteStore::open(&database).expect("writable store should initialize"));

    let mut store = SqliteStore::open_read_only(&database).expect("read-only store should open");
    assert!(load_routing_telemetry_read_model_snapshot(&mut store)
        .expect("routing snapshot should remain read-only")
        .is_empty());
    assert!(
        load_project_memory_ledger_snapshot(&mut store, "project-read-only")
            .expect("memory snapshot should remain read-only")
            .records
            .is_empty()
    );
    assert!(
        load_agent_session_read_model_snapshot(&store, "session-read-only")
            .expect("session snapshot should remain read-only")
            .state
            .messages
            .is_empty()
    );

    drop(store);
    fs::remove_dir_all(root).expect("snapshot test directory should be removed");
}

#[test]
fn workspace_knowledge_snapshot_reuses_one_graph_parse_and_borrowed_projection() {
    let root = temp_test_root("phase7-shared-graph-snapshot");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "SharedGraphMarker uses file.read with docs/reference.md",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("graph should build");
    let mut adapter = open_rag_adapter_for(&root).expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");

    reset_graph_store_open_count();
    let entry = workspace_knowledge_cache_entry(&adapter).expect("cache entry should build");
    assert_eq!(graph_store_open_count(), 1);
    let first = entry
        .snapshot_if_current(adapter.path())
        .expect("current cache entry should produce a snapshot");
    let second = entry
        .snapshot_if_current(adapter.path())
        .expect("repeated cache hit should produce a snapshot");
    let first_graph = first
        .graph_store
        .as_ref()
        .expect("snapshot should include graph");
    let second_graph = second
        .graph_store
        .as_ref()
        .expect("snapshot should include graph");
    assert!(Arc::ptr_eq(first_graph, second_graph));
    assert_eq!(
        first_graph.path(),
        crate::knowledge_generation_runtime::knowledge_paths_for_rag_index(first.adapter.path())
            .graph_store
    );

    let focus_paths = vec!["notes.md".to_string()];
    let borrowed = graph_state_for_snapshot(&first, &focus_paths);
    assert_eq!(graph_store_open_count(), 1);
    assert!(borrowed.nodes.len() <= 80);
    assert!(borrowed.edges.len() <= 140);
    let from_disk = graph_state_at_path(first_graph.path(), &focus_paths)
        .expect("disk projection should remain available");
    assert_eq!(graph_store_open_count(), 2);
    assert_eq!(
        serde_json::to_value(&borrowed).expect("borrowed graph should serialize"),
        serde_json::to_value(&from_disk).expect("disk graph should serialize")
    );

    fs::remove_file(first_graph.path()).expect("graph fixture should be removable");
    let after_removal = graph_state_for_snapshot(&second, &focus_paths);
    assert_eq!(graph_store_open_count(), 2);
    assert_eq!(
        serde_json::to_value(&after_removal).expect("leased graph should serialize"),
        serde_json::to_value(&borrowed).expect("original graph should serialize")
    );
    println!(
        "{{\"schema\":\"cindx.workspace-graph-cache-scaling.v1\",\"cache_hit_graph_opens\":1,\"final_graph_opens\":{},\"shared_graph_store\":true,\"borrowed_nodes\":{},\"borrowed_edges\":{}}}",
        graph_store_open_count(),
        borrowed.nodes.len(),
        borrowed.edges.len()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn default_sidecar_state_reports_runtime_capabilities() {
    let config = SidecarConfig::default();
    let state = sidecar_state(&config, None);

    assert!(state.auto_configure);
    assert!(state.browser.exists);
    assert!(state.browser.healthy);
    assert!(state.computer.exists);
    assert!(state.computer.executable);
    if !state.computer.healthy {
        assert!(!state.computer.health_output.trim().is_empty());
    }
}
