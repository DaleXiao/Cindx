use crate::{
    agent_read_model::agent_events_for_session,
    app_state::{AppState, SessionOutputCacheEntry},
    runtime_values::{current_time_millis, phase16_task_id},
};
use agent_application::{
    project_agent_artifacts as agent_output_artifacts_from_events,
    AgentOutputArtifact as AgentOutputArtifactView,
};
use agent_core::Event;
use agent_storage::SqliteStore;
use std::collections::BTreeMap;

const SESSION_OUTPUT_CACHE_LIMIT: usize = 32;

pub(crate) fn cached_agent_output_artifacts(
    state: &tauri::State<'_, AppState>,
    store: &SqliteStore,
    session_id: &str,
) -> Result<Vec<AgentOutputArtifactView>, String> {
    let revision = store
        .event_revision_by_metadata(&phase16_task_id(), "session_id", session_id)
        .map_err(|error| error.to_string())?;
    let now_ms = current_time_millis();
    let cached = {
        let mut cache = state
            .session_output_cache
            .lock()
            .map_err(|error| format!("session output cache lock poisoned: {error}"))?;
        if let Some(entry) = cache.get_mut(session_id) {
            if entry.event_count == revision.event_count
                && entry.latest_sequence == revision.latest_sequence
            {
                entry.last_accessed_at_ms = now_ms;
                return Ok(entry.outputs.clone());
            }
        }
        cache.remove(session_id)
    };

    let outputs = if let Some(cached) = cached.filter(|entry| {
        entry.event_count <= revision.event_count
            && entry.latest_sequence <= revision.latest_sequence
    }) {
        let delta = store
            .list_by_task_and_metadata_after(
                &phase16_task_id(),
                "session_id",
                session_id,
                cached.latest_sequence,
            )
            .map_err(|error| error.to_string())?;
        if cached.event_count.saturating_add(delta.len() as u64) == revision.event_count {
            merge_agent_output_delta(cached.outputs, &delta)
        } else {
            rebuild_agent_output_artifacts(store, session_id)?
        }
    } else {
        rebuild_agent_output_artifacts(store, session_id)?
    };

    let mut cache = state
        .session_output_cache
        .lock()
        .map_err(|error| format!("session output cache lock poisoned: {error}"))?;
    if !cache.contains_key(session_id) && cache.len() >= SESSION_OUTPUT_CACHE_LIMIT {
        if let Some(oldest_session_id) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_accessed_at_ms)
            .map(|(session_id, _)| session_id.clone())
        {
            cache.remove(&oldest_session_id);
        }
    }
    cache.insert(
        session_id.to_string(),
        SessionOutputCacheEntry {
            event_count: revision.event_count,
            latest_sequence: revision.latest_sequence,
            outputs: outputs.clone(),
            last_accessed_at_ms: now_ms,
        },
    );
    Ok(outputs)
}

fn rebuild_agent_output_artifacts(
    store: &SqliteStore,
    session_id: &str,
) -> Result<Vec<AgentOutputArtifactView>, String> {
    let events = agent_events_for_session(store, &phase16_task_id(), Some(session_id))
        .map_err(|error| error.to_string())?;
    Ok(agent_output_artifacts_from_events(&events))
}

fn merge_agent_output_delta(
    mut outputs: Vec<AgentOutputArtifactView>,
    delta: &[Event],
) -> Vec<AgentOutputArtifactView> {
    let mut max_versions = BTreeMap::<String, usize>::new();
    for output in &outputs {
        let logical_path = output.source_path.as_deref().unwrap_or(&output.path);
        max_versions
            .entry(logical_path.to_string())
            .and_modify(|version| *version = (*version).max(output.version))
            .or_insert(output.version);
    }
    let mut delta_outputs = agent_output_artifacts_from_events(delta);
    for output in &mut delta_outputs {
        let logical_path = output.source_path.as_deref().unwrap_or(&output.path);
        output.version = output
            .version
            .saturating_add(max_versions.get(logical_path).copied().unwrap_or_default());
    }
    outputs.extend(delta_outputs);
    outputs.sort_by(|left, right| {
        right
            .timestamp_ms
            .cmp(&left.timestamp_ms)
            .then_with(|| right.id.cmp(&left.id))
    });
    outputs
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind, Metadata};

    #[test]
    fn output_delta_versions_continue_from_cached_projection() {
        let cached = vec![AgentOutputArtifactView {
            id: "old".to_string(),
            path: "snapshot-v1.png".to_string(),
            source_path: Some("image.png".to_string()),
            tool_name: "file.write".to_string(),
            status: "succeeded".to_string(),
            timestamp_ms: 1,
            run_id: None,
            version: 1,
            kind: "image".to_string(),
        }];
        let mut metadata = Metadata::new();
        metadata.insert("status".to_string(), "succeeded".to_string());
        metadata.insert("tool".to_string(), "file.write".to_string());
        metadata.insert("result_source_path".to_string(), "image.png".to_string());
        metadata.insert(
            "result_artifact_path".to_string(),
            "snapshot-v2.png".to_string(),
        );
        let delta = vec![Event {
            id: EventId("new".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 2,
            kind: EventKind::ToolCallFinished,
            summary: "done".to_string(),
            metadata,
        }];

        let outputs = merge_agent_output_delta(cached, &delta);

        assert_eq!(outputs[0].version, 2);
        assert_eq!(outputs[0].path, "snapshot-v2.png");
    }
}
