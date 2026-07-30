use crate::{
    agent_resource_snapshot::delete_persisted_agent_resource_snapshot,
    agent_runtime_snapshot_cursor::{AgentRuntimeSnapshotCursor, PreparedAgentRuntimeSnapshot},
    event_security::{redact_metadata, redact_sensitive_text},
    runtime_constants::AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
    runtime_values::{current_time_millis, phase16_task_id},
};
use agent_core::{MessageRole, Metadata};
use agent_runtime::{sanitize_assistant_content, AgentTaskStateSnapshot};
use agent_storage::SqliteStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedAgentRuntimeSnapshot {
    schema: String,
    session_id: String,
    project_id: Option<String>,
    source_run_id: String,
    prompt_fingerprint: String,
    event_revision: u64,
    task_state: AgentTaskStateSnapshot,
    updated_at_ms: u64,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn persist_agent_runtime_snapshot(
    store: &mut SqliteStore,
    runtime: &agent_runtime::AgentLoopState,
    run_context: &Metadata,
) -> Result<(), String> {
    let cursor = AgentRuntimeSnapshotCursor::rebuild(runtime, run_context);
    let (prepared, _) = cursor.prepare_after_append(runtime, runtime.messages.len(), run_context);
    persist_prepared_agent_runtime_snapshot(store, &prepared)
}

pub(super) fn persist_prepared_agent_runtime_snapshot(
    store: &mut SqliteStore,
    prepared: &PreparedAgentRuntimeSnapshot,
) -> Result<(), String> {
    let Some(identity) = prepared.identity.as_ref() else {
        return Ok(());
    };
    let revision = store
        .event_revision_by_metadata(&phase16_task_id(), "session_id", &identity.session_id)
        .map_err(|error| error.to_string())?
        .latest_sequence;
    let snapshot = PersistedAgentRuntimeSnapshot {
        schema: AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE.to_string(),
        session_id: identity.session_id.clone(),
        project_id: identity.project_id.clone(),
        source_run_id: identity.source_run_id.clone(),
        prompt_fingerprint: prepared.task_state.user_prompt_fingerprint.clone(),
        event_revision: revision,
        task_state: prepared.task_state.clone(),
        updated_at_ms: current_time_millis(),
    };
    let payload = serde_json::to_string(&snapshot)
        .map_err(|error| format!("failed to encode agent runtime snapshot: {error}"))?;
    store
        .save_read_model(
            AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
            &identity.session_id,
            revision,
            &payload,
        )
        .map_err(|error| error.to_string())
}

pub(super) fn capture_persistable_agent_task_state(
    runtime: &agent_runtime::AgentLoopState,
) -> AgentTaskStateSnapshot {
    let mut projection = runtime.clone();
    projection.user_prompt = redact_sensitive_text(&projection.user_prompt);
    for message in &mut projection.messages {
        if message.role == MessageRole::Assistant {
            message.content = sanitize_assistant_content(&message.content);
            if let Some(display_content) = message.metadata.get_mut("display_content") {
                *display_content = sanitize_assistant_content(display_content);
            }
        }
        message.content = redact_sensitive_text(&message.content);
        message.metadata = redact_metadata(&message.metadata);
    }
    AgentTaskStateSnapshot::capture(&projection)
}

pub(super) fn delete_persisted_agent_runtime_snapshot(
    store: &mut SqliteStore,
    session_id: Option<&str>,
) -> Result<(), String> {
    let Some(session_id) = session_id else {
        return Ok(());
    };
    let runtime = store
        .delete_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, session_id)
        .map_err(|error| error.to_string());
    let resources = delete_persisted_agent_resource_snapshot(store, session_id);
    runtime.and(resources)
}

pub(super) fn load_matching_agent_runtime_snapshot(
    store: &SqliteStore,
    run_context: &Metadata,
    source_run_id: &str,
    prompt_fingerprint: &str,
    latest_revision: u64,
) -> Result<Option<AgentTaskStateSnapshot>, String> {
    let Some(session_id) = run_context.get("session_id") else {
        return Ok(None);
    };
    let Some(stored) = store
        .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, session_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let snapshot = match serde_json::from_str::<PersistedAgentRuntimeSnapshot>(&stored.payload) {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(None),
    };
    let matches = snapshot.schema.as_str() == AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE
        && snapshot.session_id.as_str() == session_id.as_str()
        && snapshot.project_id == run_context.get("project_id").cloned()
        && snapshot.source_run_id.as_str() == source_run_id
        && snapshot.prompt_fingerprint.as_str() == prompt_fingerprint
        && snapshot.event_revision == stored.revision
        && snapshot.event_revision <= latest_revision;
    Ok(matches.then_some(snapshot.task_state))
}

#[cfg(test)]
#[path = "agent_runtime_snapshot_tests.rs"]
mod tests;
