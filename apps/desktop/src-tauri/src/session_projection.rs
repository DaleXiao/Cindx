use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct AgentSessionReadModel {
    pub(crate) schema: String,
    pub(crate) revision: u64,
    pub(crate) event_count: u64,
    pub(crate) estimated_context_tokens: u64,
    #[serde(default)]
    pub(crate) has_effective_context_usage: bool,
    pub(crate) has_user_prompt: bool,
    pub(crate) active_run_id: Option<String>,
    #[serde(default)]
    pub(crate) latest_run_queue_id: Option<String>,
    #[serde(default)]
    pub(crate) queued_payloads: BTreeMap<String, QueuedAgentMessagePayload>,
    pub(crate) state: AgentState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionProjectionLoadStats {
    events_read: usize,
    rebuilt: bool,
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
        max_turns: 0,
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
        pending_plan_confirmation: None,
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
            has_effective_context_usage: false,
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
    let has_effective_context_usage = events
        .iter()
        .any(|event| effective_context_usage_from_event(event).is_some());
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
        has_effective_context_usage,
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

fn normalize_agent_session_budget_state(model: &mut AgentSessionReadModel) -> bool {
    let before = model.state.max_turns;
    model.state.max_turns = model.state.run_model_call_budget;
    before != model.state.max_turns
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
        let fallback_budget = agent_run_budget_from_start_event(event);
        model.state.status = "running".to_string();
        model.state.turn_count = 0;
        model.state.context_window_tokens = metadata_u64(event, "context_window_tokens")
            .unwrap_or(128_000)
            .max(1);
        model.state.run_started_at_ms = event.timestamp_ms;
        model.state.run_budget_ms = metadata_u64(event, "run_budget_ms").unwrap_or_else(|| {
            fallback_budget
                .max_duration
                .as_millis()
                .min(u64::MAX as u128) as u64
        });
        model.state.run_model_call_budget = metadata_usize(event, "run_model_call_budget")
            .unwrap_or(fallback_budget.max_model_calls);
        model.state.max_turns = model.state.run_model_call_budget;
        model.state.run_tool_call_budget =
            metadata_usize(event, "run_tool_call_budget").unwrap_or(fallback_budget.max_tool_calls);
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
            if !model.has_effective_context_usage {
                model.state.context_tokens_used = model.estimated_context_tokens;
            }
            match message.role {
                MessageRole::User => model.has_user_prompt = true,
                MessageRole::Assistant if !message.content.is_empty() => {
                    model.state.latest_answer = Some(message.content)
                }
                _ => {}
            }
        }
    }

    if agent_application::is_agent_model_turn_finished(event) {
        model.state.turn_count = model.state.turn_count.saturating_add(1);
    }

    if let Some((tokens, estimated)) = effective_context_usage_from_event(event) {
        model.state.context_tokens_used = tokens;
        model.state.context_usage_estimated = estimated;
        model.has_effective_context_usage = true;
        if let Some(context_window_tokens) = metadata_u64(event, "context_window_tokens") {
            model.state.context_window_tokens = context_window_tokens.max(1);
        }
    }

    if event.kind == EventKind::Error {
        model.state.last_error = event
            .metadata
            .get("error")
            .cloned()
            .or_else(|| Some(event.summary.clone()));
    }
    if AgentRunStatus::parse(&model.state.status) != AgentRunStatus::Failed {
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
    load_agent_session_read_model_with_stats(store, session_id).map(|(model, _)| model)
}

pub(crate) fn load_agent_session_read_model_snapshot(
    store: &SqliteStore,
    session_id: &str,
) -> Result<AgentSessionReadModel, StorageError> {
    load_current_agent_session_read_model(store, session_id).map(|(model, _, _)| model)
}

fn load_agent_session_read_model_with_stats(
    store: &mut SqliteStore,
    session_id: &str,
) -> Result<(AgentSessionReadModel, SessionProjectionLoadStats), StorageError> {
    let (model, stats, needs_persist) = load_current_agent_session_read_model(store, session_id)?;
    if needs_persist {
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
    Ok((model, stats))
}

fn load_current_agent_session_read_model(
    store: &SqliteStore,
    session_id: &str,
) -> Result<(AgentSessionReadModel, SessionProjectionLoadStats, bool), StorageError> {
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

    let (mut model, dirty, stats) = if let Some(mut model) = stored {
        let needs_budget_rebuild = model.state.run_started_at_ms > 0
            && (model.state.run_budget_ms == 0
                || model.state.run_model_call_budget == 0
                || model.state.run_tool_call_budget == 0);
        let normalized_budget_state = normalize_agent_session_budget_state(&mut model);
        model.has_effective_context_usage |= !model.state.context_usage_estimated;
        let delta = store.list_by_task_and_metadata_after(
            &task_id,
            "session_id",
            session_id,
            model.revision,
        )?;
        if needs_budget_rebuild
            || model.event_count.saturating_add(delta.len() as u64) != revision.event_count
        {
            let events = store.list_by_task_and_metadata(&task_id, "session_id", session_id)?;
            let events_read = events.len();
            (
                build_agent_session_read_model(store, session_id, events)?,
                true,
                SessionProjectionLoadStats {
                    events_read,
                    rebuilt: true,
                },
            )
        } else {
            let dirty = normalized_budget_state || !delta.is_empty();
            let events_read = delta.len();
            for event in &delta {
                apply_event_to_agent_session_read_model(&mut model, event);
            }
            (
                model,
                dirty,
                SessionProjectionLoadStats {
                    events_read,
                    rebuilt: false,
                },
            )
        }
    } else {
        let events = store.list_by_task_and_metadata(&task_id, "session_id", session_id)?;
        let events_read = events.len();
        (
            build_agent_session_read_model(store, session_id, events)?,
            true,
            SessionProjectionLoadStats {
                events_read,
                rebuilt: true,
            },
        )
    };
    let revision_changed =
        model.event_count != revision.event_count || model.revision != revision.latest_sequence;
    model.event_count = revision.event_count;
    model.revision = revision.latest_sequence;
    model.state.event_count = revision.event_count;
    model.state.latest_sequence = revision.latest_sequence;
    Ok((model, stats, dirty || revision_changed))
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
    state.latest_answer = state
        .latest_answer
        .as_deref()
        .map(sanitize_assistant_content)
        .filter(|answer| !answer.is_empty());
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{insert_event_type_v1, EventTypeV1, EVENT_TYPE_METADATA_KEY};

    fn session_context(session_id: &str) -> Metadata {
        [("session_id".to_string(), session_id.to_string())]
            .into_iter()
            .collect()
    }

    fn percentile(sorted_samples: &[u128], percentile: usize) -> u128 {
        let index = (sorted_samples.len().saturating_sub(1) * percentile) / 100;
        sorted_samples[index]
    }

    #[test]
    fn incremental_projection_reads_only_the_target_session_delta() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let target_context = session_context("session-target");
        let other_context = session_context("session-other");

        for index in 0..1_000 {
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                format!("Target projection event {index}"),
                target_context.clone(),
            )
            .expect("target event should append");
        }
        let initial_started_at = Instant::now();
        let (initial, initial_stats) =
            load_agent_session_read_model_with_stats(&mut store, "session-target")
                .expect("initial projection should build");
        let initial_micros = initial_started_at.elapsed().as_micros();
        assert!(initial_stats.rebuilt);
        assert_eq!(initial_stats.events_read, 1_000);
        assert_eq!(initial.event_count, 1_000);

        let mut unrelated_events = 4_000usize;
        for index in 0..unrelated_events {
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                format!("Unrelated projection event {index}"),
                other_context.clone(),
            )
            .expect("unrelated event should append");
        }
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Target projection delta",
            target_context,
        )
        .expect("target delta should append");

        let warm_started_at = Instant::now();
        let (updated, updated_stats) =
            load_agent_session_read_model_with_stats(&mut store, "session-target")
                .expect("incremental projection should load");
        let warm_micros = warm_started_at.elapsed().as_micros();
        assert!(!updated_stats.rebuilt);
        assert_eq!(updated_stats.events_read, 1);
        assert_eq!(updated.event_count, 1_001);
        let mut warm_samples = vec![warm_micros];
        for sample in 1..20 {
            for index in 0..50 {
                append_event(
                    &mut store,
                    &phase16_task_id(),
                    EventKind::TaskStatusChanged,
                    format!("Unrelated projection sample {sample} event {index}"),
                    other_context.clone(),
                )
                .expect("unrelated sample event should append");
            }
            unrelated_events += 50;
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                format!("Target projection delta {sample}"),
                session_context("session-target"),
            )
            .expect("target sample delta should append");
            let started_at = Instant::now();
            let (sampled, stats) =
                load_agent_session_read_model_with_stats(&mut store, "session-target")
                    .expect("incremental projection sample should load");
            warm_samples.push(started_at.elapsed().as_micros());
            assert!(!stats.rebuilt);
            assert_eq!(stats.events_read, 1);
            assert_eq!(sampled.event_count, 1_001 + sample);
        }
        warm_samples.sort_unstable();
        let warm_p50_micros = percentile(&warm_samples, 50);
        let warm_p95_micros = percentile(&warm_samples, 95);
        let warm_max_micros = warm_samples.last().copied().unwrap_or_default();
        println!(
            "{{\"schema\":\"cindx.session-projection-diagnostic.v1\",\"initial_events\":1000,\"unrelated_events\":{unrelated_events},\"delta_events_read\":{},\"initial_micros\":{},\"warm_micros\":{},\"warm_sample_count\":{},\"warm_p50_micros\":{warm_p50_micros},\"warm_p95_micros\":{warm_p95_micros},\"warm_max_micros\":{warm_max_micros}}}",
            updated_stats.events_read,
            initial_micros,
            warm_micros,
            warm_samples.len()
        );
    }

    #[test]
    fn read_only_snapshot_applies_unpersisted_session_delta() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Initial session event",
            session_context("session-snapshot"),
        )
        .expect("initial event should append");
        let persisted = load_agent_session_read_model(&mut store, "session-snapshot")
            .expect("initial projection should persist");
        assert_eq!(persisted.event_count, 1);

        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Unpersisted session delta",
            session_context("session-snapshot"),
        )
        .expect("delta should append");
        let snapshot = store
            .with_read_snapshot(|snapshot| {
                load_agent_session_read_model_snapshot(snapshot, "session-snapshot")
            })
            .expect("snapshot should apply the delta");
        assert_eq!(snapshot.event_count, 2);
        assert_eq!(snapshot.revision, 2);

        let stored = store
            .load_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, "session-snapshot")
            .expect("stored projection should load")
            .expect("stored projection should exist");
        assert_eq!(stored.revision, 1);
    }

    #[test]
    fn recorded_error_does_not_block_incremental_completion() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let session_id = "session-recorded-error";
        let context = session_context(session_id);
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            context.clone(),
        )
        .expect("start should append");
        load_agent_session_read_model(&mut store, session_id)
            .expect("initial projection should persist");

        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::Error,
            "Background extraction unavailable",
            metadata_with_context(
                [("error".to_string(), "recoverable warning".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        )
        .expect("recorded error should append");
        let (with_error, error_stats) =
            load_agent_session_read_model_with_stats(&mut store, session_id)
                .expect("error delta should project");
        assert!(!error_stats.rebuilt);
        assert_eq!(with_error.state.status, "running");
        assert_eq!(
            with_error.state.last_error.as_deref(),
            Some("recoverable warning")
        );

        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task completed",
            context,
        )
        .expect("completion should append");
        let (incremental, completion_stats) =
            load_agent_session_read_model_with_stats(&mut store, session_id)
                .expect("completion delta should project");
        assert!(!completion_stats.rebuilt);
        assert_eq!(incremental.state.status, "completed");
        assert_eq!(
            incremental.state.last_error.as_deref(),
            Some("recoverable warning")
        );

        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", session_id)
            .expect("session events should load");
        let rebuilt = build_agent_session_read_model(&store, session_id, events)
            .expect("full projection should rebuild");
        assert_eq!(incremental.state.status, rebuilt.state.status);
        assert_eq!(incremental.state.last_error, rebuilt.state.last_error);
    }

    #[test]
    fn typed_model_turn_count_matches_rebuild_and_invalid_tags_fail_closed() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let session_id = "session-typed-turn";
        let context = session_context(session_id);
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            context.clone(),
        )
        .expect("start should append");
        load_agent_session_read_model(&mut store, session_id)
            .expect("initial projection should persist");

        let mut typed_start = context.clone();
        typed_start.insert("context_projected_tokens".to_string(), "222".to_string());
        insert_event_type_v1(
            &EventKind::ModelRequestStarted,
            &mut typed_start,
            EventTypeV1::AgentModelTurnStarted,
        )
        .expect("model turn start tag should build");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestStarted,
            "模型回合开始",
            typed_start,
        )
        .expect("typed model turn start should append");
        let projected = load_agent_session_read_model(&mut store, session_id)
            .expect("typed turn start should project");
        assert_eq!(projected.state.turn_count, 0);
        assert_eq!(projected.state.context_tokens_used, 222);
        assert!(projected.state.context_usage_estimated);

        let mut typed_turn = context.clone();
        typed_turn.insert("prompt_tokens".to_string(), "321".to_string());
        insert_event_type_v1(
            &EventKind::ModelRequestFinished,
            &mut typed_turn,
            EventTypeV1::AgentModelTurnFinished,
        )
        .expect("model turn tag should build");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            "模型回合结束",
            typed_turn,
        )
        .expect("typed model turn should append");
        let typed = load_agent_session_read_model(&mut store, session_id)
            .expect("typed turn should project");
        assert_eq!(typed.state.turn_count, 1);
        assert_eq!(typed.state.context_tokens_used, 321);
        assert!(!typed.state.context_usage_estimated);

        let mut invalid_turn = context;
        invalid_turn.insert("prompt_tokens".to_string(), "999".to_string());
        invalid_turn.insert(
            EVENT_TYPE_METADATA_KEY.to_string(),
            "cindx.event.v2/agent.model_turn.finished".to_string(),
        );
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            invalid_turn,
        )
        .expect("future model event should append");
        let incremental = load_agent_session_read_model(&mut store, session_id)
            .expect("invalid turn should project safely");
        assert_eq!(incremental.state.turn_count, 1);
        assert_eq!(incremental.state.context_tokens_used, 321);
        assert!(!incremental.state.context_usage_estimated);

        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", session_id)
            .expect("session events should load");
        let rebuilt = build_agent_session_read_model(&store, session_id, events)
            .expect("full projection should rebuild");
        assert_eq!(incremental.state.turn_count, rebuilt.state.turn_count);
        assert_eq!(
            incremental.state.context_tokens_used,
            rebuilt.state.context_tokens_used
        );
        assert_eq!(
            incremental.state.context_usage_estimated,
            rebuilt.state.context_usage_estimated
        );
    }

    #[test]
    fn effective_context_usage_survives_later_message_events() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let session_id = "session-context-usage";
        let context = session_context(session_id);
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            metadata_with_context(
                [
                    ("prompt".to_string(), "continue".to_string()),
                    ("context_window_tokens".to_string(), "100000".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        )
        .expect("run start should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            "continue",
            context.clone(),
        )
        .expect("user message should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent context compacted",
            metadata_with_context(
                [
                    ("context_usage_reset".to_string(), "true".to_string()),
                    ("context_tokens_used".to_string(), "12000".to_string()),
                    ("context_window_tokens".to_string(), "100000".to_string()),
                    ("context_usage_estimated".to_string(), "true".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        )
        .expect("context reset should append");

        let compacted = load_agent_session_read_model(&mut store, session_id)
            .expect("compacted projection should load");
        assert_eq!(compacted.state.context_tokens_used, 12_000);
        assert_eq!(compacted.state.context_remaining_percent, 88.0);
        assert!(compacted.state.context_usage_estimated);

        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "preparing the next request",
            context.clone(),
        )
        .expect("assistant message should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestStarted,
            "Agent model turn started",
            metadata_with_context(
                [
                    ("context_projected_tokens".to_string(), "15000".to_string()),
                    ("context_window_tokens".to_string(), "100000".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        )
        .expect("model start should append");

        let projected = load_agent_session_read_model(&mut store, session_id)
            .expect("projected usage should load");
        assert_eq!(projected.state.context_tokens_used, 15_000);
        assert!(projected.state.context_usage_estimated);

        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            metadata_with_context(
                [
                    ("context_projected_tokens".to_string(), "15000".to_string()),
                    ("prompt_tokens".to_string(), "14500".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        )
        .expect("model finish should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "finished",
            context,
        )
        .expect("final assistant message should append");

        let exact =
            load_agent_session_read_model(&mut store, session_id).expect("exact usage should load");
        assert_eq!(exact.state.context_tokens_used, 14_500);
        assert_eq!(exact.state.context_remaining_percent, 85.5);
        assert!(!exact.state.context_usage_estimated);
    }
}
