use agent_core::{
    AgentRunIdentity, AgentRunLineage, Event, Metadata, TaskId, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use agent_storage::{SqliteStore, StorageError};

pub(super) fn semantic_memory_events_for_run(
    store: &SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
) -> Result<Vec<Event>, StorageError> {
    let Some(attempt_run_id) = run_context.get("agent_run_id") else {
        return Ok(Vec::new());
    };
    if let Ok(Some(identity)) = AgentRunIdentity::from_metadata(run_context) {
        let events = store.list_by_task_and_metadata(
            task_id,
            LOGICAL_AGENT_RUN_ID_METADATA_KEY,
            identity.logical_run_id(),
        )?;
        let events = events
            .into_iter()
            .filter(|event| event_matches_run_scope(event, run_context))
            .collect::<Vec<_>>();
        let source_is_indexed = identity
            .source_attempt_run_id()
            .is_none_or(|source_run_id| {
                events.iter().any(|event| {
                    event.metadata.get("agent_run_id").map(String::as_str) == Some(source_run_id)
                })
            });
        if source_is_indexed && AgentRunLineage::from_events(&events).is_ok() {
            return Ok(events);
        }
    }
    let Some(session_id) = run_context.get("session_id") else {
        return physical_semantic_memory_events_for_run(
            store,
            task_id,
            attempt_run_id,
            run_context,
        );
    };
    let session_events = store
        .list_by_task_and_metadata(task_id, "session_id", session_id)?
        .into_iter()
        .filter(|event| event_matches_run_scope(event, run_context))
        .collect::<Vec<_>>();
    let Ok(lineage) = AgentRunLineage::from_events(&session_events) else {
        return physical_semantic_memory_events_for_run(
            store,
            task_id,
            attempt_run_id,
            run_context,
        );
    };
    let Some(logical_run_id) = lineage.logical_run_id_for_attempt(attempt_run_id) else {
        return physical_semantic_memory_events_for_run(
            store,
            task_id,
            attempt_run_id,
            run_context,
        );
    };
    Ok(session_events
        .into_iter()
        .filter(|event| {
            lineage.logical_run_id_for_event(event).ok().flatten() == Some(logical_run_id)
        })
        .collect())
}

fn event_matches_run_scope(event: &Event, run_context: &Metadata) -> bool {
    ["project_id", "session_id"].into_iter().all(|key| {
        event.metadata.get(key).map(String::as_str) == run_context.get(key).map(String::as_str)
    })
}

fn physical_semantic_memory_events_for_run(
    store: &SqliteStore,
    task_id: &TaskId,
    attempt_run_id: &str,
    run_context: &Metadata,
) -> Result<Vec<Event>, StorageError> {
    Ok(store
        .list_by_task_and_metadata(task_id, "agent_run_id", attempt_run_id)?
        .into_iter()
        .filter(|event| event_matches_run_scope(event, run_context))
        .collect())
}
