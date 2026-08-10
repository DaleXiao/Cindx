use crate::{
    agent_read_model::{
        active_agent_events_for_session, agent_events_for_session, is_agent_run_start_event,
    },
    queue_service::is_agent_queue_event,
    runtime_values::phase16_task_id,
    session_projection::load_agent_session_read_model,
};
use agent_application::{
    AgentRunEvent, AgentRunEventDecodeError, AgentRunStatus, AgentStrategyDecisionReceipt,
};
use agent_core::{Event, EventKind, Metadata, EVENT_TYPE_METADATA_KEY};
use agent_storage::{SqliteStore, StorageError};

pub(super) fn recovery_run_context(event: &Event) -> Metadata {
    let mut context = event.metadata.clone();
    for key in [
        EVENT_TYPE_METADATA_KEY,
        "agent_model_attribution_schema",
        "agent_actor",
        "agent_service",
        "agent_stage",
        "agent_model_profile",
        "agent_output_trust",
        "agent_effect_authority",
        "agent_attribution_component",
        "agent_attribution_model",
        "agent_attribution_legacy_role",
    ] {
        context.remove(key);
    }
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

pub(crate) fn latest_applied_agent_steer_epoch(events: &[Event]) -> u64 {
    let active_run_id = events
        .iter()
        .rev()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .map(String::as_str);
    events
        .iter()
        .filter_map(|event| {
            if is_agent_run_start_event(event)
                || (event.kind == EventKind::MessageAdded
                    && event.metadata.get("role").map(String::as_str) == Some("user")
                    && event.metadata.get("internal").map(String::as_str) != Some("true")
                    && (event.metadata.get("queue_mode").map(String::as_str) == Some("steer")
                        || event
                            .metadata
                            .get("continuation_replay")
                            .map(String::as_str)
                            != Some("true")))
            {
                return event
                    .metadata
                    .get("steer_epoch")
                    .and_then(|value| value.parse::<u64>().ok());
            }
            AgentStrategyDecisionReceipt::from_decision_event(event)
                .ok()
                .filter(|receipt| Some(receipt.agent_run_id()) == active_run_id)
                .map(|receipt| receipt.steer_epoch())
        })
        .max()
        .unwrap_or_default()
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
