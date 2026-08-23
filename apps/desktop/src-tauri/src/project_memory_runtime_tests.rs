use super::*;

#[test]
fn memory_recall_policy_controls_the_context_budget() {
    assert_eq!(memory_recall_limit(MemoryRecallPolicy::None), 0);
    assert!(
        memory_recall_limit(MemoryRecallPolicy::Relevant)
            < memory_recall_limit(MemoryRecallPolicy::Comprehensive)
    );
    assert_eq!(
        memory_recall_limit(MemoryRecallPolicy::Comprehensive),
        crate::runtime_constants::AGENT_MEMORY_RECALL_LIMIT
    );
}

#[test]
fn project_memory_read_model_persists_deduplicated_cross_session_requirements() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, run_id) in [
        ("session-memory-a", "run-memory-a"),
        ("session-memory-b", "run-memory-b"),
    ] {
        let context = [
            ("project_id".to_string(), "project-memory".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            context.clone(),
        )
        .expect("run should start");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            "Keep effort selection scoped to each session",
            context.clone(),
        )
        .expect("user requirement should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "Effort is now stored per session.",
            context.clone(),
        )
        .expect("assistant outcome should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task completed",
            context,
        )
        .expect("run should complete");
    }

    let ledger = load_project_memory_ledger(&mut store, "project-memory")
        .expect("memory read model should build");
    assert_eq!(ledger.project_id, "project-memory");
    assert_eq!(
        ledger
            .records
            .iter()
            .filter(|record| record.kind == agent_memory::MemoryKind::Requirement)
            .count(),
        1
    );
    let requirement = ledger
        .records
        .iter()
        .find(|record| record.kind == agent_memory::MemoryKind::Requirement)
        .expect("requirement memory should exist");
    assert_eq!(requirement.source_session_ids.len(), 2);
    let requirement_id = requirement.id.clone();
    let persisted = store
        .load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, "project-memory")
        .expect("memory read model should load")
        .expect("memory read model should exist");
    assert_eq!(persisted.revision, ledger.revision);

    let use_context = [
        ("project_id".to_string(), "project-memory".to_string()),
        ("session_id".to_string(), "session-memory-c".to_string()),
        ("agent_run_id".to_string(), "run-memory-c".to_string()),
        ("memory_ids".to_string(), requirement_id.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let used = record_project_memory_observed_use(
        &mut store,
        &phase16_task_id(),
        &use_context,
        "Kept effort selection scoped to each session.",
    )
    .expect("memory utilization should persist");
    assert_eq!(used, 1);
    let updated = load_project_memory_ledger(&mut store, "project-memory")
        .expect("updated memory ledger should load");
    assert_eq!(
        updated
            .records
            .iter()
            .find(|record| record.id == requirement_id)
            .map(|record| record.observed_use_count),
        Some(1)
    );
}

#[test]
fn project_memory_read_model_ignores_unrelated_project_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_a = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Keep project A requirements isolated",
        project_a.clone(),
    )
    .expect("project A message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_a,
    )
    .expect("project A run should complete");
    let initial =
        load_project_memory_ledger(&mut store, "project-a").expect("project A ledger should build");

    let project_b = [
        ("project_id".to_string(), "project-b".to_string()),
        ("session_id".to_string(), "session-b".to_string()),
        ("agent_run_id".to_string(), "run-b".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Unrelated project B requirement",
        project_b.clone(),
    )
    .expect("project B message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_b,
    )
    .expect("project B run should complete");

    let unchanged = load_project_memory_ledger(&mut store, "project-a")
        .expect("project A ledger should remain current");
    assert_eq!(unchanged.revision, initial.revision);
    assert_eq!(unchanged.event_count, initial.event_count);
    assert_eq!(unchanged.records, initial.records);
}

#[test]
fn project_memory_feedback_keeps_project_scoped_revision() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_a = [
        ("project_id".to_string(), "project-feedback-a".to_string()),
        ("session_id".to_string(), "session-feedback-a".to_string()),
        ("agent_run_id".to_string(), "run-feedback-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Always keep memory feedback isolated by project",
        project_a.clone(),
    )
    .expect("project A message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_a,
    )
    .expect("project A run should complete");
    let initial = load_project_memory_ledger(&mut store, "project-feedback-a")
        .expect("project A ledger should build");
    let memory_id = initial
        .records
        .iter()
        .find(|record| record.kind == agent_memory::MemoryKind::Requirement)
        .expect("requirement memory should exist")
        .id
        .clone();

    let project_b = [
        ("project_id".to_string(), "project-feedback-b".to_string()),
        ("session_id".to_string(), "session-feedback-b".to_string()),
        ("agent_run_id".to_string(), "run-feedback-b".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Unrelated project B requirement",
        project_b,
    )
    .expect("project B message should append");

    let use_context = [
        ("project_id".to_string(), "project-feedback-a".to_string()),
        ("session_id".to_string(), "session-feedback-a-2".to_string()),
        ("agent_run_id".to_string(), "run-feedback-a-2".to_string()),
        ("memory_ids".to_string(), memory_id.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    assert_eq!(
        record_project_memory_observed_use(
            &mut store,
            &phase16_task_id(),
            &use_context,
            "Kept memory feedback isolated by project.",
        )
        .expect("memory feedback should persist"),
        1
    );

    let persisted = store
        .load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, "project-feedback-a")
        .expect("memory read model should load")
        .expect("memory read model should exist");
    let scoped_revision = store
        .event_revision_by_metadata(&phase16_task_id(), "project_id", "project-feedback-a")
        .expect("project revision should load");
    assert_eq!(persisted.revision, scoped_revision.latest_sequence);
    let updated = load_project_memory_ledger(&mut store, "project-feedback-a")
        .expect("project A ledger should remain incremental");
    assert_eq!(updated.event_count, scoped_revision.event_count);
    assert_eq!(
        updated
            .records
            .iter()
            .find(|record| record.id == memory_id)
            .map(|record| record.observed_use_count),
        Some(1)
    );
}

#[test]
fn project_memory_projection_persists_and_searches_real_lancedb_vectors() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        (
            "project_id".to_string(),
            "project-vector-memory".to_string(),
        ),
        (
            "session_id".to_string(),
            "session-vector-memory".to_string(),
        ),
        ("agent_run_id".to_string(), "run-vector-memory".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Always keep the inspector frosted and translucent",
        context.clone(),
    )
    .expect("user requirement should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("run should complete");
    let ledger = load_project_memory_ledger(&mut store, "project-vector-memory")
        .expect("memory ledger should build");
    let root = std::env::temp_dir().join(format!(
        "cindx-memory-vector-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));

    let fallback = refresh_project_memory_vector_index(&root, &ProviderConfig::default(), &ledger)
        .expect("memory vectors should persist");
    assert!(fallback.is_none());
    let (database_path, manifest_path, current_generation) =
        memory_vector_paths_for_read(&root, "project-vector-memory");
    assert!(lancedb_index_exists(&database_path));
    let results = search_lancedb_index(
        &database_path,
        &local_query_embedding("frosted translucent inspector"),
        4,
    )
    .expect("memory vectors should search");
    assert_eq!(
        results.first().map(|result| result.chunk.id.as_str()),
        Some(ledger.records[0].id.as_str())
    );
    let manifest = load_memory_vector_manifest(&manifest_path)
        .expect("manifest should load")
        .expect("manifest should exist");
    assert_eq!(manifest.embedding_backend, "local");
    assert_eq!(manifest.record_count, ledger.records.len());
    assert_eq!(
        current_generation.as_deref(),
        Some(manifest.generation_id.as_str())
    );
    let unpublished_generation = unique_id("memory-vector-unpublished");
    let (_, unpublished_manifest_path) =
        memory_vector_generation_paths(&root, "project-vector-memory", &unpublished_generation);
    let mut unpublished_manifest = manifest.clone();
    unpublished_manifest.generation_id = unpublished_generation;
    write_private_file_atomically(
        &unpublished_manifest_path,
        &serde_json::to_vec(&unpublished_manifest).expect("manifest should encode"),
        "test unpublished memory vector manifest",
    )
    .expect("unpublished manifest should stage");
    let (still_published_database, _, still_published_generation) =
        memory_vector_paths_for_read(&root, "project-vector-memory");
    assert_eq!(still_published_database, database_path);
    assert_eq!(still_published_generation, current_generation);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn project_memory_never_persists_raw_secrets() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        (
            "project_id".to_string(),
            "project-memory-secret".to_string(),
        ),
        (
            "session_id".to_string(),
            "session-memory-secret".to_string(),
        ),
        ("agent_run_id".to_string(), "run-memory-secret".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Always use sk-1234567890abcdef for this project",
        context.clone(),
    )
    .expect("redacted message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("run should complete");

    let ledger = load_project_memory_ledger(&mut store, "project-memory-secret")
        .expect("memory ledger should build");
    let payload = serde_json::to_string(&ledger).expect("memory ledger should serialize");

    assert!(!payload.contains("sk-1234567890abcdef"));
    assert!(payload.contains("[REDACTED]"));
}
