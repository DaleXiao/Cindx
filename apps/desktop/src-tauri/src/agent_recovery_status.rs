use crate::{
    agent_read_model::{active_agent_events_for_session, agent_events_for_session},
    queue_service::is_agent_queue_event,
    runtime_values::phase16_task_id,
    session_projection::load_agent_session_read_model,
};
use agent_application::{AgentRunEvent, AgentRunEventDecodeError, AgentRunStatus};
use agent_core::{Event, Metadata, EVENT_TYPE_METADATA_KEY};
use agent_storage::{SqliteStore, StorageError};

pub(super) fn recovery_run_context(event: &Event) -> Metadata {
    let mut context = event.metadata.clone();
    context.remove(EVENT_TYPE_METADATA_KEY);
    context
}

pub(super) fn latest_agent_run_event(
    events: &[Event],
) -> Result<Option<AgentRunEvent>, AgentRunEventDecodeError> {
    for event in events.iter().rev() {
        if is_agent_queue_event(event) {
            continue;
        }
        match AgentRunEvent::try_from_event(event)? {
            Some(run_event) => return Ok(Some(run_event)),
            None => continue,
        }
    }
    Ok(None)
}

pub(crate) fn agent_task_is_cancelled(
    store: &mut SqliteStore,
    session_id: Option<&str>,
) -> Result<bool, StorageError> {
    if let Some(session_id) = session_id {
        let status = load_agent_session_read_model(store, session_id)?
            .state
            .status;
        return Ok(AgentRunStatus::parse(&status) == AgentRunStatus::Cancelled);
    }
    let events = agent_events_for_session(store, &phase16_task_id(), session_id)?;
    let active_events = active_agent_events_for_session(&events, session_id);
    Ok(matches!(
        latest_agent_run_event(&active_events),
        Ok(Some(AgentRunEvent::Cancelled))
    ))
}
