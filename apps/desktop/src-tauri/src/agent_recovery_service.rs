use crate::desktop_prelude::*;
use crate::{
    agent_read_model::{
        active_agent_events_for_session, agent_events_for_session, is_agent_run_start_event,
    },
    agent_recovery_identity::{
        enrich_legacy_recovery_envelope, enrich_legacy_run_context, resolve_agent_recovery_identity,
    },
    agent_resource_snapshot::load_matching_agent_resource_snapshot,
    agent_runtime_snapshot::{
        delete_persisted_agent_runtime_snapshot, load_matching_agent_runtime_snapshot,
    },
    app_state::AgentRecoveryEnvelope,
    event_persistence::append_event,
    project_session_persistence::metadata_with_context,
    runtime_constants::AGENT_RECOVERY_SCHEMA,
    runtime_values::{current_time_millis, phase16_task_id},
};
#[path = "agent_recovery_status.rs"]
mod recovery_status;
pub(super) use crate::agent_recovery_identity::recovery_envelope_matches_active_turn;
use agent_application::insert_run_objectives;
use agent_core::{
    AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_IDENTITY_V1_SCHEMA,
    AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
pub(super) use recovery_status::{agent_task_is_cancelled, latest_applied_agent_steer_epoch};
use recovery_status::{latest_agent_run_event, recovery_run_context};

pub(super) fn latest_agent_recovery_envelope(events: &[Event]) -> Option<AgentRecoveryEnvelope> {
    events
        .iter()
        .rev()
        .find(|event| event.metadata.contains_key("recovery_envelope"))
        .and_then(|event| {
            let encoded = event.metadata.get("recovery_envelope")?;
            let envelope = serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok()?;
            (envelope.schema == AGENT_RECOVERY_SCHEMA
                && envelope.identity.validate().is_ok()
                && envelope
                    .task_state
                    .as_ref()
                    .is_none_or(|snapshot| snapshot.validate_checkpoint().is_ok()))
            .then_some(envelope)
        })
}

pub(super) fn initial_agent_objective_from_events(events: &[Event]) -> Option<String> {
    events.iter().find_map(|event| {
        is_agent_run_start_event(event).then(|| {
            event
                .metadata
                .get("initial_prompt_objective")
                .or_else(|| event.metadata.get("prompt"))
                .cloned()
        })?
    })
}

pub(super) fn build_agent_recovery_envelope_with_task_state(
    events: &[Event],
    run_context: &Metadata,
    state: AgentRecoveryState,
    reason: AgentRecoveryReason,
    now_ms: u64,
    task_state: Option<&AgentTaskStateSnapshot>,
    resource_snapshot: Option<&RunResourceSnapshot>,
) -> Option<AgentRecoveryEnvelope> {
    let identity = resolve_agent_recovery_identity(events, run_context)?.identity;
    let prior = latest_agent_recovery_envelope(events)
        .filter(|envelope| envelope.identity.matches(&identity));
    let latest_counter = |keys: &[&str]| {
        events.iter().rev().find_map(|event| {
            keys.iter().find_map(|key| {
                event
                    .metadata
                    .get(*key)
                    .and_then(|value| value.parse::<usize>().ok())
            })
        })
    };
    Some(AgentRecoveryEnvelope {
        schema: AGENT_RECOVERY_SCHEMA.to_string(),
        identity,
        effort: run_context
            .get("agent_effort")
            .cloned()
            .unwrap_or_else(|| "auto".to_string()),
        policy: run_context
            .get("collaboration_policy")
            .or_else(|| run_context.get("requested_policy"))
            .cloned()
            .unwrap_or_else(|| "auto_router".to_string()),
        queue_id: run_context.get("queue_id").cloned(),
        state,
        reason,
        attempts: prior
            .as_ref()
            .map(|envelope| envelope.attempts)
            .unwrap_or_default(),
        model_calls: events
            .iter()
            .filter(|event| event.kind == EventKind::ModelRequestFinished)
            .count(),
        tool_calls: events
            .iter()
            .filter(|event| event.kind == EventKind::ToolCallFinished)
            .count(),
        material_checkpoints: latest_counter(&["material_checkpoints", "run_checkpoints"])
            .or_else(|| prior.as_ref().map(|envelope| envelope.material_checkpoints))
            .unwrap_or_default(),
        observations: latest_counter(&["observations", "run_observations"])
            .or_else(|| prior.as_ref().map(|envelope| envelope.observations))
            .unwrap_or_default(),
        budget_extensions: latest_counter(&["budget_extensions", "run_budget_extensions"])
            .or_else(|| prior.as_ref().map(|envelope| envelope.budget_extensions))
            .unwrap_or_default(),
        task_state: task_state.cloned().or_else(|| {
            prior
                .as_ref()
                .and_then(|envelope| envelope.task_state.clone())
        }),
        resource_snapshot: resource_snapshot.cloned().or_else(|| {
            prior
                .as_ref()
                .and_then(|envelope| envelope.resource_snapshot.clone())
        }),
        created_at_ms: prior
            .as_ref()
            .map(|envelope| envelope.created_at_ms)
            .unwrap_or(now_ms),
        updated_at_ms: now_ms,
    })
}

pub(super) fn agent_recovery_metadata_with_task_state(
    events: &[Event],
    run_context: &Metadata,
    state: AgentRecoveryState,
    reason: AgentRecoveryReason,
    mut metadata: Metadata,
    task_state: Option<&AgentTaskStateSnapshot>,
    resource_snapshot: Option<&RunResourceSnapshot>,
) -> Result<Metadata, String> {
    let envelope = build_agent_recovery_envelope_with_task_state(
        events,
        run_context,
        state.clone(),
        reason,
        current_time_millis(),
        task_state,
        resource_snapshot,
    )
    .ok_or_else(|| "agent recovery checkpoint is missing a durable session prompt".to_string())?;
    metadata.insert(
        "recovery_schema".to_string(),
        AGENT_RECOVERY_SCHEMA.to_string(),
    );
    metadata.insert(
        "recovery_resume_key".to_string(),
        envelope.identity.resume_key.clone(),
    );
    metadata.insert(
        "recovery_state".to_string(),
        envelope.state.label().to_string(),
    );
    metadata.insert(
        "recovery_reason".to_string(),
        envelope.reason.label().to_string(),
    );
    metadata.insert(
        "recovery_attempts".to_string(),
        envelope.attempts.to_string(),
    );
    metadata.insert(
        "material_checkpoints".to_string(),
        envelope.material_checkpoints.to_string(),
    );
    metadata.insert(
        "observations".to_string(),
        envelope.observations.to_string(),
    );
    metadata.insert(
        "budget_extensions".to_string(),
        envelope.budget_extensions.to_string(),
    );
    metadata.insert(
        AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
        AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
    );
    metadata.insert(
        AGENT_RUN_ID_METADATA_KEY.to_string(),
        envelope.identity.source_run_id.clone(),
    );
    metadata.insert(
        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
        envelope.identity.logical_run_id().to_string(),
    );
    metadata.insert(
        "user_turn_sequence".to_string(),
        envelope.identity.user_turn_sequence.to_string(),
    );
    metadata.insert(
        "continuation_available".to_string(),
        (state == AgentRecoveryState::Paused).to_string(),
    );
    metadata.insert(
        "recovery_envelope".to_string(),
        serde_json::to_string(&envelope)
            .map_err(|error| format!("failed to encode agent recovery checkpoint: {error}"))?,
    );
    crate::agent_strategy_receipt_runtime::bind_strategy_receipt_from_events(
        events,
        run_context,
        &mut metadata,
    )?;
    insert_run_objectives(&mut metadata, run_context);
    Ok(metadata_with_context(metadata, run_context))
}

pub(super) fn peek_agent_recovery_envelope(
    store: &SqliteStore,
    run_context: &Metadata,
    allowed_states: &[AgentRecoveryState],
) -> Result<Option<AgentRecoveryEnvelope>, String> {
    let session_id = run_context
        .get("session_id")
        .ok_or_else(|| "agent recovery requires a session".to_string())?;
    let events = agent_events_for_session(store, &phase16_task_id(), Some(session_id))
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, Some(session_id));
    let Some(mut envelope) = latest_agent_recovery_envelope(&active_events) else {
        return Ok(None);
    };
    if envelope.state == AgentRecoveryState::Resuming {
        // A claim only blocks a new resume while the claimed resume is still in
        // flight. If the resumed run settled afterwards (terminal or paused),
        // the claim is stale and must not poison retry/resume for that session.
        let recovery_sequence = active_events
            .iter()
            .rev()
            .find(|event| {
                event
                    .metadata
                    .get("recovery_envelope")
                    .and_then(|encoded| serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok())
                    .is_some_and(|candidate| candidate == envelope)
            })
            .map(|event| event.sequence)
            .unwrap_or_default();
        let settled_after_claim = active_events
            .iter()
            .filter(|event| event.sequence > recovery_sequence)
            .filter_map(AgentRunEvent::from_event)
            .any(|event| event.status().is_terminal() || matches!(event, AgentRunEvent::Paused));
        if !settled_after_claim {
            return Err("agent recovery checkpoint is already claimed".to_string());
        }
        return Ok(None);
    }
    let recoverable_status = latest_agent_run_event(&active_events)
        .ok()
        .flatten()
        .is_some_and(|event| {
            matches!(
                event,
                AgentRunEvent::Paused | AgentRunEvent::WaitingForPermission
            )
        });
    if !recoverable_status {
        return Ok(None);
    }
    if !allowed_states.contains(&envelope.state) {
        return Err(format!(
            "agent recovery checkpoint is {}, not resumable",
            envelope.state.label()
        ));
    }
    if !recovery_envelope_matches_active_turn(&envelope, &active_events, run_context) {
        return Err("agent recovery checkpoint is stale for the latest user turn".to_string());
    }
    enrich_legacy_recovery_envelope(&events, &mut envelope);
    if envelope
        .resource_snapshot
        .as_ref()
        .is_some_and(|snapshot| !snapshot.is_within_persistence_bounds())
    {
        return Err("agent recovery resource checkpoint is invalid".to_string());
    }
    Ok(Some(envelope))
}

#[cfg(test)]
pub(super) fn claim_agent_recovery_envelope(
    store: &mut SqliteStore,
    run_context: &Metadata,
    allowed_states: &[AgentRecoveryState],
    reason: AgentRecoveryReason,
) -> Result<Option<AgentRecoveryEnvelope>, String> {
    store
        .with_immediate_transaction(|store| {
            claim_agent_recovery_envelope_in_transaction(store, run_context, allowed_states, reason)
                .map_err(StorageError::new)
        })
        .map_err(|error| error.to_string())
}

pub(super) fn claim_agent_recovery_envelope_in_transaction(
    store: &mut SqliteStore,
    run_context: &Metadata,
    allowed_states: &[AgentRecoveryState],
    reason: AgentRecoveryReason,
) -> Result<Option<AgentRecoveryEnvelope>, String> {
    let Some(mut envelope) = peek_agent_recovery_envelope(store, run_context, allowed_states)?
    else {
        return Ok(None);
    };
    envelope.state = AgentRecoveryState::Resuming;
    envelope.reason = reason;
    envelope.attempts = envelope.attempts.saturating_add(1);
    envelope.updated_at_ms = current_time_millis();
    append_event(
        store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Recovery resume claimed",
        metadata_with_context(
            [
                (
                    "recovery_schema".to_string(),
                    AGENT_RECOVERY_SCHEMA.to_string(),
                ),
                (
                    "recovery_resume_key".to_string(),
                    envelope.identity.resume_key.clone(),
                ),
                (
                    "recovery_state".to_string(),
                    envelope.state.label().to_string(),
                ),
                (
                    "recovery_reason".to_string(),
                    envelope.reason.label().to_string(),
                ),
                (
                    "recovery_attempts".to_string(),
                    envelope.attempts.to_string(),
                ),
                (
                    "agent_run_id".to_string(),
                    envelope.identity.source_run_id.clone(),
                ),
                (
                    AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
                    AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
                ),
                (
                    LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                    envelope.identity.logical_run_id().to_string(),
                ),
                (
                    "recovery_envelope".to_string(),
                    serde_json::to_string(&envelope).map_err(|error| {
                        format!("failed to encode claimed recovery checkpoint: {error}")
                    })?,
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(Some(envelope))
}

#[path = "agent_permission_recovery.rs"]
mod permission_recovery;
pub(super) use permission_recovery::pause_permission_recovery_after_handoff_error;

#[path = "agent_recovery_transcript.rs"]
mod recovery_transcript;
pub(super) use recovery_transcript::recovery_safe_transcript;

pub(super) fn reconcile_interrupted_agent_runs(store: &mut SqliteStore) -> Result<usize, String> {
    let task_id = phase16_task_id();
    let lifecycle_events = store
        .list_by_task_and_kinds(&task_id, &[EventKind::TaskStatusChanged, EventKind::Error])
        .map_err(|error| error.to_string())?;
    let mut active_runs = BTreeMap::<String, Metadata>::new();

    for event in &lifecycle_events {
        let session_key = event
            .metadata
            .get("session_id")
            .cloned()
            .unwrap_or_else(|| "__default__".to_string());
        if is_agent_run_start_event(event) {
            active_runs.insert(session_key, recovery_run_context(event));
            continue;
        }
        if event.summary == "Recovery resume claimed"
            && event.metadata.get("recovery_state").map(String::as_str) == Some("resuming")
        {
            active_runs.insert(session_key, recovery_run_context(event));
            continue;
        }
        let terminal = match AgentRunEvent::try_from_event(event) {
            Err(_) => true,
            Ok(Some(run_event)) => {
                run_event.status().is_terminal() || matches!(run_event, AgentRunEvent::Paused)
            }
            Ok(None) => false,
        };
        if terminal {
            active_runs.remove(&session_key);
        }
    }

    let mut recovered = 0;
    for (session_key, mut run_context) in active_runs {
        let session_id = (session_key != "__default__").then_some(session_key.as_str());
        let events = agent_events_for_session(store, &task_id, session_id)
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, session_id);
        enrich_legacy_run_context(&events, &mut run_context);
        run_context.insert(
            "steer_epoch".to_string(),
            latest_applied_agent_steer_epoch(&active_events).to_string(),
        );
        if let Some(initial_objective) = initial_agent_objective_from_events(&active_events) {
            run_context.insert("initial_prompt_objective".to_string(), initial_objective);
        }
        let already_recovered_wait = active_events.last().is_some_and(|event| {
            AgentRunEvent::from_event(event) == Some(AgentRunEvent::WaitingForPermission)
                && event.metadata.get("recovery_state").map(String::as_str) == Some("blocked")
        });
        if already_recovered_wait {
            if let Err(error) = delete_persisted_agent_runtime_snapshot(store, session_id) {
                eprintln!("stale agent runtime snapshot cleanup unavailable: {error}");
            }
            continue;
        }
        let pending_permissions = pending_agent_permissions_for_run(
            store,
            &task_id,
            session_id,
            run_context.get("agent_run_id").map(String::as_str),
        )
        .map_err(|error| error.to_string())?;
        let (summary, recovery_state, recovery_reason) = if pending_permissions.is_empty() {
            (
                "Agent task paused",
                AgentRecoveryState::Paused,
                AgentRecoveryReason::AppRestarted,
            )
        } else {
            (
                "Agent task waiting for permission",
                AgentRecoveryState::Blocked,
                AgentRecoveryReason::AppRestartedWaitingForPermission,
            )
        };
        let (task_state, resource_snapshot) = match resolve_agent_recovery_identity(
            &active_events,
            &run_context,
        ) {
            Some(resolved) => {
                let latest_revision = active_events
                    .last()
                    .map(|event| event.sequence)
                    .unwrap_or_default();
                let persisted_task_state = load_matching_agent_runtime_snapshot(
                    store,
                    &run_context,
                    &resolved.identity.source_run_id,
                    &resolved.identity.prompt_fingerprint,
                    latest_revision,
                )?;
                let task_state = latest_agent_recovery_envelope(&active_events)
                        .and_then(|recovery| {
                            crate::agent_commands::recovery_task_state_with_persisted_permission_denials(
                                &active_events,
                                &run_context,
                                &recovery,
                                persisted_task_state.as_ref(),
                            )
                        })
                        .or(persisted_task_state);
                (
                    task_state,
                    load_matching_agent_resource_snapshot(
                        store,
                        &run_context,
                        &resolved.identity.source_run_id,
                        latest_revision,
                    )?,
                )
            }
            None => (None, None),
        };
        let metadata = agent_recovery_metadata_with_task_state(
            &active_events,
            &run_context,
            recovery_state,
            recovery_reason,
            [
                ("completion".to_string(), "partial".to_string()),
                ("stop_reason".to_string(), "app_restarted".to_string()),
                ("recovery_code".to_string(), "run_interrupted".to_string()),
            ]
            .into_iter()
            .collect(),
            task_state.as_ref(),
            resource_snapshot.as_ref(),
        )?;
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            summary,
            metadata,
        )
        .map_err(|error| error.to_string())?;
        // The recovery event now owns the durable task-state checkpoint. Keeping
        // the run-scoped snapshot after that handoff only leaves stale state that
        // can never match a future run id.
        if let Err(error) = delete_persisted_agent_runtime_snapshot(store, session_id) {
            eprintln!("recovered agent runtime snapshot cleanup unavailable: {error}");
        }
        recovered += 1;
    }
    Ok(recovered)
}

#[cfg(test)]
#[path = "agent_recovery_service_tests.rs"]
mod tests;
