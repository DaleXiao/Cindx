use crate::{
    app_state::AppState,
    event_persistence::{append_event, append_message_event_with_metadata},
    memory_runtime::{
        delete_project_memory_vector_index, load_project_memory_ledger,
        load_project_memory_ledger_with_status, purge_project_memory_vector_history,
        save_project_memory_ledger, schedule_project_memory_vector_refresh,
    },
    memory_vector_generation_runtime::memory_vector_projection_sha256,
    runtime_constants::AGENT_MEMORY_READ_MODEL_NAMESPACE,
    runtime_values::{phase16_task_id, unique_id},
};
use agent_core::{Event, EventKind, MessageRole, Metadata};
use agent_memory::{
    is_memory_user_confirmation_event, memory_content_sha256, memory_settings_session_id,
    validate_memory_user_confirmation, MemoryControlAction, MemoryLedger, MemoryRecord,
    MEMORY_CONTROL_SCHEMA, MEMORY_USER_CONFIRMATION_SCHEMA,
};
use agent_storage::{SqliteStore, StorageError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) fn delete_project_memory(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    project_id: &str,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .delete_records_by_metadata("project_id", project_id)
        .map_err(|error| error.to_string())?;
    store
        .delete_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, project_id)
        .map_err(|error| error.to_string())?;
    drop(store);
    delete_project_memory_vector_index(workspace_root, project_id)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateProjectMemoryInput {
    pub(crate) project_id: String,
    pub(crate) memory_id: String,
    pub(crate) action: String,
    pub(crate) expected_item_revision: u64,
    pub(crate) expected_content_sha256: String,
    pub(crate) confirmed_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectMemoryItemView {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) trust: String,
    pub(crate) content: String,
    pub(crate) state: String,
    pub(crate) pinned: bool,
    pub(crate) item_revision: u64,
    pub(crate) content_sha256: String,
    pub(crate) importance: u8,
    pub(crate) source_session_ids: Vec<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) recall_count: u64,
    pub(crate) observed_use_count: u64,
    pub(crate) superseded_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quarantine_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectMemoryStateView {
    pub(crate) project_id: String,
    pub(crate) revision: u64,
    pub(crate) items: Vec<ProjectMemoryItemView>,
    pub(crate) active_count: usize,
    pub(crate) disabled_count: usize,
    pub(crate) quarantined_count: usize,
    pub(crate) pinned_count: usize,
}

#[derive(Debug, Clone)]
struct MemoryCommandContext {
    project_id: String,
    project_name: String,
    project_root: PathBuf,
    session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestedMemoryAction {
    Delete,
    Disable,
    Enable,
    Pin,
    Unpin,
    Promote,
}

impl RequestedMemoryAction {
    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "delete" => Ok(Self::Delete),
            "disable" => Ok(Self::Disable),
            "enable" => Ok(Self::Enable),
            "pin" => Ok(Self::Pin),
            "unpin" => Ok(Self::Unpin),
            "promote" => Ok(Self::Promote),
            _ => Err(StorageError::new("unknown project memory action")),
        }
    }

    fn control_action(self) -> Option<MemoryControlAction> {
        match self {
            Self::Delete => Some(MemoryControlAction::Delete),
            Self::Disable => Some(MemoryControlAction::Disable),
            Self::Enable => Some(MemoryControlAction::Enable),
            Self::Pin => Some(MemoryControlAction::Pin),
            Self::Unpin => Some(MemoryControlAction::Unpin),
            Self::Promote => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Disable => "disable",
            Self::Enable => "enable",
            Self::Pin => "pin",
            Self::Unpin => "unpin",
            Self::Promote => "promote",
        }
    }
}

#[tauri::command]
pub(crate) fn get_project_memory_state(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<ProjectMemoryStateView, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    if config.active_project_id != project_id {
        return Err("project memory request does not match the active project".to_string());
    }
    let project_root = config
        .active_project()
        .map(|project| PathBuf::from(&project.root))
        .ok_or_else(|| "active project is unavailable".to_string())?;
    drop(config);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let mut loaded = load_project_memory_ledger_with_status(&mut store, &project_id)
        .map_err(|error| error.to_string())?;
    let reset_vectors = loaded.vector_reset_required;
    if reset_vectors {
        purge_project_memory_vector_history(&project_root, &loaded.ledger)?;
        loaded.ledger.vector_history_reset_required = false;
        save_project_memory_ledger(&mut store, &loaded.ledger)
            .map_err(|error| error.to_string())?;
    }
    let ledger = loaded.ledger;
    drop(store);
    if reset_vectors {
        let provider_config = state
            .provider_config
            .lock()
            .map_err(|error| format!("provider config lock poisoned: {error}"))?
            .clone();
        schedule_project_memory_vector_refresh(project_root, provider_config, ledger.clone());
    }
    Ok(project_memory_state_from_ledger(&ledger))
}

#[tauri::command]
pub(crate) fn update_project_memory(
    state: tauri::State<'_, AppState>,
    input: UpdateProjectMemoryInput,
) -> Result<ProjectMemoryStateView, String> {
    let provider_config = state
        .provider_config
        .lock()
        .map_err(|error| format!("provider config lock poisoned: {error}"))?
        .clone();
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    if config.active_project_id != input.project_id {
        return Err("project memory request does not match the active project".to_string());
    }
    let project = config
        .active_project()
        .ok_or_else(|| "active project is unavailable".to_string())?;
    let context = MemoryCommandContext {
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        project_root: PathBuf::from(&project.root),
        session_id: memory_settings_session_id(&project.id),
    };
    drop(config);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let (mut ledger, response, vector_changed) = store
        .with_immediate_transaction(|store| {
            update_project_memory_transaction(store, &context, &input)
        })
        .map_err(|error| error.to_string())?;
    let purge_vector_history = input.action == "delete" || ledger.vector_history_reset_required;
    if purge_vector_history {
        ledger.vector_history_reset_required = true;
        save_project_memory_ledger(&mut store, &ledger).map_err(|error| error.to_string())?;
        purge_project_memory_vector_history(&context.project_root, &ledger)?;
        ledger.vector_history_reset_required = false;
        save_project_memory_ledger(&mut store, &ledger).map_err(|error| error.to_string())?;
    }
    drop(store);
    if vector_changed || purge_vector_history {
        schedule_project_memory_vector_refresh(context.project_root, provider_config, ledger);
    }
    Ok(response)
}

fn update_project_memory_transaction(
    store: &mut SqliteStore,
    context: &MemoryCommandContext,
    input: &UpdateProjectMemoryInput,
) -> Result<(MemoryLedger, ProjectMemoryStateView, bool), StorageError> {
    validate_memory_mutation_identity(context, input)?;
    let action = RequestedMemoryAction::parse(&input.action)?;
    let before = load_project_memory_ledger(store, &context.project_id)?;
    let current = current_memory_item(&before, &input.memory_id)
        .ok_or_else(|| StorageError::new("project memory item is no longer available"))?;
    if current.item_revision != input.expected_item_revision
        || current.content_sha256 != input.expected_content_sha256
    {
        return Err(StorageError::new(
            "project memory item changed; refresh before trying again",
        ));
    }
    validate_requested_action(&before, &current, action)?;
    if requested_action_is_noop(&before, &current, action) {
        let response = project_memory_state_from_ledger(&before);
        return Ok((before, response, false));
    }

    let before_vector = memory_vector_projection_sha256(&before);
    let confirmation_run_id = if action == RequestedMemoryAction::Promote {
        Some(append_memory_confirmation_event(store, context, input)?)
    } else {
        append_memory_control_event(store, context, input, action)?;
        None
    };
    let after = load_project_memory_ledger(store, &context.project_id)?;
    if let Some(run_id) = confirmation_run_id {
        let confirmed = input
            .confirmed_content
            .as_deref()
            .map(str::trim)
            .unwrap_or_default();
        let created = after.records.iter().any(|record| {
            record.provenance.agent_run_id.as_deref() == Some(run_id.as_str())
                && record.content == confirmed
                && record.is_recall_eligible()
        });
        let source_remains = after
            .quarantined_records
            .iter()
            .any(|item| item.record.id == input.memory_id);
        if !created || source_remains {
            return Err(StorageError::new(
                "confirmed text is not a safe single project requirement",
            ));
        }
    }
    let vector_changed = before_vector != memory_vector_projection_sha256(&after);
    let response = project_memory_state_from_ledger(&after);
    Ok((after, response, vector_changed))
}

fn validate_memory_mutation_identity(
    context: &MemoryCommandContext,
    input: &UpdateProjectMemoryInput,
) -> Result<(), StorageError> {
    if input.project_id != context.project_id
        || input.memory_id.is_empty()
        || input.memory_id.len() > 256
        || input.expected_content_sha256.len() != 64
        || !input
            .expected_content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StorageError::new(
            "invalid project memory mutation identity",
        ));
    }
    Ok(())
}

fn validate_requested_action(
    ledger: &MemoryLedger,
    current: &ProjectMemoryItemView,
    action: RequestedMemoryAction,
) -> Result<(), StorageError> {
    let active = current.state == "active";
    let disabled = current.state == "disabled";
    let quarantined = current.state == "quarantined";
    let allowed = match action {
        RequestedMemoryAction::Delete => true,
        RequestedMemoryAction::Disable | RequestedMemoryAction::Enable => active || disabled,
        RequestedMemoryAction::Pin | RequestedMemoryAction::Unpin => active,
        RequestedMemoryAction::Promote => quarantined,
    };
    if !allowed {
        return Err(StorageError::new(
            "project memory action is not allowed for this item",
        ));
    }
    if action == RequestedMemoryAction::Pin
        && !ledger
            .records
            .iter()
            .find(|record| record.id == current.id)
            .is_some_and(|record| ledger.record_is_active_for_recall(record))
    {
        return Err(StorageError::new(
            "only active verified memory can be pinned",
        ));
    }
    Ok(())
}

fn requested_action_is_noop(
    ledger: &MemoryLedger,
    current: &ProjectMemoryItemView,
    action: RequestedMemoryAction,
) -> bool {
    match action {
        RequestedMemoryAction::Disable => current.state == "disabled",
        RequestedMemoryAction::Enable => current.state == "active",
        RequestedMemoryAction::Pin => ledger.is_pinned(&current.id),
        RequestedMemoryAction::Unpin => !ledger.is_pinned(&current.id),
        RequestedMemoryAction::Delete | RequestedMemoryAction::Promote => false,
    }
}

fn append_memory_control_event(
    store: &mut SqliteStore,
    context: &MemoryCommandContext,
    input: &UpdateProjectMemoryInput,
    action: RequestedMemoryAction,
) -> Result<(), StorageError> {
    debug_assert!(action.control_action().is_some());
    append_event(
        store,
        &phase16_task_id(),
        EventKind::RetrievalPerformed,
        "Project memory preference changed",
        memory_command_metadata(context, input, action),
    )
}

fn append_memory_confirmation_event(
    store: &mut SqliteStore,
    context: &MemoryCommandContext,
    input: &UpdateProjectMemoryInput,
) -> Result<String, StorageError> {
    let confirmed = input
        .confirmed_content
        .as_deref()
        .and_then(validate_memory_user_confirmation)
        .ok_or_else(|| StorageError::new("confirm one safe project requirement at a time"))?;
    let run_id = unique_id("memory-confirmation");
    let mut metadata = memory_command_metadata(context, input, RequestedMemoryAction::Promote);
    metadata.insert("internal".to_string(), "true".to_string());
    metadata.insert(
        "memory_user_confirmation_schema".to_string(),
        MEMORY_USER_CONFIRMATION_SCHEMA.to_string(),
    );
    metadata.insert("agent_run_id".to_string(), run_id.clone());
    append_message_event_with_metadata(
        store,
        &phase16_task_id(),
        MessageRole::User,
        confirmed,
        metadata,
    )?;
    Ok(run_id)
}

fn memory_command_metadata(
    context: &MemoryCommandContext,
    input: &UpdateProjectMemoryInput,
    action: RequestedMemoryAction,
) -> Metadata {
    [
        (
            "memory_control_schema".to_string(),
            MEMORY_CONTROL_SCHEMA.to_string(),
        ),
        ("memory_action".to_string(), action.label().to_string()),
        ("memory_id".to_string(), input.memory_id.clone()),
        ("project_id".to_string(), context.project_id.clone()),
        ("project_name".to_string(), context.project_name.clone()),
        (
            "project_root".to_string(),
            context.project_root.display().to_string(),
        ),
        ("session_id".to_string(), context.session_id.clone()),
        ("actor".to_string(), "user".to_string()),
    ]
    .into_iter()
    .collect()
}

pub(crate) fn is_memory_management_event(event: &Event) -> bool {
    let Some(project_id) = event
        .metadata
        .get("project_id")
        .filter(|project_id| !project_id.is_empty())
    else {
        return false;
    };
    let Some(_memory_id) = event
        .metadata
        .get("memory_id")
        .filter(|memory_id| !memory_id.is_empty() && memory_id.len() <= 256)
    else {
        return false;
    };
    let common_contract = event
        .metadata
        .get("memory_control_schema")
        .map(String::as_str)
        == Some(MEMORY_CONTROL_SCHEMA)
        && event.metadata.get("actor").map(String::as_str) == Some("user")
        && event.metadata.get("session_id").map(String::as_str)
            == Some(memory_settings_session_id(project_id).as_str());
    if !common_contract {
        return false;
    }
    match event.metadata.get("memory_action").map(String::as_str) {
        Some("promote") => is_memory_user_confirmation_event(event),
        Some("delete" | "disable" | "enable" | "pin" | "unpin") => {
            event.kind == EventKind::RetrievalPerformed
                && event.summary == "Project memory preference changed"
                && event.metadata.get("internal").map(String::as_str) != Some("true")
        }
        _ => false,
    }
}

pub(crate) fn replay_project_memory_management(
    ledger: &mut MemoryLedger,
    event: &Event,
    project_id: &str,
) {
    if !is_memory_management_event(event)
        || event.metadata.get("project_id").map(String::as_str) != Some(project_id)
    {
        return;
    }
    let Some(memory_id) = event.metadata.get("memory_id") else {
        return;
    };
    let Some(action) = event.metadata.get("memory_action").map(String::as_str) else {
        return;
    };
    if action == "promote" {
        let created = ledger
            .records
            .iter()
            .any(|record| record.provenance.event_id == event.id.0 && record.is_recall_eligible());
        if created {
            ledger
                .quarantined_records
                .retain(|item| item.record.id != *memory_id);
        }
        return;
    }
    let action = match action {
        "delete" => MemoryControlAction::Delete,
        "disable" => MemoryControlAction::Disable,
        "enable" => MemoryControlAction::Enable,
        "pin" => MemoryControlAction::Pin,
        "unpin" => MemoryControlAction::Unpin,
        _ => return,
    };
    let _ = agent_memory::replay_memory_control(
        ledger,
        action,
        memory_id,
        event.sequence,
        event.timestamp_ms,
    );
}

pub(crate) fn project_memory_state_from_ledger(ledger: &MemoryLedger) -> ProjectMemoryStateView {
    let mut items = ledger
        .records
        .iter()
        .filter(|record| {
            ledger
                .controls
                .get(&record.id)
                .is_none_or(|control| !control.deleted)
        })
        .map(|record| active_memory_item(ledger, record))
        .chain(
            ledger
                .quarantined_records
                .iter()
                .filter(|item| {
                    ledger
                        .controls
                        .get(&item.record.id)
                        .is_none_or(|control| !control.deleted)
                })
                .map(|item| {
                    memory_item_view(
                        ledger,
                        &item.record,
                        "legacy_unverified",
                        "quarantined",
                        false,
                        Some(item.reason.clone()),
                    )
                }),
        )
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        memory_state_order(&left.state)
            .cmp(&memory_state_order(&right.state))
            .then_with(|| right.pinned.cmp(&left.pinned))
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.id.cmp(&right.id))
    });
    ProjectMemoryStateView {
        project_id: ledger.project_id.clone(),
        revision: ledger.revision,
        active_count: items.iter().filter(|item| item.state == "active").count(),
        disabled_count: items.iter().filter(|item| item.state == "disabled").count(),
        quarantined_count: items
            .iter()
            .filter(|item| item.state == "quarantined")
            .count(),
        pinned_count: items.iter().filter(|item| item.pinned).count(),
        items,
    }
}

fn current_memory_item(ledger: &MemoryLedger, memory_id: &str) -> Option<ProjectMemoryItemView> {
    project_memory_state_from_ledger(ledger)
        .items
        .into_iter()
        .find(|item| item.id == memory_id)
}

fn active_memory_item(ledger: &MemoryLedger, record: &MemoryRecord) -> ProjectMemoryItemView {
    let disabled = ledger
        .controls
        .get(&record.id)
        .is_some_and(|control| control.disabled);
    let state = if record.superseded_by.is_some() {
        "superseded"
    } else if disabled {
        "disabled"
    } else {
        "active"
    };
    memory_item_view(
        ledger,
        record,
        record.trust.label(),
        state,
        ledger.is_pinned(&record.id),
        None,
    )
}

fn memory_item_view(
    ledger: &MemoryLedger,
    record: &MemoryRecord,
    trust: &str,
    state: &str,
    pinned: bool,
    quarantine_reason: Option<String>,
) -> ProjectMemoryItemView {
    ProjectMemoryItemView {
        id: record.id.clone(),
        kind: record.kind.label().to_string(),
        trust: trust.to_string(),
        content: record.content.clone(),
        state: state.to_string(),
        pinned,
        item_revision: ledger
            .item_revision(&record.id)
            .unwrap_or(record.provenance.sequence),
        content_sha256: memory_content_sha256(&record.content),
        importance: record.importance,
        source_session_ids: record.source_session_ids.clone(),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
        recall_count: record.recall_count,
        observed_use_count: record.observed_use_count,
        superseded_by: record.superseded_by.clone(),
        quarantine_reason,
    }
}

fn memory_state_order(state: &str) -> u8 {
    match state {
        "active" => 0,
        "disabled" => 1,
        "superseded" => 2,
        "quarantined" => 3,
        _ => 4,
    }
}

#[cfg(test)]
#[path = "memory_management_runtime_tests.rs"]
mod tests;
