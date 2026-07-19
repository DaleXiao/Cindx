use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct AgentSessionReadModel {
    pub(crate) schema: String,
    pub(crate) revision: u64,
    pub(crate) event_count: u64,
    pub(crate) estimated_context_tokens: u64,
    pub(crate) has_user_prompt: bool,
    pub(crate) active_run_id: Option<String>,
    #[serde(default)]
    pub(crate) latest_run_queue_id: Option<String>,
    #[serde(default)]
    pub(crate) queued_payloads: BTreeMap<String, QueuedAgentMessagePayload>,
    pub(crate) state: AgentState,
}

pub(crate) fn empty_agent_state_for_session(session_id: &str) -> AgentState {
    AgentState {
        task_id: phase16_task_id().0,
        project_id: None,
        project_name: None,
        session_id: Some(session_id.to_string()),
        session_name: None,
        status: "idle".to_string(),
        turn_count: 0,
        max_turns: RunBudget::for_effort("auto").max_model_calls,
        transcript_messages: 0,
        context_tokens_used: 0,
        context_window_tokens: 128_000,
        context_remaining_percent: 100.0,
        context_usage_estimated: true,
        run_started_at_ms: 0,
        run_budget_ms: 0,
        run_model_call_budget: 0,
        run_tool_call_budget: 0,
        can_cancel: false,
        can_retry: false,
        can_continue: false,
        event_count: 0,
        latest_sequence: 0,
        oldest_sequence: 0,
        has_older_history: false,
        timeline: Vec::new(),
        messages: Vec::new(),
        pending_approvals: Vec::new(),
        queued_messages: Vec::new(),
        latest_answer: None,
        last_error: None,
    }
}

fn build_agent_session_read_model(
    store: &SqliteStore,
    session_id: &str,
    events: Vec<Event>,
) -> Result<AgentSessionReadModel, StorageError> {
    if events.is_empty() {
        return Ok(AgentSessionReadModel {
            schema: AGENT_SESSION_READ_MODEL_NAMESPACE.to_string(),
            revision: 0,
            event_count: 0,
            estimated_context_tokens: 0,
            has_user_prompt: false,
            active_run_id: None,
            latest_run_queue_id: None,
            queued_payloads: BTreeMap::new(),
            state: empty_agent_state_for_session(session_id),
        });
    }
    let active_events = active_agent_events_for_session(&events, Some(session_id));
    let estimated_context_tokens = estimate_context_tokens(
        &events
            .iter()
            .filter_map(message_from_event)
            .collect::<Vec<_>>(),
    );
    let has_user_prompt = latest_agent_prompt_from_active_events(&active_events).is_some();
    let active_run_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .cloned();
    let latest_run_queue_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("queue_id"))
        .cloned();
    let queued_payloads = pending_queued_agent_messages(&events, session_id)
        .into_iter()
        .map(|queued| (queued.view.id, queued.payload))
        .collect();
    let revision = events
        .last()
        .map(|event| event.sequence)
        .unwrap_or_default();
    let event_count = events.len() as u64;
    let mut state = agent_state_from_events(store, None, Some(session_id), events)?;
    state.timeline.clear();
    state.messages.clear();
    state.pending_approvals.clear();
    state.event_count = event_count;
    state.latest_sequence = revision;
    state.oldest_sequence = 0;
    state.has_older_history = false;
    Ok(AgentSessionReadModel {
        schema: AGENT_SESSION_READ_MODEL_NAMESPACE.to_string(),
        revision,
        event_count,
        estimated_context_tokens,
        has_user_prompt,
        active_run_id,
        latest_run_queue_id,
        queued_payloads,
        state,
    })
}

fn metadata_u64(event: &Event, key: &str) -> Option<u64> {
    event.metadata.get(key)?.parse::<u64>().ok()
}

fn metadata_usize(event: &Event, key: &str) -> Option<usize> {
    event.metadata.get(key)?.parse::<usize>().ok()
}

fn apply_event_to_agent_session_read_model(model: &mut AgentSessionReadModel, event: &Event) {
    model.revision = model.revision.max(event.sequence);
    model.event_count = model.event_count.saturating_add(1);
    model.state.event_count = model.event_count;
    model.state.latest_sequence = model.revision;
    for (key, target) in [
        ("project_id", &mut model.state.project_id),
        ("project_name", &mut model.state.project_name),
        ("session_id", &mut model.state.session_id),
        ("session_name", &mut model.state.session_name),
    ] {
        if let Some(value) = event.metadata.get(key) {
            *target = Some(value.clone());
        }
    }

    if is_agent_queue_event(event) {
        apply_queue_event(
            &mut model.state.queued_messages,
            &mut model.queued_payloads,
            event,
        );
    }

    if is_agent_run_start_event(event) {
        model.state.status = "running".to_string();
        model.state.turn_count = 0;
        model.state.context_window_tokens = metadata_u64(event, "context_window_tokens")
            .unwrap_or(128_000)
            .max(1);
        model.state.run_started_at_ms = event.timestamp_ms;
        model.state.run_budget_ms = metadata_u64(event, "run_budget_ms").unwrap_or_default();
        model.state.run_model_call_budget =
            metadata_usize(event, "run_model_call_budget").unwrap_or_default();
        model.state.run_tool_call_budget =
            metadata_usize(event, "run_tool_call_budget").unwrap_or_default();
        model.state.last_error = None;
        model.state.can_continue = false;
        model.active_run_id = event.metadata.get("agent_run_id").cloned();
        model.latest_run_queue_id = event.metadata.get("queue_id").cloned();
        model.has_user_prompt = event
            .metadata
            .get("prompt")
            .is_some_and(|prompt| !prompt.trim().is_empty());
    }

    if event.kind == EventKind::MessageAdded {
        if let Some(message) = message_from_event(event) {
            model.state.transcript_messages = model.state.transcript_messages.saturating_add(1);
            model.estimated_context_tokens = if model.estimated_context_tokens == 0 {
                512_u64.saturating_add(estimate_message_tokens(&message))
            } else {
                model
                    .estimated_context_tokens
                    .saturating_add(estimate_message_tokens(&message))
            };
            if model.state.context_usage_estimated {
                model.state.context_tokens_used = model.estimated_context_tokens;
            }
            match message.role {
                MessageRole::User => model.has_user_prompt = true,
                MessageRole::Assistant => model.state.latest_answer = Some(message.content),
                _ => {}
            }
        }
    }

    if event.kind == EventKind::ModelRequestFinished && event.summary == "Agent model turn finished"
    {
        model.state.turn_count = model.state.turn_count.saturating_add(1);
        if let Some(tokens) = metadata_u64(event, "prompt_tokens") {
            model.state.context_tokens_used = tokens;
            model.state.context_usage_estimated = false;
        }
    }

    if event.kind == EventKind::Error {
        model.state.last_error = event
            .metadata
            .get("error")
            .cloned()
            .or_else(|| Some(event.summary.clone()));
    }
    if event.kind == EventKind::Error || model.state.last_error.is_none() {
        if let Some(run_event) = AgentRunEvent::from_event(event) {
            let run_status = run_event.status();
            let partial_completion =
                event.metadata.get("completion").map(String::as_str) == Some("partial");
            model.state.status = run_status.label().to_string();
            model.state.can_continue = run_status.can_continue(partial_completion);
        }
    }

    model.state.context_remaining_percent = (model
        .state
        .context_window_tokens
        .saturating_sub(model.state.context_tokens_used)
        as f64
        / model.state.context_window_tokens.max(1) as f64
        * 100.0)
        .clamp(0.0, 100.0);
    let run_status = AgentRunStatus::parse(&model.state.status);
    model.state.can_cancel = run_status.can_cancel();
    model.state.can_retry = run_status.can_retry(model.has_user_prompt);
}

pub(crate) fn load_agent_session_read_model(
    store: &mut SqliteStore,
    session_id: &str,
) -> Result<AgentSessionReadModel, StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision_by_metadata(&task_id, "session_id", session_id)?;
    let stored = store
        .load_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, session_id)?
        .and_then(|stored| {
            serde_json::from_str::<AgentSessionReadModel>(&stored.payload)
                .ok()
                .filter(|model| {
                    model.schema == AGENT_SESSION_READ_MODEL_NAMESPACE
                        && model.revision == stored.revision
                        && model.revision <= revision.latest_sequence
                })
        });

    let (mut model, dirty) = if let Some(mut model) = stored {
        let delta = store.list_by_task_and_metadata_after(
            &task_id,
            "session_id",
            session_id,
            model.revision,
        )?;
        if model.event_count.saturating_add(delta.len() as u64) != revision.event_count {
            (
                build_agent_session_read_model(
                    store,
                    session_id,
                    store.list_by_task_and_metadata(&task_id, "session_id", session_id)?,
                )?,
                true,
            )
        } else {
            let dirty = !delta.is_empty();
            for event in &delta {
                apply_event_to_agent_session_read_model(&mut model, event);
            }
            (model, dirty)
        }
    } else {
        (
            build_agent_session_read_model(
                store,
                session_id,
                store.list_by_task_and_metadata(&task_id, "session_id", session_id)?,
            )?,
            true,
        )
    };
    let revision_changed =
        model.event_count != revision.event_count || model.revision != revision.latest_sequence;
    model.event_count = revision.event_count;
    model.revision = revision.latest_sequence;
    model.state.event_count = revision.event_count;
    model.state.latest_sequence = revision.latest_sequence;
    if dirty || revision_changed {
        let payload = serde_json::to_string(&model).map_err(|error| {
            StorageError::new(format!("session read model serialization failed: {error}"))
        })?;
        store.save_read_model(
            AGENT_SESSION_READ_MODEL_NAMESPACE,
            session_id,
            model.revision,
            &payload,
        )?;
    }
    Ok(model)
}

pub(crate) fn agent_session_audits(
    store: &SqliteStore,
    session_id: &str,
    active_run_id: Option<&str>,
    run_started_at_ms: u64,
) -> Result<Vec<PermissionAuditRecord>, StorageError> {
    store.list_permission_audits_for_session(
        &phase16_task_id(),
        session_id,
        active_run_id,
        run_started_at_ms,
    )
}

pub(crate) fn agent_state_from_read_model(
    store: &SqliteStore,
    model: &AgentSessionReadModel,
    session_id: &str,
    current_context: &Metadata,
    history_events: Vec<Event>,
) -> Result<AgentState, StorageError> {
    let audits = agent_session_audits(
        store,
        session_id,
        model.active_run_id.as_deref(),
        model.state.run_started_at_ms,
    )?;
    let mut state = model.state.clone();
    state.project_id = current_context
        .get("project_id")
        .cloned()
        .or(state.project_id);
    state.project_name = current_context
        .get("project_name")
        .cloned()
        .or(state.project_name);
    state.session_id = Some(session_id.to_string());
    state.session_name = current_context
        .get("session_name")
        .cloned()
        .or(state.session_name);
    state.timeline = history_events
        .iter()
        .filter(|event| !is_agent_queue_event(event))
        .cloned()
        .map(|event| timeline_entry(event, &audits))
        .collect();
    state.messages = history_events
        .iter()
        .filter_map(message_view_from_event)
        .collect();
    state.oldest_sequence = history_events
        .first()
        .map(|event| event.sequence)
        .unwrap_or(model.revision);
    state.has_older_history = state.oldest_sequence > 0
        && store.has_task_metadata_event_before(
            &phase16_task_id(),
            "session_id",
            session_id,
            state.oldest_sequence,
        )?;
    let mut pending_approvals = audits
        .into_iter()
        .filter(|audit| audit.resolution.is_none())
        .filter_map(tool_approval_from_audit)
        .collect::<Vec<_>>();
    pending_approvals.reverse();
    let mut run_status = AgentRunStatus::parse(&state.status);
    if !pending_approvals.is_empty() && run_status == AgentRunStatus::Running {
        run_status = AgentRunStatus::WaitingForPermission;
        state.status = run_status.label().to_string();
        state.can_cancel = run_status.can_cancel();
    }
    if run_status.is_terminal() {
        pending_approvals.clear();
    }
    state.pending_approvals = pending_approvals;
    Ok(state)
}
