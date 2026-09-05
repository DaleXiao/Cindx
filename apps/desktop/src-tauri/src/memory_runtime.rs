use crate::desktop_prelude::*;
#[cfg(test)]
pub(crate) use crate::memory_projection_runtime::is_memory_checkpoint_event;
pub(crate) use crate::memory_projection_runtime::{
    memory_events_for_terminal_steer_epoch, save_project_memory_ledger,
};
#[cfg(test)]
use crate::memory_vector_generation_runtime::memory_vector_project_key;
pub(crate) use crate::memory_vector_refresh_coordinator::{
    delete_project_memory_vector_index, purge_project_memory_vector_history,
    schedule_project_memory_vector_refresh,
};
#[cfg(test)]
pub(crate) use crate::memory_vector_refresh_coordinator::{
    invalidate_stale_project_memory_vector_index, memory_vector_project_is_deleted,
    memory_vector_refresh_gate, refresh_project_memory_vector_index,
};
#[cfg(test)]
pub(crate) use crate::memory_vector_refresh_generation::memory_rag_index;
use crate::{
    agent_effort_planner::KnowledgeDecision,
    agent_query_commands::append_agent_progress_event,
    app_state::AppState,
    configuration_models::ProviderConfig,
    event_persistence::append_event,
    knowledge_runtime::{prepare_agent_knowledge_context, CloudRagEmbedder},
    memory_projection_runtime::{
        load_project_memory_ledger_inner, persist_project_memory_snapshot_if_current,
        LoadedProjectMemoryLedger,
    },
    memory_record_persistence_runtime::persist_quarantined_memory_records,
    memory_vector_generation_runtime::{
        memory_recall_projection_sha256, memory_vector_manifest_matches,
        memory_vector_projection_sha256, open_memory_vector_snapshot, MemoryVectorManifest,
    },
    persistence_runtime::{open_app_read_store, skill_catalog_for_root},
    project_session_persistence::metadata_with_context,
    runtime_constants::AGENT_MEMORY_RECALL_LIMIT,
    runtime_values::current_time_millis,
    view_models::MemoryStatsView,
};
use agent_memory::suppress_conflicting_recalls_for_current_request;

pub(crate) fn load_project_memory_ledger(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<MemoryLedger, StorageError> {
    Ok(load_project_memory_ledger_with_status(store, project_id)?.ledger)
}

pub(crate) fn load_project_memory_ledger_with_status(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<LoadedProjectMemoryLedger, StorageError> {
    let mut loaded = load_project_memory_ledger_inner(store, project_id)?;
    let mut vector_reset_required = loaded.vector_reset_required;
    if loaded.quarantine_needs_persistence {
        persist_quarantined_memory_records(store, &loaded.ledger)?;
        loaded = load_project_memory_ledger_inner(store, project_id)?;
        vector_reset_required |= loaded.vector_reset_required;
        if loaded.quarantine_needs_persistence {
            return Err(StorageError::new(
                "legacy memory quarantine could not be retained durably",
            ));
        }
    }
    loaded.vector_reset_required = vector_reset_required;
    loaded.ledger.vector_history_reset_required = vector_reset_required;
    if loaded.needs_persist {
        save_project_memory_ledger(store, &loaded.ledger)?;
    }
    Ok(loaded)
}

#[cfg(test)]
pub(crate) fn load_project_memory_ledger_snapshot(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<MemoryLedger, StorageError> {
    Ok(load_project_memory_ledger_inner(store, project_id)?.ledger)
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

pub(crate) fn active_project_id_for_memory(state: &AppState) -> Result<Option<String>, String> {
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
    let records = ledger
        .records
        .iter()
        .filter(|record| {
            ledger
                .controls
                .get(&record.id)
                .is_none_or(|control| !control.deleted)
        })
        .collect::<Vec<_>>();
    Ok(MemoryStatsView {
        records: records.len(),
        requirements: records
            .iter()
            .filter(|record| record.kind == MemoryKind::Requirement)
            .count(),
        outcomes: records
            .iter()
            .filter(|record| record.kind == MemoryKind::Outcome)
            .count(),
        evidence: records
            .iter()
            .filter(|record| record.kind == MemoryKind::Evidence)
            .count(),
        recalls: records.iter().map(|record| record.recall_count).sum(),
        observed_uses: records.iter().map(|record| record.observed_use_count).sum(),
        updated_at_ms: records
            .iter()
            .map(|record| record.updated_at_ms)
            .max()
            .unwrap_or_default(),
    })
}

#[derive(Debug, Default)]
pub(crate) struct PreparedRunKnowledgeContexts {
    pub(crate) memory: Option<PreparedMemoryRecall>,
    pub(crate) workspace: Option<Message>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedMemoryRecall {
    project_id: String,
    recall_projection_sha256: String,
    recalled_at_ms: u64,
    recalls: Vec<agent_memory::MemoryRecall>,
    event_metadata: Metadata,
    message: Message,
}

pub(crate) fn append_prepared_memory_context(
    run_context: &mut Metadata,
    history: &mut Vec<Message>,
    memory_context: Option<&PreparedMemoryRecall>,
) {
    let Some(memory_context) = memory_context else {
        return;
    };
    let memory_context = &memory_context.message;
    if let Some(memory_ids) = memory_context.metadata.get("memory_ids") {
        run_context.insert("memory_ids".to_string(), memory_ids.clone());
    }
    if let Some(selected_count) = memory_context.metadata.get("selected_count") {
        run_context.insert("memory_selected_count".to_string(), selected_count.clone());
    }
    history.push(memory_context.clone());
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_run_knowledge_contexts(
    state: &AppState,
    task_id: &TaskId,
    run_context: &Metadata,
    workspace_root: &Path,
    config: &ProviderConfig,
    knowledge: &KnowledgeDecision,
    cancellation: &Arc<AgentRunControl>,
    expected_epoch: u64,
) -> Result<PreparedRunKnowledgeContexts, String> {
    let recall_memory = knowledge.memory_enabled();
    let workspace_plan = knowledge.workspace_plan();
    if !recall_memory && workspace_plan.is_none() {
        return Ok(PreparedRunKnowledgeContexts::default());
    }
    if recall_memory {
        cancellation.mark_progress_at(
            expected_epoch,
            "memory",
            "Recalling relevant project memory",
        );
    }
    if workspace_plan.is_some() {
        cancellation.mark_progress_at(expected_epoch, "retrieval", "Preparing workspace knowledge");
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
                    &knowledge.memory_query,
                    knowledge.memory_policy,
                    cancellation,
                    expected_epoch,
                )
            })
        });
        let workspace_handle = workspace_plan.as_ref().map(|plan| {
            scope.spawn(|| {
                prepare_agent_knowledge_context(
                    state,
                    config,
                    task_id,
                    run_context,
                    workspace_root,
                    plan,
                    cancellation,
                    expected_epoch,
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

    if !cancellation.preparation_epoch_is_current(expected_epoch) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }

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

pub(crate) struct ProjectMemorySemanticScoreRequest<'a> {
    pub(crate) workspace_root: &'a Path,
    pub(crate) project_id: &'a str,
    pub(crate) ledger: &'a MemoryLedger,
    pub(crate) config: &'a ProviderConfig,
    pub(crate) prompt: &'a str,
    pub(crate) cancellation: &'a Arc<AgentRunControl>,
    pub(crate) expected_epoch: u64,
    pub(crate) resource_checkpoint: Option<&'a AgentResourceCheckpoint<'a>>,
}

pub(crate) fn project_memory_semantic_scores(
    request: ProjectMemorySemanticScoreRequest<'_>,
) -> Result<(BTreeMap<String, f64>, MemoryVectorManifest), String> {
    let ProjectMemorySemanticScoreRequest {
        workspace_root,
        project_id,
        ledger,
        config,
        prompt,
        cancellation,
        expected_epoch,
        resource_checkpoint,
    } = request;
    let snapshot = open_memory_vector_snapshot(workspace_root, project_id)?;
    let manifest = snapshot
        .manifest
        .as_ref()
        .ok_or_else(|| "memory vector index is not ready".to_string())?;
    if snapshot
        .generation_id
        .as_deref()
        .is_some_and(|generation| manifest.generation_id != generation)
        || !lancedb_index_exists(&snapshot.database_path)
        || !memory_vector_manifest_matches(
            manifest,
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
            expected_steer_epoch: Some(expected_epoch),
            resource_checkpoint,
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
        &snapshot.database_path,
        &query_embedding,
        AGENT_MEMORY_RECALL_LIMIT.saturating_mul(4),
    )
    .map_err(|error| error.to_string())?;
    Ok((
        results
            .into_iter()
            .map(|result| (result.chunk.id, f64::from(result.score)))
            .collect(),
        manifest.clone(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn recall_project_memory_for_prompt(
    state: &AppState,
    _task_id: &TaskId,
    run_context: &Metadata,
    workspace_root: &Path,
    config: &ProviderConfig,
    prompt: &str,
    policy: MemoryRecallPolicy,
    cancellation: &Arc<AgentRunControl>,
    expected_epoch: u64,
) -> Result<Option<PreparedMemoryRecall>, String> {
    if !cancellation.preparation_epoch_is_current(expected_epoch) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let Some(project_id) = run_context.get("project_id") else {
        return Ok(None);
    };
    let session_id = run_context.get("session_id").map(String::as_str);
    let recall_limit = memory_recall_limit(policy);
    if recall_limit == 0 {
        return Ok(None);
    }
    let started_at = Instant::now();
    let now_ms = current_time_millis();
    let loaded = {
        let mut store = open_app_read_store()?;
        load_project_memory_ledger_inner(&mut store, project_id)
            .map_err(|error| error.to_string())?
    };
    let vector_reset_required = loaded.vector_reset_required;
    let quarantine_needs_persistence = loaded.quarantine_needs_persistence;
    let needs_persist = loaded.needs_persist;
    let mut ledger = loaded.ledger;
    if vector_reset_required {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let mut current = load_project_memory_ledger_with_status(&mut store, project_id)
            .map_err(|error| error.to_string())?;
        if current.vector_reset_required {
            purge_project_memory_vector_history(workspace_root, &current.ledger)?;
            current.ledger.vector_history_reset_required = false;
            save_project_memory_ledger(&mut store, &current.ledger)
                .map_err(|error| error.to_string())?;
        }
        ledger = current.ledger;
    } else if quarantine_needs_persistence {
        let retained = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))
            .and_then(|mut store| {
                load_project_memory_ledger(&mut store, project_id)
                    .map_err(|error| error.to_string())
            });
        match retained {
            Ok(current) => ledger = current,
            Err(error) => {
                eprintln!("failed to retain legacy memory quarantine: {error}");
            }
        }
    } else if needs_persist {
        let persisted = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))
            .and_then(|mut store| {
                persist_project_memory_snapshot_if_current(&mut store, &ledger)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = persisted {
            eprintln!("failed to persist migrated project memory projection: {error}");
        }
    }
    if !ledger
        .records
        .iter()
        .any(|record| ledger.record_is_active_for_recall(record))
    {
        schedule_project_memory_vector_refresh(
            workspace_root.to_path_buf(),
            config.clone(),
            ledger,
        );
        return Ok(None);
    }
    let lexical_recalls = recall_memories_at(
        &ledger,
        prompt,
        session_id,
        recall_limit.saturating_mul(2),
        now_ms,
    );
    if !cancellation.preparation_epoch_is_current(expected_epoch) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    schedule_project_memory_vector_refresh(
        workspace_root.to_path_buf(),
        config.clone(),
        ledger.clone(),
    );
    let resource_checkpoint = |control: &AgentRunControl| {
        crate::agent_resource_snapshot::checkpoint_agent_run_resources(state, run_context, control)
    };
    let (semantic_scores, vector_manifest, vector_error) =
        match project_memory_semantic_scores(ProjectMemorySemanticScoreRequest {
            workspace_root,
            project_id,
            ledger: &ledger,
            config,
            prompt,
            cancellation,
            expected_epoch,
            resource_checkpoint: Some(&resource_checkpoint),
        }) {
            Ok((scores, manifest)) => (scores, Some(manifest), None),
            Err(error) => (BTreeMap::new(), None, Some(error)),
        };
    let mut recalls = fuse_memory_recalls_at(
        &ledger,
        lexical_recalls,
        &semantic_scores,
        session_id,
        recall_limit.saturating_mul(2),
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
    let suppressed_conflicts = run_context
        .get("effective_prompt_objective")
        .or_else(|| run_context.get("prompt_objective"))
        .or_else(|| run_context.get("initial_prompt_objective"))
        .map(|current_request| {
            suppress_conflicting_recalls_for_current_request(&mut recalls, current_request)
        })
        .unwrap_or_default();
    recalls.truncate(recall_limit);
    if !cancellation.preparation_epoch_is_current(expected_epoch) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    if recalls.is_empty() {
        return Ok(None);
    }
    let mut metadata = [
        ("action".to_string(), "memory_recall".to_string()),
        ("query".to_string(), prompt.to_string()),
        (
            "retrieval_mode".to_string(),
            if semantic_scores.is_empty() {
                "project_memory_hybrid_fallback"
            } else {
                "project_memory_hybrid"
            }
            .to_string(),
        ),
        ("selected_count".to_string(), recalls.len().to_string()),
        (
            "suppressed_current_request_conflicts".to_string(),
            suppressed_conflicts.to_string(),
        ),
        (
            "memory_policy".to_string(),
            format!("{policy:?}").to_ascii_lowercase(),
        ),
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
    let message = Message {
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
    };
    Ok(Some(PreparedMemoryRecall {
        project_id: project_id.clone(),
        recall_projection_sha256: memory_recall_projection_sha256(&ledger),
        recalled_at_ms: now_ms,
        recalls,
        event_metadata: metadata,
        message,
    }))
}

pub(crate) fn memory_recall_limit(policy: MemoryRecallPolicy) -> usize {
    match policy {
        MemoryRecallPolicy::None => 0,
        MemoryRecallPolicy::Relevant => AGENT_MEMORY_RECALL_LIMIT.div_ceil(2),
        MemoryRecallPolicy::Comprehensive => AGENT_MEMORY_RECALL_LIMIT,
    }
}

pub(crate) fn commit_prepared_memory_recall(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    prepared: Option<&PreparedMemoryRecall>,
) -> Result<(), StorageError> {
    let Some(prepared) = prepared else {
        return Ok(());
    };
    let mut ledger = load_project_memory_ledger(store, &prepared.project_id)?;
    if memory_recall_projection_sha256(&ledger) != prepared.recall_projection_sha256 {
        return Err(StorageError::new(MEMORY_RECALL_STALE_ERROR));
    }
    let mut recalls = Vec::with_capacity(prepared.recalls.len());
    for prepared_recall in &prepared.recalls {
        let current = ledger
            .records
            .iter()
            .find(|record| record.id == prepared_recall.record.id)
            .filter(|record| {
                ledger.record_is_active_for_recall(record)
                    && record.fingerprint == prepared_recall.record.fingerprint
                    && record.content == prepared_recall.record.content
                    && record.superseded_by == prepared_recall.record.superseded_by
            })
            .ok_or_else(|| StorageError::new(MEMORY_RECALL_STALE_ERROR))?;
        let mut recall = prepared_recall.clone();
        recall.record = current.clone();
        recalls.push(recall);
    }
    record_memory_recalls(&mut ledger, &recalls, prepared.recalled_at_ms);
    let mut metadata = prepared.event_metadata.clone();
    metadata.insert("selected_count".to_string(), recalls.len().to_string());
    metadata.insert(
        "recalled_at_ms".to_string(),
        prepared.recalled_at_ms.to_string(),
    );
    metadata.insert(
        "memory_ids".to_string(),
        recalls
            .iter()
            .map(|recall| recall.record.id.as_str())
            .collect::<Vec<_>>()
            .join(","),
    );
    crate::memory_projection_runtime::attribution::insert_memory_recall_attribution_source(
        &mut metadata,
        run_context,
        &prepared.recall_projection_sha256,
        &recalls,
    );
    append_event(
        store,
        task_id,
        EventKind::RetrievalPerformed,
        "Project memory recalled",
        metadata_with_context(metadata, run_context),
    )?;
    refresh_project_memory_ledger_revision(store, task_id, &mut ledger)?;
    save_project_memory_ledger(store, &ledger)
}

pub(crate) const MEMORY_RECALL_STALE_ERROR: &str =
    "prepared project memory changed before execution handoff";

#[cfg(test)]
#[path = "memory_runtime_tests.rs"]
mod tests;

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
                ("observed_at_ms".to_string(), observed_at_ms.to_string()),
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
