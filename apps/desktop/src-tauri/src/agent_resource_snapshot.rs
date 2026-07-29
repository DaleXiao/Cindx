use crate::{
    app_state::AppState,
    runtime_constants::AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
    runtime_values::{current_time_millis, phase16_task_id},
};
use agent_core::Metadata;
use agent_runtime::{AgentRunControl, RunResourceSnapshot};
use agent_storage::SqliteStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedAgentResourceSnapshot {
    schema: String,
    session_id: String,
    project_id: Option<String>,
    source_run_id: String,
    event_revision: u64,
    steer_epoch: u64,
    resources: RunResourceSnapshot,
    updated_at_ms: u64,
}

pub(super) fn persist_agent_resource_snapshot(
    store: &mut SqliteStore,
    run_context: &Metadata,
    resources: &RunResourceSnapshot,
) -> Result<(), String> {
    let Some(session_id) = run_context.get("session_id") else {
        return Ok(());
    };
    let Some(source_run_id) = run_context
        .get("agent_run_id")
        .filter(|run_id| !run_id.trim().is_empty())
    else {
        return Ok(());
    };
    if !resources.is_within_persistence_bounds() {
        return Err("agent resource checkpoint exceeds persistence bounds".to_string());
    }
    if let Some(stored) = store
        .load_read_model(AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE, session_id)
        .map_err(|error| error.to_string())?
    {
        let existing = serde_json::from_str::<PersistedAgentResourceSnapshot>(&stored.payload).ok();
        let existing_is_newer = existing.as_ref().is_some_and(|existing| {
            existing.schema == AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE
                && existing.session_id.as_str() == session_id.as_str()
                && existing.project_id == run_context.get("project_id").cloned()
                && existing.source_run_id.as_str() == source_run_id.as_str()
                && existing.event_revision == stored.revision
                && existing.resources.is_within_persistence_bounds()
                && existing.resources.mutation_revision() > resources.mutation_revision()
        });
        if existing_is_newer {
            return Ok(());
        }
    }
    let revision = store
        .event_revision_by_metadata(&phase16_task_id(), "session_id", session_id)
        .map_err(|error| error.to_string())?
        .latest_sequence;
    let snapshot = PersistedAgentResourceSnapshot {
        schema: AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE.to_string(),
        session_id: session_id.clone(),
        project_id: run_context.get("project_id").cloned(),
        source_run_id: source_run_id.clone(),
        event_revision: revision,
        steer_epoch: run_context
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default(),
        resources: resources.clone(),
        updated_at_ms: current_time_millis(),
    };
    let payload = serde_json::to_string(&snapshot)
        .map_err(|error| format!("failed to encode agent resource checkpoint: {error}"))?;
    store
        .save_read_model(
            AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE,
            session_id,
            revision,
            &payload,
        )
        .map_err(|error| error.to_string())
}

pub(super) fn checkpoint_agent_run_resources(
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    control: &AgentRunControl,
) -> Result<(), String> {
    let resources = control.resource_usage();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    persist_agent_resource_snapshot(&mut store, run_context, &resources)
}

pub(super) fn load_matching_agent_resource_snapshot(
    store: &SqliteStore,
    run_context: &Metadata,
    source_run_id: &str,
    latest_revision: u64,
) -> Result<Option<RunResourceSnapshot>, String> {
    let Some(session_id) = run_context.get("session_id") else {
        return Ok(None);
    };
    let Some(stored) = store
        .load_read_model(AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE, session_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let snapshot = match serde_json::from_str::<PersistedAgentResourceSnapshot>(&stored.payload) {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(None),
    };
    let matches = snapshot.schema.as_str() == AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE
        && snapshot.session_id.as_str() == session_id.as_str()
        && snapshot.project_id == run_context.get("project_id").cloned()
        && snapshot.source_run_id.as_str() == source_run_id
        && snapshot.event_revision == stored.revision
        && snapshot.event_revision <= latest_revision
        && snapshot.resources.is_within_persistence_bounds();
    Ok(matches.then_some(snapshot.resources))
}

pub(super) fn delete_persisted_agent_resource_snapshot(
    store: &mut SqliteStore,
    session_id: &str,
) -> Result<(), String> {
    store
        .delete_read_model(AGENT_RESOURCE_SNAPSHOT_READ_MODEL_NAMESPACE, session_id)
        .map_err(|error| error.to_string())
}
