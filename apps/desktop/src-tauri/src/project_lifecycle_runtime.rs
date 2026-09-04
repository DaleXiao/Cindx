use crate::{
    agent_read_model::{active_agent_events_for_session, agent_session_events},
    app_state::AppState,
    configuration_models::ProjectSessionConfig,
    event_persistence::append_event,
    event_projection::write_private_file_atomically,
    managed_artifact_lifecycle::{
        apply_managed_artifact_retirement, plan_managed_artifact_retirement,
        retire_managed_browser_sessions,
    },
    memory_projection_runtime::save_project_memory_ledger,
    memory_record_persistence_runtime::{
        persist_memory_session_retirements, retain_memory_records_for_deleted_sessions,
    },
    memory_runtime::{
        delete_project_memory_vector_index, load_project_memory_ledger,
        purge_project_memory_vector_history,
    },
    persistence_runtime::{
        app_data_root, context_checkpoint_manifest_path_for_session,
        context_checkpoint_path_for_session, secure_directory,
    },
    queue_service::{pending_queued_agent_messages, QueuedAgentMessagePayload},
    runtime_constants::{
        AGENT_MEMORY_READ_MODEL_NAMESPACE, AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
        AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, AGENT_SESSION_READ_MODEL_NAMESPACE,
        LEGACY_AGENT_MEMORY_READ_MODEL_NAMESPACE, LEGACY_ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
        ROUTING_TELEMETRY_READ_MODEL_KEY, ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
    },
    runtime_values::{phase16_task_id, unique_id},
    session_projection::load_agent_session_read_model_snapshot,
};
use agent_application::AgentRunStatus;
use agent_core::{insert_event_type_v1, Event, EventKind, EventTypeV1, Metadata};
use agent_memory::MemoryLedger;
use agent_storage::{EventStore, SqliteStore, StorageError};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const PROJECT_LIFECYCLE_JOURNAL_SCHEMA: &str = "cindx.project-lifecycle-journal.v1";
const PROJECT_LIFECYCLE_JOURNAL_DIRECTORY: &str = "project-lifecycle-journal";
const FORK_RECOVERY_METADATA_KEYS: [&str; 7] = [
    "recovery_schema",
    "recovery_resume_key",
    "recovery_state",
    "recovery_reason",
    "recovery_attempts",
    "recovery_envelope",
    "continuation_available",
];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectLifecycleJournal {
    schema: String,
    operation_id: String,
    operation: ProjectLifecycleOperation,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ProjectLifecycleOperation {
    Fork {
        project_id: String,
        project_root: PathBuf,
        source_session_id: String,
        target_session_id: String,
    },
    Delete {
        project_id: String,
        project_root: PathBuf,
        session_ids: Vec<String>,
        delete_project: bool,
    },
}

#[derive(Debug)]
pub(crate) struct RecoveredMemoryRefresh {
    pub(crate) project_root: PathBuf,
    pub(crate) ledger: MemoryLedger,
    pub(crate) journal: ProjectLifecycleJournal,
}

impl ProjectLifecycleJournal {
    pub(crate) fn fork(
        project_id: String,
        project_root: PathBuf,
        source_session_id: String,
        target_session_id: String,
    ) -> Self {
        Self {
            schema: PROJECT_LIFECYCLE_JOURNAL_SCHEMA.to_string(),
            operation_id: unique_id("session-fork"),
            operation: ProjectLifecycleOperation::Fork {
                project_id,
                project_root,
                source_session_id,
                target_session_id,
            },
        }
    }

    pub(crate) fn delete(
        project_id: String,
        project_root: PathBuf,
        session_ids: Vec<String>,
        delete_project: bool,
    ) -> Self {
        Self {
            schema: PROJECT_LIFECYCLE_JOURNAL_SCHEMA.to_string(),
            operation_id: unique_id(if delete_project {
                "project-delete"
            } else {
                "session-delete"
            }),
            operation: ProjectLifecycleOperation::Delete {
                project_id,
                project_root,
                session_ids,
                delete_project,
            },
        }
    }

    fn path_in(&self, data_root: &Path) -> PathBuf {
        lifecycle_journal_directory(data_root).join(format!("{}.json", self.operation_id))
    }
}

pub(crate) fn session_deletion_block_reason(
    state: &tauri::State<'_, AppState>,
    session_ids: &[String],
) -> Result<Option<String>, String> {
    if state
        .agent_run_controls
        .contains_any(session_ids.iter().map(String::as_str))
        .map_err(|error| error.to_string())?
    {
        return Ok(Some(
            "Session cannot be deleted while its agent is active".to_string(),
        ));
    }
    if state.suspended_agent_runs.contains_any(session_ids)? {
        return Ok(Some(
            "Session cannot be deleted while its agent is suspended".to_string(),
        ));
    }
    if state
        .queue_dispatching_sessions
        .contains_any(session_ids.iter().map(String::as_str))
        .map_err(|error| error.to_string())?
    {
        return Ok(Some(
            "Session cannot be deleted while queued work is starting".to_string(),
        ));
    }
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    for session_id in session_ids {
        let model = load_agent_session_read_model_snapshot(&store, session_id)
            .map_err(|error| error.to_string())?;
        if matches!(
            model.state.status.as_str(),
            "running" | "waiting_for_permission"
        ) {
            return Ok(Some(
                "Session cannot be deleted while its agent is active".to_string(),
            ));
        }
        if model.state.status == "paused" {
            return Ok(Some(
                "Session cannot be deleted while its agent is suspended".to_string(),
            ));
        }
        if !model.queued_payloads.is_empty() {
            return Ok(Some(
                "Session cannot be deleted while it has queued messages".to_string(),
            ));
        }
    }
    Ok(None)
}

pub(crate) fn clear_session_runtime_state(
    state: &tauri::State<'_, AppState>,
    session_ids: &[String],
) -> Result<(), String> {
    if session_ids.is_empty() {
        return Ok(());
    }
    state.process_manager.shutdown_sessions(session_ids);
    state
        .agent_run_controls
        .remove_many(session_ids, true)
        .map_err(|error| error.to_string())?;
    state.suspended_agent_runs.remove_many(session_ids)?;
    state.session_output_cache.remove_many(session_ids)?;
    state
        .queue_dispatching_sessions
        .remove_many(session_ids)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn lifecycle_journal_directory(data_root: &Path) -> PathBuf {
    data_root.join(PROJECT_LIFECYCLE_JOURNAL_DIRECTORY)
}

pub(crate) fn persist_project_lifecycle_journal(
    journal: &ProjectLifecycleJournal,
) -> Result<(), String> {
    persist_project_lifecycle_journal_at(&app_data_root(), journal)
}

fn persist_project_lifecycle_journal_at(
    data_root: &Path,
    journal: &ProjectLifecycleJournal,
) -> Result<(), String> {
    let directory = lifecycle_journal_directory(data_root);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create project lifecycle journal: {error}"))?;
    secure_directory(&directory)
        .map_err(|error| format!("failed to secure project lifecycle journal: {error}"))?;
    let payload = serde_json::to_vec(journal)
        .map_err(|error| format!("failed to encode project lifecycle journal: {error}"))?;
    write_private_file_atomically(
        &journal.path_in(data_root),
        &payload,
        "project lifecycle journal",
    )
}

pub(crate) fn complete_project_lifecycle_journal(
    journal: &ProjectLifecycleJournal,
) -> Result<(), String> {
    complete_project_lifecycle_journal_at(&app_data_root(), journal)
}

fn complete_project_lifecycle_journal_at(
    data_root: &Path,
    journal: &ProjectLifecycleJournal,
) -> Result<(), String> {
    let path = journal.path_in(data_root);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to remove project lifecycle journal {}: {error}",
            path.display()
        )),
    }
}

fn load_project_lifecycle_journals_at(
    data_root: &Path,
) -> Result<Vec<ProjectLifecycleJournal>, String> {
    let directory = lifecycle_journal_directory(data_root);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "failed to read project lifecycle journal {}: {error}",
                directory.display()
            ))
        }
    };
    let mut paths = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("failed to inspect project lifecycle journal: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().and_then(|value| value.to_str()) == Some("json"));
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let payload = fs::read(&path).map_err(|error| {
                format!(
                    "failed to read project lifecycle journal {}: {error}",
                    path.display()
                )
            })?;
            let journal =
                serde_json::from_slice::<ProjectLifecycleJournal>(&payload).map_err(|error| {
                    format!(
                        "invalid project lifecycle journal {}: {error}",
                        path.display()
                    )
                })?;
            if journal.schema != PROJECT_LIFECYCLE_JOURNAL_SCHEMA
                || journal.path_in(data_root) != path
            {
                return Err(format!(
                    "invalid project lifecycle journal identity: {}",
                    path.display()
                ));
            }
            Ok(journal)
        })
        .collect()
}

pub(crate) fn recover_project_lifecycle_operations(
    store: &mut SqliteStore,
    config: &ProjectSessionConfig,
) -> Result<Vec<RecoveredMemoryRefresh>, String> {
    recover_project_lifecycle_operations_at(&app_data_root(), store, config)
}

fn recover_project_lifecycle_operations_at(
    data_root: &Path,
    store: &mut SqliteStore,
    config: &ProjectSessionConfig,
) -> Result<Vec<RecoveredMemoryRefresh>, String> {
    let mut refreshes = Vec::new();
    for journal in load_project_lifecycle_journals_at(data_root)? {
        let mut refresh_pending = false;
        match &journal.operation {
            ProjectLifecycleOperation::Fork {
                project_root,
                target_session_id,
                ..
            } => {
                let published = config
                    .sessions
                    .iter()
                    .any(|session| session.id == *target_session_id);
                if !published {
                    rollback_unpublished_fork(store, project_root, target_session_id)?;
                }
            }
            ProjectLifecycleOperation::Delete {
                project_id,
                project_root,
                session_ids,
                delete_project,
            } => {
                let published = if *delete_project {
                    !config
                        .projects
                        .iter()
                        .any(|project| project.id == *project_id)
                } else {
                    session_ids.iter().all(|session_id| {
                        !config
                            .sessions
                            .iter()
                            .any(|session| session.id == *session_id)
                    })
                };
                if published {
                    let retirement =
                        plan_managed_artifact_retirement(store, project_root, session_ids)?;
                    retire_managed_browser_sessions(&retirement)?;
                    apply_managed_artifact_retirement(&retirement)?;
                    if let Some(ledger) = cleanup_published_delete(
                        store,
                        project_id,
                        project_root,
                        session_ids,
                        *delete_project,
                    )? {
                        refreshes.push(RecoveredMemoryRefresh {
                            project_root: project_root.clone(),
                            ledger,
                            journal: journal.clone(),
                        });
                        refresh_pending = true;
                    }
                }
            }
        }
        if !refresh_pending {
            complete_project_lifecycle_journal_at(data_root, &journal)?;
        }
    }
    Ok(refreshes)
}

pub(crate) fn load_forkable_session_events(
    store: &SqliteStore,
    source_session_id: &str,
) -> Result<Vec<Event>, StorageError> {
    let events = store.list_by_task(&phase16_task_id())?;
    Ok(agent_session_events(&events, source_session_id))
}

pub(crate) fn clone_fork_attachments_and_rewrite_events(
    events: &mut [Event],
    source_attachment_dir: &Path,
    target_attachment_dir: &Path,
) -> Result<(), String> {
    if source_attachment_dir.exists()
        && !fs::symlink_metadata(source_attachment_dir)
            .map_err(|error| format!("failed to inspect source attachment directory: {error}"))?
            .file_type()
            .is_dir()
    {
        return Err("source attachment directory is not a regular directory".to_string());
    }
    if target_attachment_dir.exists()
        && !fs::symlink_metadata(target_attachment_dir)
            .map_err(|error| format!("failed to inspect target attachment directory: {error}"))?
            .file_type()
            .is_dir()
    {
        return Err("target attachment directory is not a regular directory".to_string());
    }
    let mut mappings = BTreeMap::<String, String>::new();
    for event in events.iter() {
        for key in ["attachment_paths", "image_paths"] {
            let Some(paths) = event.metadata.get(key) else {
                continue;
            };
            for path in paths.lines().filter(|path| !path.trim().is_empty()) {
                let source = Path::new(path);
                let Ok(relative) = source.strip_prefix(source_attachment_dir) else {
                    continue;
                };
                validate_direct_attachment_relative(relative)?;
                let target = target_attachment_dir.join(relative);
                clone_managed_attachment(source, &target)?;
                mappings.insert(path.to_string(), target.display().to_string());
            }
        }
        if let Some(payload) = event
            .metadata
            .get("queue_payload")
            .and_then(|value| serde_json::from_str::<QueuedAgentMessagePayload>(value).ok())
        {
            for attachment in payload.attachments {
                let source = Path::new(&attachment.path);
                let Ok(relative) = source.strip_prefix(source_attachment_dir) else {
                    continue;
                };
                validate_direct_attachment_relative(relative)?;
                let target = target_attachment_dir.join(relative);
                clone_managed_attachment(source, &target)?;
                mappings.insert(attachment.path, target.display().to_string());
            }
        }
    }
    for event in events {
        rewrite_fork_event_metadata(&mut event.metadata, &mappings);
    }
    Ok(())
}

fn validate_direct_attachment_relative(relative: &Path) -> Result<(), String> {
    let mut components = relative.components();
    let direct_file = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if !direct_file {
        return Err("fork attachment path is not a direct managed file".to_string());
    }
    Ok(())
}

fn clone_managed_attachment(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        format!(
            "failed to inspect fork attachment {}: {error}",
            source.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "fork attachment is not a regular file: {}",
            source.display()
        ));
    }
    let parent = target
        .parent()
        .ok_or_else(|| "fork attachment has no parent directory".to_string())?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(format!(
                "fork attachment target directory is not a regular directory: {}",
                parent.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create fork attachment directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "failed to inspect fork attachment directory {}: {error}",
                parent.display()
            ));
        }
    }
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_file() => return Ok(()),
        Ok(_) => {
            return Err(format!(
                "fork attachment target is not a regular file: {}",
                target.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "failed to inspect fork attachment target {}: {error}",
                target.display()
            ));
        }
    }
    fs::copy(source, target).map_err(|error| {
        format!(
            "failed to copy fork attachment {}: {error}",
            source.display()
        )
    })?;
    Ok(())
}

fn rewrite_fork_event_metadata(metadata: &mut Metadata, mappings: &BTreeMap<String, String>) {
    for key in ["attachment_paths", "image_paths"] {
        if let Some(paths) = metadata.get_mut(key) {
            *paths = paths
                .lines()
                .map(|path| mappings.get(path).map(String::as_str).unwrap_or(path))
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    if let Some(value) = metadata.get_mut("model_content") {
        *value = replace_exact_attachment_paths(value, mappings);
    }
    if let Some(value) = metadata.get_mut("queue_payload") {
        *value = rewrite_queue_payload(value, mappings);
    }
}

fn scrub_fork_recovery_metadata(metadata: &mut Metadata) {
    for key in FORK_RECOVERY_METADATA_KEYS {
        metadata.remove(key);
    }
}

fn replace_exact_attachment_paths(value: &str, mappings: &BTreeMap<String, String>) -> String {
    let mut replacements = mappings.iter().collect::<Vec<_>>();
    replacements.sort_by_key(|(source, _)| std::cmp::Reverse(source.len()));
    replacements
        .into_iter()
        .fold(value.to_string(), |current, (source, target)| {
            replace_path_token(&current, source, target)
        })
}

fn replace_path_token(value: &str, source: &str, target: &str) -> String {
    let mut rewritten = String::with_capacity(value.len());
    let mut cursor = 0;
    for (offset, _) in value.match_indices(source) {
        let end = offset + source.len();
        let before_is_path = value[..offset]
            .chars()
            .next_back()
            .is_some_and(path_token_character);
        let after_is_path = value[end..]
            .chars()
            .next()
            .is_some_and(path_token_character);
        if before_is_path || after_is_path {
            continue;
        }
        rewritten.push_str(&value[cursor..offset]);
        rewritten.push_str(target);
        cursor = end;
    }
    rewritten.push_str(&value[cursor..]);
    rewritten
}

fn path_token_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '/' | '\\' | '_' | '-' | '.')
}

fn rewrite_queue_payload(value: &str, mappings: &BTreeMap<String, String>) -> String {
    let Ok(mut payload) = serde_json::from_str::<QueuedAgentMessagePayload>(value) else {
        return replace_exact_attachment_paths(value, mappings);
    };
    payload.prompt = replace_exact_attachment_paths(&payload.prompt, mappings);
    for attachment in &mut payload.attachments {
        if let Some(target) = mappings.get(&attachment.path) {
            attachment.path = target.clone();
        }
    }
    serde_json::to_string(&payload).unwrap_or_else(|_| value.to_string())
}

pub(crate) fn persist_fork_events(
    store: &mut SqliteStore,
    events: &[Event],
    fork_metadata: &Metadata,
    source_session_id: &str,
) -> Result<(), StorageError> {
    persist_fork_events_with(store, events, fork_metadata, source_session_id, |_, _| {
        Ok(())
    })
}

fn persist_fork_events_with(
    store: &mut SqliteStore,
    events: &[Event],
    fork_metadata: &Metadata,
    source_session_id: &str,
    mut after_append: impl FnMut(usize, &mut SqliteStore) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let active_events = active_agent_events_for_session(events, Some(source_session_id));
    let detach_active_run = matches!(
        AgentRunStatus::from_events(&active_events, false, false),
        AgentRunStatus::Running | AgentRunStatus::WaitingForPermission | AgentRunStatus::Paused
    );
    let pending_queue_closures = pending_queued_agent_messages(events, source_session_id)
        .into_iter()
        .map(|pending| {
            (
                pending.view.id,
                pending.view.mode,
                pending.view.created_at_ms,
            )
        })
        .collect::<Vec<_>>();
    store.with_immediate_transaction(|transaction| {
        let mut append_index = 0usize;
        for event in events {
            let mut metadata = event.metadata.clone();
            scrub_fork_recovery_metadata(&mut metadata);
            metadata.extend(fork_metadata.clone());
            append_event(
                transaction,
                &phase16_task_id(),
                event.kind.clone(),
                event.summary.clone(),
                metadata,
            )?;
            after_append(append_index, transaction)?;
            append_index = append_index.saturating_add(1);
        }
        for (queue_id, queue_mode, created_at_ms) in &pending_queue_closures {
            let mut metadata = fork_metadata.clone();
            metadata.extend([
                ("queue_action".to_string(), "delete".to_string()),
                ("queue_id".to_string(), queue_id.clone()),
                ("queue_mode".to_string(), queue_mode.clone()),
                ("queue_created_at_ms".to_string(), created_at_ms.to_string()),
                ("fork_snapshot".to_string(), "true".to_string()),
            ]);
            append_event(
                transaction,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                "Queued agent message deleted",
                metadata,
            )?;
            after_append(append_index, transaction)?;
            append_index = append_index.saturating_add(1);
        }
        if detach_active_run {
            let mut metadata = fork_metadata.clone();
            metadata.extend([
                ("fork_snapshot".to_string(), "true".to_string()),
                (
                    "fork_snapshot_reason".to_string(),
                    "detached_source_run".to_string(),
                ),
            ]);
            insert_event_type_v1(
                &EventKind::TaskStatusChanged,
                &mut metadata,
                EventTypeV1::AgentRunCancelled,
            )
            .map_err(|error| StorageError::new(error.to_string()))?;
            append_event(
                transaction,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                "Fork snapshot detached",
                metadata,
            )?;
            after_append(append_index, transaction)?;
        }
        Ok(())
    })
}

pub(crate) fn rollback_unpublished_fork(
    store: &mut SqliteStore,
    project_root: &Path,
    target_session_id: &str,
) -> Result<(), String> {
    store
        .with_immediate_transaction(|transaction| {
            delete_session_storage_in_transaction(transaction, target_session_id)
        })
        .map_err(|error| error.to_string())?;
    remove_directory_if_present(&managed_attachment_dir(project_root, target_session_id))?;
    remove_context_artifacts(project_root, target_session_id)
}

pub(crate) fn cleanup_published_delete(
    store: &mut SqliteStore,
    project_id: &str,
    project_root: &Path,
    session_ids: &[String],
    delete_project: bool,
) -> Result<Option<MemoryLedger>, String> {
    let mut ledger = if delete_project {
        cleanup_deleted_project_storage(store, project_id, session_ids)?;
        None
    } else {
        Some(cleanup_deleted_session_storage(
            store,
            project_id,
            session_ids,
        )?)
    };
    for session_id in session_ids {
        remove_directory_if_present(&managed_attachment_dir(project_root, session_id))?;
        remove_context_artifacts(project_root, session_id)?;
    }
    if delete_project {
        delete_project_memory_vector_index(project_root, project_id)?;
    } else if let Some(current) = ledger.as_mut() {
        purge_project_memory_vector_history(project_root, current)?;
        current.vector_history_reset_required = false;
        store
            .with_immediate_transaction(|transaction| {
                save_project_memory_ledger(transaction, current)
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(ledger)
}

fn cleanup_deleted_project_storage(
    store: &mut SqliteStore,
    project_id: &str,
    session_ids: &[String],
) -> Result<(), String> {
    store
        .with_immediate_transaction(|transaction| {
            for session_id in session_ids {
                delete_session_storage_in_transaction(transaction, session_id)?;
            }
            transaction.delete_records_by_metadata_in_transaction("project_id", project_id)?;
            delete_project_memory_read_models(transaction, project_id)?;
            delete_global_projection_caches(transaction)
        })
        .map_err(|error| error.to_string())
}

fn cleanup_deleted_session_storage(
    store: &mut SqliteStore,
    project_id: &str,
    session_ids: &[String],
) -> Result<MemoryLedger, String> {
    store
        .with_immediate_transaction(|transaction| {
            let ledger = load_project_memory_ledger(transaction, project_id)?;
            retain_memory_records_for_deleted_sessions(transaction, &ledger, session_ids)?;
            persist_memory_session_retirements(transaction, project_id, session_ids)?;
            for session_id in session_ids {
                delete_session_storage_in_transaction(transaction, session_id)?;
            }
            delete_project_memory_read_models(transaction, project_id)?;
            let mut rebuilt = load_project_memory_ledger(transaction, project_id)?;
            rebuilt.vector_history_reset_required = true;
            save_project_memory_ledger(transaction, &rebuilt)?;
            delete_global_projection_caches(transaction)?;
            Ok(rebuilt)
        })
        .map_err(|error| error.to_string())
}

fn delete_project_memory_read_models(
    store: &mut SqliteStore,
    project_id: &str,
) -> Result<(), StorageError> {
    for namespace in [
        LEGACY_AGENT_MEMORY_READ_MODEL_NAMESPACE,
        AGENT_MEMORY_READ_MODEL_NAMESPACE,
    ] {
        store.delete_read_model(namespace, project_id)?;
    }
    Ok(())
}

fn delete_session_storage_in_transaction(
    store: &mut SqliteStore,
    session_id: &str,
) -> Result<(), StorageError> {
    store.delete_records_by_metadata_in_transaction("session_id", session_id)?;
    for namespace in [
        AGENT_SESSION_READ_MODEL_NAMESPACE,
        AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
        AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
    ] {
        store.delete_read_model(namespace, session_id)?;
    }
    Ok(())
}

fn delete_global_projection_caches(store: &mut SqliteStore) -> Result<(), StorageError> {
    for namespace in [
        LEGACY_ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
        ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
    ] {
        store.delete_read_model(namespace, ROUTING_TELEMETRY_READ_MODEL_KEY)?;
    }
    Ok(())
}

fn managed_attachment_dir(project_root: &Path, session_id: &str) -> PathBuf {
    project_root
        .join(".cindx")
        .join("attachments")
        .join(crate::runtime_values::slug_label(session_id))
}

fn remove_context_artifacts(project_root: &Path, session_id: &str) -> Result<(), String> {
    for path in [
        context_checkpoint_path_for_session(project_root, Some(session_id)),
        context_checkpoint_manifest_path_for_session(project_root, Some(session_id)),
    ] {
        remove_file_if_present(&path, "session context")?;
    }
    Ok(())
}

fn remove_directory_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("failed to remove {}: {error}", path.display())),
    }
}

fn remove_file_if_present(path: &Path, label: &str) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to remove {label} {}: {error}",
            path.display()
        )),
    }
}

/// The published-delete cleanup that both a project delete and a session delete
/// run: managed-artifact retirement, browser session retirement, and the durable
/// cleanup itself. Owned here rather than in either command module so the two
/// never depend on each other.
pub(crate) fn cleanup_published_delete_with_managed_artifacts(
    state: &tauri::State<'_, AppState>,
    project_id: &str,
    project_root: &Path,
    session_ids: &[String],
    delete_project: bool,
) -> Result<Option<MemoryLedger>, String> {
    let initial_retirement = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        plan_managed_artifact_retirement(&store, project_root, session_ids)?
    };
    retire_managed_browser_sessions(&initial_retirement)?;

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let retirement = plan_managed_artifact_retirement(&store, project_root, session_ids)?;
    apply_managed_artifact_retirement(&retirement)?;
    cleanup_published_delete(
        &mut store,
        project_id,
        project_root,
        session_ids,
        delete_project,
    )
}

#[cfg(test)]
#[path = "project_lifecycle_runtime_tests.rs"]
mod tests;
