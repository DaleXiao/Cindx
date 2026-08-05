use super::{
    agent_recovery_metadata_with_task_state, latest_agent_recovery_envelope,
    recovery_envelope_matches_active_turn,
};
use crate::{
    agent_commands::recovery_task_state_with_persisted_permission_denials,
    agent_read_model::{active_agent_events_for_session, agent_events_for_session},
    agent_runtime_snapshot::load_matching_agent_runtime_snapshot,
    app_state::AgentRecoveryEnvelope,
    event_persistence::append_event,
    permission_service::pending_agent_permissions_for_run,
    runtime_values::phase16_task_id,
};
use agent_application::{AgentRecoveryReason, AgentRecoveryState, AgentRunEvent};
use agent_core::{EventKind, Metadata};
use agent_storage::{SqliteStore, StorageError};

pub(crate) fn pause_permission_recovery_after_handoff_error(
    store: &mut SqliteStore,
    run_context: &Metadata,
) -> Result<bool, String> {
    store
        .with_immediate_transaction(|store| {
            pause_permission_recovery_after_handoff_error_in_transaction(store, run_context)
                .map_err(StorageError::new)
        })
        .map_err(|error| error.to_string())
}

fn pause_permission_recovery_after_handoff_error_in_transaction(
    store: &mut SqliteStore,
    run_context: &Metadata,
) -> Result<bool, String> {
    let session_id = run_context
        .get("session_id")
        .ok_or_else(|| "permission recovery handoff requires a session".to_string())?;
    let events = agent_events_for_session(store, &phase16_task_id(), Some(session_id))
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, Some(session_id));
    let Some(recovery) = latest_agent_recovery_envelope(&active_events) else {
        return Ok(false);
    };
    if !matches!(
        recovery.state,
        AgentRecoveryState::Blocked | AgentRecoveryState::Resuming
    ) || !recovery_envelope_matches_active_turn(&recovery, &active_events, run_context)
    {
        return Ok(false);
    }
    let recovery_sequence = active_events
        .iter()
        .rev()
        .find(|event| {
            event
                .metadata
                .get("recovery_envelope")
                .and_then(|encoded| serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok())
                .is_some_and(|candidate| candidate == recovery)
        })
        .map(|event| event.sequence)
        .unwrap_or_default();
    let settled_after_recovery = active_events
        .iter()
        .filter(|event| event.sequence > recovery_sequence)
        .filter_map(AgentRunEvent::from_event)
        .any(|event| event.status().is_terminal() || matches!(event, AgentRunEvent::Paused));
    if settled_after_recovery {
        return Ok(false);
    }
    if !pending_agent_permissions_for_run(
        store,
        &phase16_task_id(),
        Some(session_id),
        run_context.get("agent_run_id").map(String::as_str),
    )
    .map_err(|error| error.to_string())?
    .is_empty()
    {
        return Ok(false);
    }

    let latest_revision = active_events
        .last()
        .map(|event| event.sequence)
        .unwrap_or_default();
    let persisted_task_state = load_matching_agent_runtime_snapshot(
        store,
        run_context,
        &recovery.identity.source_run_id,
        &recovery.identity.prompt_fingerprint,
        latest_revision,
    )?;
    let task_state = recovery_task_state_with_persisted_permission_denials(
        &active_events,
        run_context,
        &recovery,
        persisted_task_state.as_ref(),
    )
    .or(persisted_task_state)
    .or_else(|| recovery.task_state.clone());
    let metadata = agent_recovery_metadata_with_task_state(
        &active_events,
        run_context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::Unknown("permission_handoff_failed".to_string()),
        [
            ("completion".to_string(), "partial".to_string()),
            (
                "stop_reason".to_string(),
                "permission_handoff_failed".to_string(),
            ),
            (
                "recovery_code".to_string(),
                "permission_handoff_failed".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        task_state.as_ref(),
        recovery.resource_snapshot.as_ref(),
    )?;
    append_event(
        store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        metadata,
    )
    .map_err(|error| error.to_string())?;
    Ok(true)
}
