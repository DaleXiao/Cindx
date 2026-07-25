use super::*;

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
    let user_turn_sequence = latest_external_user_turn_event(events)
        .map(|event| event.sequence)
        .unwrap_or_default();
    let prompt = latest_agent_prompt_from_active_events(events)?;
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

#[cfg(test)]
pub(super) fn build_agent_recovery_envelope(
    events: &[Event],
    run_context: &Metadata,
    state: &str,
    reason: &str,
    now_ms: u64,
) -> Option<AgentRecoveryEnvelope> {
    build_agent_recovery_envelope_with_task_state(events, run_context, state, reason, now_ms, None)
}

pub(super) fn build_agent_recovery_envelope_with_task_state(
    events: &[Event],
    run_context: &Metadata,
    state: &str,
    reason: &str,
    now_ms: u64,
    task_state: Option<&AgentTaskStateSnapshot>,
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
        created_at_ms: prior
            .as_ref()
            .map(|envelope| envelope.created_at_ms)
            .unwrap_or(now_ms),
        updated_at_ms: now_ms,
    })
}

pub(super) fn agent_recovery_metadata(
    events: &[Event],
    run_context: &Metadata,
    state: &str,
    reason: &str,
    metadata: Metadata,
) -> Result<Metadata, String> {
    agent_recovery_metadata_with_task_state(events, run_context, state, reason, metadata, None)
}

pub(super) fn agent_recovery_metadata_with_task_state(
    events: &[Event],
    run_context: &Metadata,
    state: &str,
    reason: &str,
    mut metadata: Metadata,
    task_state: Option<&AgentTaskStateSnapshot>,
) -> Result<Metadata, String> {
    let envelope = build_agent_recovery_envelope_with_task_state(
        events,
        run_context,
        state,
        reason,
        current_time_millis(),
        task_state,
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

pub(super) fn claim_agent_recovery_envelope(
    store: &mut SqliteStore,
    run_context: &Metadata,
    allowed_states: &[&str],
    reason: &str,
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

pub(super) fn recovery_safe_transcript(events: &[Event]) -> Vec<Message> {
    let resolved_tool_calls = events
        .iter()
        .filter(|event| {
            event.kind == EventKind::MessageAdded
                && event.metadata.get("role").map(String::as_str) == Some("tool")
        })
        .filter_map(|event| event.metadata.get("tool_call_id").cloned())
        .collect::<BTreeSet<_>>();
    let mut synthetic = BTreeSet::new();
    let mut messages = Vec::new();
    for message in events.iter().filter_map(message_from_event) {
        let unresolved = if message.role == MessageRole::Assistant {
            message
                .metadata
                .get("tool_call_ids")
                .map(|ids| {
                    ids.split(',')
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .filter(|id| !resolved_tool_calls.contains(*id))
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        messages.push(message);
        for tool_call_id in unresolved {
            if !synthetic.insert(tool_call_id.clone()) {
                continue;
            }
            messages.push(Message {
                role: MessageRole::Tool,
                content: "The prior tool call was interrupted before a durable result was recorded. Treat its outcome as unknown. Inspect current state before retrying, and request permission again for any write or destructive action.".to_string(),
                metadata: [
                    ("tool_call_id".to_string(), tool_call_id),
                    ("status".to_string(), "interrupted".to_string()),
                    ("kind".to_string(), "recovery_observation".to_string()),
                ]
                .into_iter()
                .collect(),
            });
        }
    }
    messages
}

pub(super) fn reconcile_interrupted_agent_runs(store: &mut SqliteStore) -> Result<usize, String> {
    let events = store
        .list_by_task(&phase16_task_id())
        .map_err(|error| error.to_string())?;
    let mut active_runs = BTreeMap::<String, Metadata>::new();

    for event in &events {
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
    for (session_key, run_context) in active_runs {
        let session_id = (session_key != "__default__").then_some(session_key.as_str());
        let active_events = active_agent_events_for_session(&events, session_id);
        let already_recovered_wait = active_events.last().is_some_and(|event| {
            event.summary == "Agent task waiting for permission"
                && event.metadata.get("recovery_state").map(String::as_str) == Some("blocked")
        });
        if already_recovered_wait {
            continue;
        }
        let pending_permissions = pending_agent_permissions_for_run(
            store,
            &phase16_task_id(),
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
        let metadata = agent_recovery_metadata(
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
        )?;
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            summary,
            metadata,
        )
        .map_err(|error| error.to_string())?;
        recovered += 1;
    }
    Ok(recovered)
}
