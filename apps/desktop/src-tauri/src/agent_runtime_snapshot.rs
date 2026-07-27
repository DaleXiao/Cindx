use crate::{
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

pub(super) fn persist_agent_runtime_snapshot(
    store: &mut SqliteStore,
    runtime: &agent_runtime::AgentLoopState,
    run_context: &Metadata,
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
    let revision = store
        .event_revision_by_metadata(&phase16_task_id(), "session_id", session_id)
        .map_err(|error| error.to_string())?
        .latest_sequence;
    let task_state = capture_persistable_agent_task_state(runtime);
    let snapshot = PersistedAgentRuntimeSnapshot {
        schema: AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE.to_string(),
        session_id: session_id.clone(),
        project_id: run_context.get("project_id").cloned(),
        source_run_id: source_run_id.clone(),
        prompt_fingerprint: task_state.user_prompt_fingerprint.clone(),
        event_revision: revision,
        task_state,
        updated_at_ms: current_time_millis(),
    };
    let payload = serde_json::to_string(&snapshot)
        .map_err(|error| format!("failed to encode agent runtime snapshot: {error}"))?;
    store
        .save_read_model(
            AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE,
            session_id,
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
    store
        .delete_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, session_id)
        .map_err(|error| error.to_string())
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
mod tests {
    use super::*;
    use crate::{
        agent_read_model::{active_agent_events_for_session, agent_events_for_session},
        agent_recovery_service::agent_recovery_identity,
        event_persistence::{append_event, append_message_event_with_metadata},
        project_session_persistence::metadata_with_context,
    };
    use agent_core::{EventKind, MessageRole};
    use agent_runtime::{start_agent_loop, AgentRuntimeConfig};

    fn run_context(session_id: &str, run_id: &str) -> Metadata {
        [
            ("session_id".to_string(), session_id.to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
            ("agent_effort".to_string(), "auto".to_string()),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn runtime_snapshot_is_overwritten_and_bound_to_the_active_run() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let context = run_context("session-a", "run-a");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            metadata_with_context(
                [("prompt".to_string(), "inspect workspace".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        )
        .expect("run start should persist");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            "inspect workspace",
            context.clone(),
        )
        .expect("user message should persist");
        let runtime = start_agent_loop(
            phase16_task_id(),
            "inspect workspace".to_string(),
            AgentRuntimeConfig::default(),
        );

        persist_agent_runtime_snapshot(&mut store, &runtime, &context)
            .expect("runtime snapshot should persist");
        let events = agent_events_for_session(&store, &phase16_task_id(), Some("session-a"))
            .expect("events should load");
        let active = active_agent_events_for_session(&events, Some("session-a"));
        let (_, source_run_id, _, prompt_fingerprint, _) =
            agent_recovery_identity(&active, &context).expect("identity should resolve");
        let latest_revision = active.last().map(|event| event.sequence).unwrap_or_default();
        assert!(load_matching_agent_runtime_snapshot(
            &store,
            &context,
            &source_run_id,
            &prompt_fingerprint,
            latest_revision,
        )
        .expect("snapshot should load")
        .is_some());

        let stale_context = run_context("session-a", "run-b");
        assert!(load_matching_agent_runtime_snapshot(
            &store,
            &stale_context,
            "run-b",
            &prompt_fingerprint,
            latest_revision,
        )
        .expect("stale snapshot lookup should succeed")
        .is_none());

        delete_persisted_agent_runtime_snapshot(&mut store, Some("session-a"))
            .expect("snapshot should delete");
        assert!(store
            .load_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, "session-a")
            .expect("read model lookup should succeed")
            .is_none());
    }

}
