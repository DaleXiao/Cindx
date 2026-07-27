use super::*;

#[cfg(test)]
pub(crate) fn agent_state(
    store: &SqliteStore,
    last_error: Option<String>,
) -> Result<AgentState, StorageError> {
    agent_state_for_session(store, last_error, None)
}

pub(crate) fn agent_state_for_session(
    store: &SqliteStore,
    last_error: Option<String>,
    session_id: Option<&str>,
) -> Result<AgentState, StorageError> {
    let task_id = phase16_task_id();
    let events = agent_events_for_session(store, &task_id, session_id)?;
    agent_state_from_events(store, last_error, session_id, events)
}

pub(crate) fn agent_state_from_events(
    store: &SqliteStore,
    last_error: Option<String>,
    session_id: Option<&str>,
    events: Vec<Event>,
) -> Result<AgentState, StorageError> {
    let task_id = phase16_task_id();
    let active_events = active_agent_events_for_session(&events, session_id);
    let last_error = last_error.or_else(|| {
        active_events.iter().rev().find_map(|event| {
            matches!(event.kind, EventKind::Error)
                .then(|| event.metadata.get("error").cloned())
                .flatten()
        })
    });
    let thread_events = session_id
        .map(|session_id| agent_session_events(&events, session_id))
        .unwrap_or_else(|| active_events.clone());
    let mut run_context = agent_run_context_from_events(if active_events.is_empty() {
        &thread_events
    } else {
        &active_events
    });
    if run_context.session_id.is_none() {
        run_context.session_id = session_id.map(str::to_string);
    }
    let (active_start_ts, active_end_ts) = agent_run_time_bounds(&events, &active_events);
    let active_run_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .map(String::as_str);
    let mut audits = match session_id {
        Some(session_id) if active_run_id.is_some() || active_start_ts.is_some() => store
            .list_permission_audits_for_session(
                &task_id,
                session_id,
                active_run_id,
                active_start_ts.unwrap_or_default(),
            )?,
        Some(_) => Vec::new(),
        None => store
            .list_permission_audits()?
            .into_iter()
            .filter(|audit| audit.request.task_id == task_id)
            .filter(|audit| {
                if let Some(active_run_id) = active_run_id {
                    return audit
                        .request
                        .metadata
                        .get("agent_run_id")
                        .map(String::as_str)
                        == Some(active_run_id);
                }
                active_start_ts.is_some_and(|timestamp| audit.requested_at_ms >= timestamp)
            })
            .collect(),
    };
    if active_run_id.is_none() {
        if let Some(active_end_ts) = active_end_ts {
            audits.retain(|audit| audit.requested_at_ms < active_end_ts);
        }
    }
    let timeline = thread_events
        .iter()
        .filter(|event| !is_agent_queue_event(event))
        .cloned()
        .map(|event| timeline_entry(event, &audits))
        .collect::<Vec<_>>();
    let messages = thread_events
        .iter()
        .filter_map(message_view_from_event)
        .collect::<Vec<_>>();
    let mut pending_approvals = audits
        .iter()
        .filter(|audit| audit.resolution.is_none())
        .cloned()
        .filter_map(tool_approval_from_audit)
        .collect::<Vec<_>>();
    pending_approvals.reverse();
    let queued_messages = session_id
        .map(|session_id| {
            pending_queued_agent_messages(&thread_events, session_id)
                .into_iter()
                .map(|message| message.view)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let latest_answer = thread_events
        .iter()
        .rev()
        .filter(|event| matches!(event.kind, EventKind::MessageAdded))
        .find(|event| {
            event
                .metadata
                .get("role")
                .map(|role| role == "assistant")
                .unwrap_or(false)
        })
        .and_then(|event| event.metadata.get("content").cloned());
    let run_status = AgentRunStatus::from_events(
        &active_events,
        !pending_approvals.is_empty(),
        last_error.is_some(),
    );
    let status = run_status.label().to_string();
    if run_status.is_terminal() {
        pending_approvals.clear();
    }
    let transcript_messages = agent_transcript_from_active_events(&thread_events).len();
    let context_window_tokens = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("context_window_tokens"))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(128_000)
        .max(1);
    let effective_context_usage = active_events
        .iter()
        .rev()
        .find_map(effective_context_usage_from_event);
    let (context_tokens_used, context_usage_estimated) =
        effective_context_usage.unwrap_or_else(|| {
            (
                estimate_context_tokens(
                    thread_events
                        .iter()
                        .filter_map(message_from_event)
                        .collect::<Vec<_>>()
                        .as_slice(),
                ),
                true,
            )
        });
    let context_remaining_percent = (context_window_tokens.saturating_sub(context_tokens_used)
        as f64
        / context_window_tokens as f64
        * 100.0)
        .clamp(0.0, 100.0);
    let turn_count = active_events
        .iter()
        .filter(|event| {
            matches!(event.kind, EventKind::ModelRequestFinished)
                && event.summary == "Agent model turn finished"
        })
        .count();
    let run_start = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event));
    let run_started_at_ms = run_start
        .map(|event| event.timestamp_ms)
        .unwrap_or_default();
    let run_budget_ms = run_start
        .and_then(|event| event.metadata.get("run_budget_ms"))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let run_model_call_budget = run_start
        .and_then(|event| event.metadata.get("run_model_call_budget"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let run_tool_call_budget = run_start
        .and_then(|event| event.metadata.get("run_tool_call_budget"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let partial_completion = active_events
        .iter()
        .rev()
        .find_map(|event| {
            (AgentRunEvent::from_event(event) == Some(AgentRunEvent::Completed))
                .then(|| event.metadata.get("completion").map(String::as_str) == Some("partial"))
        })
        .unwrap_or(false);
    let has_user_prompt = latest_agent_prompt_from_active_events(&active_events).is_some();
    let can_cancel = run_status.can_cancel();
    let can_continue = run_status.can_continue(partial_completion);
    let can_retry = run_status.can_retry(has_user_prompt);

    Ok(AgentState {
        task_id: task_id.0,
        project_id: run_context.project_id,
        project_name: run_context.project_name,
        session_id: run_context.session_id,
        session_name: run_context.session_name,
        status,
        turn_count,
        max_turns: if run_model_call_budget > 0 {
            run_model_call_budget
        } else {
            RunBudget::for_effort("auto").max_model_calls
        },
        transcript_messages,
        context_tokens_used,
        context_window_tokens,
        context_remaining_percent,
        context_usage_estimated,
        run_started_at_ms,
        run_budget_ms,
        run_model_call_budget,
        run_tool_call_budget,
        can_cancel,
        can_retry,
        can_continue,
        event_count: thread_events.len() as u64,
        latest_sequence: thread_events
            .last()
            .map(|event| event.sequence)
            .unwrap_or_default(),
        oldest_sequence: thread_events
            .first()
            .map(|event| event.sequence)
            .unwrap_or_default(),
        has_older_history: false,
        timeline,
        messages,
        pending_approvals,
        queued_messages,
        latest_answer,
        last_error,
    })
}

pub(crate) fn agent_state_with_error_in_context(
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    message: impl Into<String>,
) -> Result<AgentState, String> {
    let message = message.into();
    let session_id = run_context.get("session_id").map(String::as_str);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::Error,
        "Agent task failed",
        metadata_with_context(
            [("error".to_string(), message.clone())]
                .into_iter()
                .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    delete_persisted_agent_runtime_snapshot(&mut store, session_id)?;

    agent_state_for_session(&store, Some(message), session_id).map_err(|error| error.to_string())
}

pub(crate) fn latest_agent_prompt_from_active_events(active_events: &[Event]) -> Option<String> {
    latest_external_user_turn_event(active_events)
        .and_then(model_prompt_from_message_event)
        .or_else(|| {
            active_events
                .iter()
                .find(|event| {
                    matches!(event.kind, EventKind::TaskStatusChanged)
                        && is_agent_run_start_event(event)
                })
                .and_then(model_prompt_from_run_start)
        })
}

pub(crate) fn latest_agent_display_prompt_from_active_events(
    active_events: &[Event],
) -> Option<String> {
    latest_external_user_turn_event(active_events)
        .and_then(display_prompt_from_message_event)
        .or_else(|| {
            active_events
                .iter()
                .find(|event| {
                    matches!(event.kind, EventKind::TaskStatusChanged)
                        && is_agent_run_start_event(event)
                })
                .and_then(|event| event.metadata.get("prompt").cloned())
        })
}

pub(crate) fn agent_recovery_prompt_from_active_events(
    active_events: &[Event],
) -> Option<String> {
    primary_agent_user_turn_event(active_events)
        .and_then(model_prompt_from_message_event)
        .or_else(|| {
            active_events
                .iter()
                .find(|event| {
                    matches!(event.kind, EventKind::TaskStatusChanged)
                        && is_agent_run_start_event(event)
                })
                .and_then(|event| {
                    event
                        .metadata
                        .get("recovery_prompt")
                        .or_else(|| event.metadata.get("model_prompt"))
                        .or_else(|| event.metadata.get("prompt"))
                        .cloned()
                })
        })
}

pub(crate) fn primary_agent_user_turn_event(active_events: &[Event]) -> Option<&Event> {
    active_events.iter().find(|event| {
        event.kind == EventKind::MessageAdded
            && event.metadata.get("role").map(String::as_str) == Some("user")
            && event
                .metadata
                .get("continuation_replay")
                .map(String::as_str)
                != Some("true")
            && event.metadata.get("internal").map(String::as_str) != Some("true")
            && event.metadata.get("steer").map(String::as_str) != Some("true")
            && event.metadata.get("queue_mode").map(String::as_str) != Some("steer")
    })
}

fn model_prompt_from_message_event(event: &Event) -> Option<String> {
    event
        .metadata
        .get("model_content")
        .or_else(|| event.metadata.get("content"))
        .cloned()
}

fn display_prompt_from_message_event(event: &Event) -> Option<String> {
    event
        .metadata
        .get("display_content")
        .or_else(|| event.metadata.get("content"))
        .cloned()
}

fn model_prompt_from_run_start(event: &Event) -> Option<String> {
    event
        .metadata
        .get("model_prompt")
        .or_else(|| event.metadata.get("prompt"))
        .cloned()
}

pub(crate) fn agent_effort_from_active_events(active_events: &[Event]) -> AgentEffort {
    active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_effort"))
        .map(|effort| AgentEffort::parse(effort))
        .unwrap_or(AgentEffort::Auto)
}

pub(crate) fn agent_transcript_from_active_events(events: &[Event]) -> Vec<Message> {
    events.iter().filter_map(message_from_event).collect()
}

pub(crate) fn agent_runtime_transcript_from_active_events(events: &[Event]) -> Vec<Message> {
    events
        .iter()
        .filter_map(runtime_message_from_event)
        .collect()
}

pub(crate) fn agent_trace_state_for_session(
    store: &SqliteStore,
    export_path: Option<PathBuf>,
    last_error: Option<String>,
    session_id: Option<&str>,
) -> Result<AgentTraceState, StorageError> {
    let task_id = phase16_task_id();
    let events = agent_events_for_session(store, &task_id, session_id)?;
    agent_trace_state_from_events(store, export_path, last_error, session_id, events)
}

#[derive(Default)]
pub(crate) struct AgentTraceRoleAccumulator {
    pub(crate) models: BTreeSet<String>,
    pub(crate) calls: usize,
    pub(crate) completed: usize,
    pub(crate) degraded: usize,
    pub(crate) latency_ms: u64,
    pub(crate) first_token_latency_ms: u64,
    pub(crate) first_token_samples: u64,
    pub(crate) total_tokens: u64,
    pub(crate) evidence_count: usize,
}

pub(crate) fn agent_trace_role_summaries(events: &[Event]) -> Vec<AgentTraceRoleSummaryView> {
    let mut roles = BTreeMap::<String, AgentTraceRoleAccumulator>::new();
    for event in events.iter().filter(|event| {
        matches!(event.kind, EventKind::ModelRequestFinished)
            && event.metadata.contains_key("collaboration_id")
    }) {
        let Some(role) = event.metadata.get("role") else {
            continue;
        };
        let entry = roles.entry(role.clone()).or_default();
        entry.calls += 1;
        if event.metadata.get("status").map(String::as_str) == Some("degraded") {
            entry.degraded += 1;
        } else {
            entry.completed += 1;
        }
        if let Some(model) = event
            .metadata
            .get("model")
            .filter(|model| !model.trim().is_empty())
        {
            entry.models.insert(model.clone());
        }
        entry.latency_ms = entry.latency_ms.saturating_add(
            event
                .metadata
                .get("latency_ms")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default(),
        );
        if let Some(first_token_latency_ms) = event
            .metadata
            .get("first_token_latency_ms")
            .and_then(|value| value.parse::<u64>().ok())
        {
            entry.first_token_latency_ms = entry
                .first_token_latency_ms
                .saturating_add(first_token_latency_ms);
            entry.first_token_samples = entry.first_token_samples.saturating_add(1);
        }
        entry.total_tokens = entry.total_tokens.saturating_add(
            event
                .metadata
                .get("total_tokens")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default(),
        );
        entry.evidence_count = entry.evidence_count.saturating_add(
            event
                .metadata
                .get("evidence_count")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_default(),
        );
    }
    roles
        .into_iter()
        .map(|(role, summary)| AgentTraceRoleSummaryView {
            role,
            models: summary.models.into_iter().collect(),
            calls: summary.calls,
            completed: summary.completed,
            degraded: summary.degraded,
            latency_ms: summary.latency_ms,
            first_token_latency_ms: (summary.first_token_samples > 0)
                .then(|| summary.first_token_latency_ms / summary.first_token_samples),
            total_tokens: summary.total_tokens,
            evidence_count: summary.evidence_count,
        })
        .collect()
}

pub(crate) fn agent_trace_state_from_events(
    store: &SqliteStore,
    export_path: Option<PathBuf>,
    last_error: Option<String>,
    session_id: Option<&str>,
    events: Vec<Event>,
) -> Result<AgentTraceState, StorageError> {
    let task_id = phase16_task_id();
    let active_events = active_agent_events_for_session(&events, session_id);
    let run_context = agent_run_context_from_events(&active_events);
    let (active_start_ts, active_end_ts) = agent_run_time_bounds(&events, &active_events);
    let active_run_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .map(String::as_str);
    let audits = store
        .list_permission_audits()?
        .into_iter()
        .filter(|audit| audit.request.task_id == task_id)
        .filter(|audit| {
            if let Some(session_id) = session_id {
                if audit.request.metadata.get("session_id").map(String::as_str) != Some(session_id)
                {
                    return false;
                }
            }
            if let Some(active_run_id) = active_run_id {
                return audit
                    .request
                    .metadata
                    .get("agent_run_id")
                    .map(String::as_str)
                    == Some(active_run_id);
            }
            active_start_ts.is_some_and(|timestamp| {
                audit.requested_at_ms >= timestamp
                    && active_end_ts
                        .map(|end| audit.requested_at_ms < end)
                        .unwrap_or(true)
            })
        })
        .collect::<Vec<_>>();
    let has_pending_approval = audits.iter().any(|audit| audit.resolution.is_none());
    let status =
        AgentRunStatus::from_events(&active_events, has_pending_approval, last_error.is_some())
            .label()
            .to_string();
    let turns = agent_trace_turns_from_events(&active_events, &audits);
    let step_count = turns.iter().map(|turn| turn.steps.len()).sum::<usize>();
    let tool_call_count = turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .filter(|step| step.tool_call_id.is_some())
        .count();
    let permission_wait_count = turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .filter(|step| step.permission_id.is_some())
        .count();
    let error_count = turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .filter(|step| step.kind == "error")
        .count();
    let role_summaries = agent_trace_role_summaries(&active_events);
    let started_at_ms = active_events
        .first()
        .map(|event| event.timestamp_ms)
        .unwrap_or_default();
    let finished_at_ms = if matches!(
        status.as_str(),
        "paused" | "completed" | "failed" | "cancelled"
    ) {
        active_events.last().map(|event| event.timestamp_ms)
    } else {
        None
    };
    let duration_ms = finished_at_ms.map(|finished| finished.saturating_sub(started_at_ms));
    let run_id = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .map(|event| event.id.0.clone())
        .unwrap_or_else(|| "no-run".to_string());
    let start_sequence = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .map(|event| event.sequence)
        .unwrap_or_default();

    Ok(AgentTraceState {
        task_id: task_id.0.clone(),
        trace_id: format!("{}-{start_sequence}", task_id.0),
        run_id,
        project_id: run_context.project_id,
        project_name: run_context.project_name,
        session_id: run_context.session_id,
        session_name: run_context.session_name,
        status,
        started_at_ms,
        finished_at_ms,
        duration_ms,
        turn_count: turns.iter().filter(|turn| turn.index > 0).count(),
        step_count,
        tool_call_count,
        permission_wait_count,
        error_count,
        role_summaries,
        export_path: export_path.map(|path| path.display().to_string()),
        turns,
        last_error,
    })
}

pub(crate) fn agent_events_for_session(
    store: &SqliteStore,
    task_id: &TaskId,
    session_id: Option<&str>,
) -> Result<Vec<Event>, StorageError> {
    match session_id {
        Some(session_id) => {
            store.list_by_task_and_metadata_or_unscoped(task_id, "session_id", session_id)
        }
        None => store.list_by_task(task_id),
    }
}

pub(crate) fn agent_trace_turns_from_events(
    events: &[Event],
    audits: &[PermissionAuditRecord],
) -> Vec<AgentTraceTurnView> {
    let mut starts = TraceStarts::default();
    let mut turns: BTreeMap<usize, Vec<AgentTraceStepView>> = BTreeMap::new();
    let mut current_turn = 0usize;

    for event in events {
        if matches!(event.kind, EventKind::ModelRequestStarted) {
            current_turn = event
                .metadata
                .get("turn")
                .and_then(|value| value.parse::<usize>().ok())
                .map(|turn| turn + 1)
                .unwrap_or_else(|| current_turn.max(1));
        }

        let step = agent_trace_step_from_event(event, current_turn, audits, &starts);
        starts.record(event);
        turns.entry(step.turn_index).or_default().push(step);
    }

    turns
        .into_iter()
        .map(|(index, steps)| {
            let started_at_ms = steps
                .iter()
                .map(|step| step.started_at_ms)
                .min()
                .unwrap_or_default();
            let finished_at_ms = trace_turn_finished_at(&steps);
            let duration_ms = finished_at_ms.map(|finished| finished.saturating_sub(started_at_ms));
            let status = trace_turn_status(&steps);
            AgentTraceTurnView {
                index,
                label: if index == 0 {
                    "Run setup".to_string()
                } else {
                    format!("Turn {index}")
                },
                status,
                started_at_ms,
                finished_at_ms,
                duration_ms,
                steps,
            }
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct TraceStarts {
    pub(crate) model_requests: BTreeMap<String, u64>,
    pub(crate) tool_calls: BTreeMap<String, u64>,
    pub(crate) permissions: BTreeMap<String, u64>,
}

impl TraceStarts {
    fn record(&mut self, event: &Event) {
        match event.kind {
            EventKind::ModelRequestStarted => {
                if let Some(request_id) = event.metadata.get("request_id") {
                    self.model_requests
                        .insert(request_id.clone(), event.timestamp_ms);
                }
            }
            EventKind::ToolCallStarted => {
                if let Some(tool_call_id) = event.metadata.get("tool_call_id") {
                    self.tool_calls
                        .insert(tool_call_id.clone(), event.timestamp_ms);
                }
            }
            EventKind::PermissionRequested => {
                if let Some(permission_id) = event.metadata.get("permission_id") {
                    self.permissions
                        .insert(permission_id.clone(), event.timestamp_ms);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn agent_trace_step_from_event(
    event: &Event,
    turn_index: usize,
    audits: &[PermissionAuditRecord],
    starts: &TraceStarts,
) -> AgentTraceStepView {
    let request_id = event.metadata.get("request_id").cloned();
    let tool_call_id = event.metadata.get("tool_call_id").cloned();
    let permission_id = event.metadata.get("permission_id").cloned();
    let started_at_ms =
        trace_step_started_at(event, &request_id, &tool_call_id, &permission_id, starts);
    let finished_at_ms = trace_step_finished_at(event);
    let latency_ms = event
        .metadata
        .get("latency_ms")
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| finished_at_ms.map(|finished| finished.saturating_sub(started_at_ms)));
    let timeline = timeline_entry(event.clone(), audits);

    AgentTraceStepView {
        id: event.id.0.clone(),
        parent_id: trace_parent_id(turn_index, &tool_call_id, &permission_id),
        turn_index,
        sequence: event.sequence,
        kind: trace_kind_label(&event.kind).to_string(),
        label: redact_sensitive_text(&event.summary),
        status: trace_step_status(event, audits),
        started_at_ms,
        finished_at_ms,
        latency_ms,
        model: event.metadata.get("model").cloned(),
        tool_name: event.metadata.get("tool").cloned(),
        request_id,
        tool_call_id,
        permission_id,
        input_preview: trace_input_preview(event),
        output_preview: trace_output_preview(event),
        artifact_path: trace_artifact_path(event),
        detail: timeline.detail,
        metadata: redact_metadata(&event.metadata),
    }
}

pub(crate) fn trace_step_started_at(
    event: &Event,
    request_id: &Option<String>,
    tool_call_id: &Option<String>,
    permission_id: &Option<String>,
    starts: &TraceStarts,
) -> u64 {
    match event.kind {
        EventKind::ModelRequestFinished => request_id
            .as_ref()
            .and_then(|id| starts.model_requests.get(id).copied())
            .unwrap_or(event.timestamp_ms),
        EventKind::ToolCallFinished => tool_call_id
            .as_ref()
            .and_then(|id| starts.tool_calls.get(id).copied())
            .unwrap_or(event.timestamp_ms),
        EventKind::PermissionResolved => permission_id
            .as_ref()
            .and_then(|id| starts.permissions.get(id).copied())
            .unwrap_or(event.timestamp_ms),
        _ => event.timestamp_ms,
    }
}

pub(crate) fn trace_step_finished_at(event: &Event) -> Option<u64> {
    match event.kind {
        EventKind::ModelRequestFinished
        | EventKind::ToolCallFinished
        | EventKind::PermissionResolved
        | EventKind::Error => Some(event.timestamp_ms),
        _ => None,
    }
}

pub(crate) fn trace_parent_id(
    turn_index: usize,
    tool_call_id: &Option<String>,
    permission_id: &Option<String>,
) -> Option<String> {
    permission_id
        .as_ref()
        .map(|id| format!("permission:{id}"))
        .or_else(|| tool_call_id.as_ref().map(|id| format!("tool:{id}")))
        .or_else(|| (turn_index > 0).then(|| format!("turn:{turn_index}")))
}

pub(crate) fn trace_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished => "model",
        EventKind::ToolCallProposed | EventKind::ToolCallStarted | EventKind::ToolCallFinished => {
            "tool"
        }
        EventKind::PermissionRequested | EventKind::PermissionResolved => "permission",
        EventKind::Error => "error",
        EventKind::MessageAdded => "message",
        EventKind::RetrievalPerformed => "retrieval",
        _ => "status",
    }
}

pub(crate) fn trace_step_status(event: &Event, audits: &[PermissionAuditRecord]) -> String {
    match event.kind {
        EventKind::Error => "failed".to_string(),
        EventKind::ModelRequestStarted | EventKind::ToolCallStarted => "running".to_string(),
        EventKind::PermissionRequested => {
            if event.metadata.get("permission_id").is_some_and(|id| {
                audits
                    .iter()
                    .any(|audit| audit.request.id.0 == *id && audit.resolution.is_none())
            }) {
                "pending".to_string()
            } else {
                "done".to_string()
            }
        }
        EventKind::PermissionResolved => event
            .metadata
            .get("decision")
            .cloned()
            .unwrap_or_else(|| "done".to_string()),
        EventKind::ToolCallFinished => event
            .metadata
            .get("status")
            .cloned()
            .unwrap_or_else(|| "done".to_string()),
        EventKind::TaskStatusChanged => match event.summary.as_str() {
            "Agent task waiting for permission" => "waiting".to_string(),
            "Agent task paused" => "paused".to_string(),
            "Agent task completed" => "completed".to_string(),
            "Agent task cancelled" => "cancelled".to_string(),
            _ => "done".to_string(),
        },
        _ => "done".to_string(),
    }
}

pub(crate) fn trace_input_preview(event: &Event) -> Option<String> {
    event
        .metadata
        .get("input_preview")
        .or_else(|| event.metadata.get("tool_input"))
        .or_else(|| event.metadata.get("prompt"))
        .or_else(|| {
            event
                .metadata
                .get("content")
                .filter(|_| event.metadata.get("role").map(String::as_str) == Some("user"))
        })
        .map(|value| truncate_for_timeline(&redact_sensitive_text(value)))
}

pub(crate) fn trace_output_preview(event: &Event) -> Option<String> {
    event
        .metadata
        .get("output")
        .or_else(|| event.metadata.get("error"))
        .or_else(|| {
            event
                .metadata
                .get("content")
                .filter(|_| event.metadata.get("role").map(String::as_str) != Some("user"))
        })
        .map(|value| truncate_for_timeline(&redact_sensitive_text(value)))
}

pub(crate) fn trace_artifact_path(event: &Event) -> Option<String> {
    event
        .metadata
        .get("result_artifact_path")
        .or_else(|| event.metadata.get("result_text_path"))
        .or_else(|| {
            (event.metadata.get("tool").map(String::as_str) == Some("file.write"))
                .then(|| event.metadata.get("result_path"))
                .flatten()
        })
        .or_else(|| event.metadata.get("context_checkpoint_path"))
        .or_else(|| event.metadata.get("lancedb_export_path"))
        .cloned()
}

pub(crate) fn trace_turn_finished_at(steps: &[AgentTraceStepView]) -> Option<u64> {
    if steps
        .iter()
        .any(|step| matches!(step.status.as_str(), "running" | "pending" | "waiting"))
    {
        None
    } else {
        steps
            .iter()
            .filter_map(|step| step.finished_at_ms.or(Some(step.started_at_ms)))
            .max()
    }
}

pub(crate) fn trace_turn_status(steps: &[AgentTraceStepView]) -> String {
    if steps.iter().any(|step| step.status == "failed") {
        "failed".to_string()
    } else if steps
        .iter()
        .any(|step| step.status == "pending" || step.status == "waiting")
    {
        "waiting".to_string()
    } else if steps.iter().any(|step| step.status == "running") {
        "running".to_string()
    } else if steps.iter().any(|step| step.status == "cancelled") {
        "cancelled".to_string()
    } else if steps.iter().any(|step| step.status == "paused") {
        "paused".to_string()
    } else if steps.iter().any(|step| step.status == "completed") {
        "completed".to_string()
    } else {
        "done".to_string()
    }
}

#[cfg(test)]
pub(crate) fn active_agent_events(events: &[Event]) -> Vec<Event> {
    active_agent_events_for_session(events, None)
}

pub(crate) fn active_agent_events_for_session(
    events: &[Event],
    session_id: Option<&str>,
) -> Vec<Event> {
    let start_event = events.iter().rev().find(|event| {
        is_agent_run_start_event(event)
            && session_id
                .map(|session_id| {
                    event.metadata.get("session_id").map(String::as_str) == Some(session_id)
                })
                .unwrap_or(true)
    });
    if session_id.is_some() && start_event.is_none() {
        return Vec::new();
    }
    let start_sequence = start_event.map(|event| event.sequence);
    let run_session_id = start_event
        .and_then(|event| event.metadata.get("session_id"))
        .map(String::as_str)
        .or(session_id);
    let end_sequence = start_sequence.and_then(|start_sequence| {
        events
            .iter()
            .find(|event| {
                event.sequence > start_sequence
                    && is_agent_run_start_event(event)
                    && run_session_id
                        .map(|session_id| {
                            event.metadata.get("session_id").map(String::as_str) == Some(session_id)
                        })
                        .unwrap_or(true)
            })
            .map(|event| event.sequence)
    });
    events
        .iter()
        .filter(|event| {
            start_sequence
                .map(|sequence| {
                    event.sequence >= sequence
                        && end_sequence
                            .map(|end_sequence| event.sequence < end_sequence)
                            .unwrap_or(true)
                        && session_id
                            .map(|session_id| {
                                event
                                    .metadata
                                    .get("session_id")
                                    .map(|value| value == session_id)
                                    .unwrap_or(true)
                            })
                            .unwrap_or(true)
                })
                .unwrap_or(true)
        })
        .cloned()
        .map(redact_event)
        .collect()
}

pub(crate) fn agent_session_events(events: &[Event], session_id: &str) -> Vec<Event> {
    let mut current_session_id: Option<&str> = None;
    events
        .iter()
        .filter_map(|event| {
            if is_agent_run_start_event(event) {
                current_session_id = event.metadata.get("session_id").map(String::as_str);
            }
            let event_session_id = event.metadata.get("session_id").map(String::as_str);
            (event_session_id
                .map(|event_session_id| event_session_id == session_id)
                .unwrap_or(current_session_id == Some(session_id)))
            .then(|| redact_event(event.clone()))
        })
        .collect()
}

pub(crate) fn agent_run_time_bounds(
    events: &[Event],
    active_events: &[Event],
) -> (Option<u64>, Option<u64>) {
    let start_event = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .cloned();
    let start_sequence = start_event.as_ref().map(|event| event.sequence);
    let session_id = start_event
        .as_ref()
        .and_then(|event| event.metadata.get("session_id"))
        .map(String::as_str);
    let start_timestamp = start_sequence.and_then(|start_sequence| {
        active_events
            .iter()
            .find(|event| event.sequence == start_sequence)
            .map(|event| event.timestamp_ms)
    });
    let end_timestamp = start_sequence.and_then(|start_sequence| {
        events
            .iter()
            .find(|event| {
                event.sequence > start_sequence
                    && is_agent_run_start_event(event)
                    && session_id
                        .map(|session_id| {
                            event.metadata.get("session_id").map(String::as_str) == Some(session_id)
                        })
                        .unwrap_or(true)
            })
            .map(|event| event.timestamp_ms)
    });
    (start_timestamp, end_timestamp)
}

pub(crate) fn is_agent_run_start_event(event: &Event) -> bool {
    AgentRunEvent::from_event(event).is_some_and(AgentRunEvent::is_start)
}
