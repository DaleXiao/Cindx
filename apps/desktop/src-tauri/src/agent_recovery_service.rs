use crate::desktop_prelude::*;
use crate::{
    agent_read_model::{
        active_agent_events_for_session, agent_events_for_session,
        agent_recovery_prompt_from_active_events, is_agent_run_start_event,
        primary_agent_user_turn_event,
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

pub(super) fn agent_task_is_cancelled(
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
    Ok(active_agent_events_for_session(&events, session_id)
        .iter()
        .rev()
        .find(|event| matches!(event.kind, EventKind::TaskStatusChanged))
        .map(|event| event.summary == "Agent task cancelled")
        .unwrap_or(false))
}

pub(super) fn latest_agent_recovery_envelope(events: &[Event]) -> Option<AgentRecoveryEnvelope> {
    events.iter().rev().find_map(|event| {
        let encoded = event.metadata.get("recovery_envelope")?;
        let envelope = serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok()?;
        (envelope.schema == AGENT_RECOVERY_SCHEMA).then_some(envelope)
    })
}

pub(super) fn latest_applied_agent_steer_epoch(events: &[Event]) -> u64 {
    events
        .iter()
        .filter(|event| {
            is_agent_run_start_event(event)
                || (event.kind == EventKind::MessageAdded
                    && event.metadata.get("role").map(String::as_str) == Some("user")
                    && event.metadata.get("internal").map(String::as_str) != Some("true")
                    && (event.metadata.get("queue_mode").map(String::as_str) == Some("steer")
                        || event
                            .metadata
                            .get("continuation_replay")
                            .map(String::as_str)
                            != Some("true")))
        })
        .filter_map(|event| event.metadata.get("steer_epoch"))
        .filter_map(|value| value.parse::<u64>().ok())
        .max()
        .unwrap_or_default()
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

pub(super) fn agent_recovery_identity(
    events: &[Event],
    run_context: &Metadata,
) -> Option<(String, String, u64, String, String)> {
    let session_id = run_context.get("session_id")?.clone();
    let source_run_id = events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .cloned()
        .or_else(|| run_context.get("agent_run_id").cloned())
        .unwrap_or_default();
    let user_turn_sequence = primary_agent_user_turn_event(events)
        .map(|event| event.sequence)
        .unwrap_or_default();
    let prompt = agent_recovery_prompt_from_active_events(events)?;
    let prompt_fingerprint = sha256_hex(prompt.as_bytes());
    let project_id = run_context.get("project_id").cloned().unwrap_or_default();
    let resume_key = format!(
        "agent-resume-{}",
        &sha256_hex(
            format!(
                "{project_id}\n{session_id}\n{source_run_id}\n{user_turn_sequence}\n{prompt_fingerprint}"
            )
            .as_bytes()
        )[..24]
    );
    Some((
        resume_key,
        source_run_id,
        user_turn_sequence,
        prompt_fingerprint,
        prompt,
    ))
}

pub(super) fn build_agent_recovery_envelope_with_task_state(
    events: &[Event],
    run_context: &Metadata,
    state: &str,
    reason: &str,
    now_ms: u64,
    task_state: Option<&AgentTaskStateSnapshot>,
    resource_snapshot: Option<&RunResourceSnapshot>,
) -> Option<AgentRecoveryEnvelope> {
    let (resume_key, source_run_id, user_turn_sequence, prompt_fingerprint, _) =
        agent_recovery_identity(events, run_context)?;
    let prior =
        latest_agent_recovery_envelope(events).filter(|envelope| envelope.resume_key == resume_key);
    let workflow_resume_key = events.iter().rev().find_map(|event| {
        event
            .metadata
            .get("workflow_resume_key")
            .filter(|value| !value.trim().is_empty())
            .cloned()
    });
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
        resume_key,
        project_id: run_context.get("project_id").cloned(),
        session_id: run_context.get("session_id")?.clone(),
        source_run_id,
        user_turn_sequence,
        prompt_fingerprint,
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
        workflow_resume_key,
        state: state.to_string(),
        reason: reason.to_string(),
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
    state: &str,
    reason: &str,
    mut metadata: Metadata,
    task_state: Option<&AgentTaskStateSnapshot>,
    resource_snapshot: Option<&RunResourceSnapshot>,
) -> Result<Metadata, String> {
    let envelope = build_agent_recovery_envelope_with_task_state(
        events,
        run_context,
        state,
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
        envelope.resume_key.clone(),
    );
    metadata.insert("recovery_state".to_string(), envelope.state.clone());
    metadata.insert("recovery_reason".to_string(), envelope.reason.clone());
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
        "source_agent_run_id".to_string(),
        envelope.source_run_id.clone(),
    );
    metadata.insert(
        "user_turn_sequence".to_string(),
        envelope.user_turn_sequence.to_string(),
    );
    metadata.insert(
        "continuation_available".to_string(),
        (state == "paused").to_string(),
    );
    metadata.insert(
        "recovery_envelope".to_string(),
        serde_json::to_string(&envelope)
            .map_err(|error| format!("failed to encode agent recovery checkpoint: {error}"))?,
    );
    Ok(metadata_with_context(metadata, run_context))
}

pub(super) fn recovery_envelope_matches_active_turn(
    envelope: &AgentRecoveryEnvelope,
    events: &[Event],
    run_context: &Metadata,
) -> bool {
    let Some((resume_key, source_run_id, user_turn_sequence, prompt_fingerprint, _)) =
        agent_recovery_identity(events, run_context)
    else {
        return false;
    };
    envelope.schema == AGENT_RECOVERY_SCHEMA
        && envelope.resume_key == resume_key
        && envelope.source_run_id == source_run_id
        && envelope.user_turn_sequence == user_turn_sequence
        && envelope.prompt_fingerprint == prompt_fingerprint
        && run_context.get("session_id") == Some(&envelope.session_id)
        && run_context.get("project_id") == envelope.project_id.as_ref()
}

pub(super) fn peek_agent_recovery_envelope(
    store: &SqliteStore,
    run_context: &Metadata,
    allowed_states: &[&str],
) -> Result<Option<AgentRecoveryEnvelope>, String> {
    let session_id = run_context
        .get("session_id")
        .ok_or_else(|| "agent recovery requires a session".to_string())?;
    let events = agent_events_for_session(store, &phase16_task_id(), Some(session_id))
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, Some(session_id));
    let Some(envelope) = latest_agent_recovery_envelope(&active_events) else {
        return Ok(None);
    };
    let latest_status = active_events
        .iter()
        .rev()
        .find(|event| event.kind == EventKind::TaskStatusChanged && !is_agent_queue_event(event));
    let recoverable_status = latest_status.is_some_and(|event| {
        matches!(
            event.summary.as_str(),
            "Agent task paused" | "Agent task waiting for permission"
        )
    });
    if !recoverable_status {
        if envelope.state == "resuming" {
            return Err("agent recovery checkpoint is already claimed".to_string());
        }
        return Ok(None);
    }
    if !allowed_states.contains(&envelope.state.as_str()) {
        return Err(format!(
            "agent recovery checkpoint is {}, not resumable",
            envelope.state
        ));
    }
    if !recovery_envelope_matches_active_turn(&envelope, &active_events, run_context) {
        return Err("agent recovery checkpoint is stale for the latest user turn".to_string());
    }
    if envelope
        .resource_snapshot
        .as_ref()
        .is_some_and(|snapshot| !snapshot.is_within_persistence_bounds())
    {
        return Err("agent recovery resource checkpoint is invalid".to_string());
    }
    Ok(Some(envelope))
}

pub(super) fn claim_agent_recovery_envelope(
    store: &mut SqliteStore,
    run_context: &Metadata,
    allowed_states: &[&str],
    reason: &str,
) -> Result<Option<AgentRecoveryEnvelope>, String> {
    let Some(mut envelope) = peek_agent_recovery_envelope(store, run_context, allowed_states)?
    else {
        return Ok(None);
    };
    envelope.state = "resuming".to_string();
    envelope.reason = reason.to_string();
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
                    envelope.resume_key.clone(),
                ),
                ("recovery_state".to_string(), envelope.state.clone()),
                ("recovery_reason".to_string(), envelope.reason.clone()),
                (
                    "recovery_attempts".to_string(),
                    envelope.attempts.to_string(),
                ),
                ("agent_run_id".to_string(), envelope.source_run_id.clone()),
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
            active_runs.insert(session_key, event.metadata.clone());
            continue;
        }
        if event.summary == "Recovery resume claimed"
            && event.metadata.get("recovery_state").map(String::as_str) == Some("resuming")
        {
            active_runs.insert(session_key, event.metadata.clone());
            continue;
        }
        let terminal = matches!(event.kind, EventKind::Error)
            || matches!(
                event.summary.as_str(),
                "Agent task completed"
                    | "Agent task cancelled"
                    | "Agent task failed"
                    | "Agent task paused"
            );
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
        run_context.insert(
            "steer_epoch".to_string(),
            latest_applied_agent_steer_epoch(&active_events).to_string(),
        );
        if let Some(initial_objective) = initial_agent_objective_from_events(&active_events) {
            run_context.insert("initial_prompt_objective".to_string(), initial_objective);
        }
        let already_recovered_wait = active_events.last().is_some_and(|event| {
            event.summary == "Agent task waiting for permission"
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
            ("Agent task paused", "paused", "app_restarted")
        } else {
            (
                "Agent task waiting for permission",
                "blocked",
                "app_restarted_waiting_for_permission",
            )
        };
        let (task_state, resource_snapshot) =
            match agent_recovery_identity(&active_events, &run_context) {
                Some((_, source_run_id, _, prompt_fingerprint, _)) => {
                    let latest_revision = active_events
                        .last()
                        .map(|event| event.sequence)
                        .unwrap_or_default();
                    (
                        load_matching_agent_runtime_snapshot(
                            store,
                            &run_context,
                            &source_run_id,
                            &prompt_fingerprint,
                            latest_revision,
                        )?,
                        load_matching_agent_resource_snapshot(
                            store,
                            &run_context,
                            &source_run_id,
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
