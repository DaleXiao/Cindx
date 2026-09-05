use super::queue::*;
use super::run_identity::{
    assign_continuation_agent_run_identity, assign_initial_agent_run_identity,
    inherited_agent_run_identity, inherited_agent_run_identity_from_events,
};
use crate::agent_execution_constraint::AgentExecutionConstraint;
use crate::agent_preparation_runtime::effective_prompt_objective_for_messages;
use crate::agent_preparation_runtime::AgentMemoryEvaluationConstraint;
use crate::agent_run_engine::{
    prepare_agent_execution, AgentRunPreparationError, PreparedAgentExecution,
};
use crate::agent_terminal_commit_runtime::persist_agent_terminal_once;
use crate::desktop_event_sink::AgentRunHost;
use crate::suspended_run_runtime::{
    clear_suspended_agent_run, suspended_agent_run_control_snapshot, suspended_agent_run_policy,
    take_suspended_agent_run, SuspendedAgentRun,
};
use crate::*;
use agent_application::{
    insert_run_objectives, insert_run_start_prompts, insert_user_message_model_prompt,
    merge_persistable_run_context,
};
use agent_core::{
    AgentRunIdentity, AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_ID_METADATA_KEY,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY, SOURCE_AGENT_RUN_ID_METADATA_KEY,
};

pub(crate) fn persisted_agent_policy_from_active_events(
    active_events: &[Event],
) -> Result<AgentPolicy, String> {
    let persisted = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_effort"))
        .map(String::as_str);
    persisted_agent_policy(persisted)
}

fn commit_agent_run_start_with<T>(
    control: &Arc<AgentRunControl>,
    commit: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    match control.commit_start_checkpoint_with(commit)? {
        agent_runtime::RunStartCheckpoint::Committed(value) => Ok(value),
        agent_runtime::RunStartCheckpoint::Stopped(reason) => Err(format!(
            "agent run stopped before durable start ({})",
            reason.code()
        )),
        agent_runtime::RunStartCheckpoint::TerminalCommitted => {
            Err("agent run terminated before durable start".to_string())
        }
    }
}

#[tauri::command]
pub(crate) async fn run_agent_task(
    app: tauri::AppHandle,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        run_agent_task_blocking(&app, state.inner(), input)
    })
    .await
    .map_err(|error| format!("agent task failed to join: {error}"))?
}

pub(crate) fn run_agent_task_blocking(
    host: &dyn AgentRunHost,
    state: &AppState,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    let session_id = input.session_id.clone();
    let effort = AgentPolicy::parse_ingress(&input.effort);
    let lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
    cancel_background_prompt_evaluations(state)?;
    project_session_metadata_for_session(state, Some(&session_id))?;
    let run_control_lease = state
        .agent_run_controls
        .register(&session_id, Arc::new(AgentRunControl::new(effort.label())))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "agent run is already active for this session".to_string())?;
    let cancellation = run_control_lease.control();
    let mut start_gate = Some(lifecycle);
    let result = run_agent_task_blocking_inner_with_evaluation_constraints_and_start_gate(
        host,
        state,
        input,
        &cancellation,
        AgentExecutionConstraint::Native,
        AgentMemoryEvaluationConstraint::Native,
        false,
        &mut start_gate,
    );
    if start_gate.is_some() {
        drop(run_control_lease);
        drop(start_gate.take());
    }
    if let Ok(agent) = result.as_ref() {
        if agent.status == "completed" {
            if let Ok(Some(refinement)) =
                persist_completed_conversation_title(state, &session_id, &agent.messages)
            {
                host.spawn_session_title_refinement(refinement);
            }
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_agent_task_blocking_inner_with_evaluation_constraints_and_start_gate(
    host: &dyn AgentRunHost,
    state: &AppState,
    input: AgentTaskInput,
    cancellation: &Arc<AgentRunControl>,
    execution_constraint: AgentExecutionConstraint,
    memory_constraint: AgentMemoryEvaluationConstraint,
    matched_route_plan_anchor_present: bool,
    start_gate: &mut Option<std::sync::MutexGuard<'_, ()>>,
) -> Result<AgentState, String> {
    let effort = AgentPolicy::parse_ingress(&input.effort);
    let user_prompt = input.prompt.trim().to_string();
    let queue_id = input.queue_id.clone();
    let session_id = input.session_id;
    clear_suspended_agent_run(state, &session_id)?;
    let mut run_context = project_session_metadata_for_session(state, Some(&session_id))?;
    let mut config = clone_provider_config(state)?;
    config.agent_system_prompt = personalized_agent_instructions(
        &load_personalization_config(),
        &config.agent_system_prompt,
    );
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            state,
            &run_context,
            "Provider config is incomplete",
        );
    }

    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(state)?);
    let attachments = validate_agent_attachments(&root, input.attachments)?;
    if user_prompt.is_empty() && attachments.is_empty() {
        return agent_state_with_error_in_context(state, &run_context, "agent prompt is empty");
    }
    let display_prompt = if user_prompt.is_empty() {
        format!(
            "Review attached {}",
            attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        user_prompt
    };
    let prompt = prompt_with_attachments(&display_prompt, &attachments);
    run_context = project_session_metadata_for_session(state, Some(&session_id))?;
    assign_initial_agent_run_identity(&mut run_context)?;
    execution_constraint.write_to_context(&mut run_context);
    if matched_route_plan_anchor_present {
        return Err(
            "matched route plan anchors are retired with workflow collaboration".to_string(),
        );
    }
    if !memory_constraint.is_native() {
        memory_constraint.write_to_context(&mut run_context);
    }
    run_context.insert("steer_epoch".to_string(), "0".to_string());
    run_context.insert(
        "initial_prompt_objective".to_string(),
        truncate_for_collaboration(&display_prompt, 6_000),
    );
    run_context.insert(
        "effective_prompt_objective".to_string(),
        truncate_for_collaboration(&display_prompt, 6_000),
    );
    if let Some(queue_id) = queue_id {
        run_context.insert("queue_id".to_string(), queue_id);
    }
    run_context.insert(
        "current_time".to_string(),
        normalized_current_time_context(&input.current_time),
    );
    let requested_policy = effort.requested_policy();
    run_context.insert("agent_effort".to_string(), effort.label().to_string());
    // Plan mode is honored only for High/Xhigh runs that explicitly requested
    // it; a stale flag on any other tier is dropped here at admission.
    if crate::agent_plan_mode_runtime::plan_mode_gate_active(input.plan_mode, effort) {
        run_context.insert(
            crate::agent_plan_mode_runtime::PLAN_MODE_REQUESTED_KEY.to_string(),
            "true".to_string(),
        );
    }
    add_image_generation_run_context(&mut run_context, &config, &prompt);
    add_agent_run_budget_metadata(&mut run_context, cancellation);
    run_context.insert(
        "requested_policy".to_string(),
        requested_policy.label().to_string(),
    );
    let session_id = run_context.get("session_id").map(String::as_str);
    let task_id = phase16_task_id();
    let (history, artifact_manifest) = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = agent_events_for_session(&store, &task_id, session_id)
            .map_err(|error| error.to_string())?;
        let session_events = session_id
            .map(|session_id| agent_session_events(&events, session_id))
            .unwrap_or_default();
        let history = session_events
            .iter()
            .filter_map(model_message_from_event)
            .collect::<Vec<_>>();
        let artifact_manifest = artifact_manifest_message(&session_events);
        (history, artifact_manifest)
    };
    let mut start_metadata = run_context.clone();
    insert_run_start_prompts(&mut start_metadata, &display_prompt, &prompt, &prompt);
    start_metadata.insert(
        "context_window_tokens".to_string(),
        config.context_window_tokens.to_string(),
    );
    let mut message_metadata = merge_persistable_run_context(Metadata::new(), &run_context);
    insert_user_message_model_prompt(&mut message_metadata, &display_prompt, &prompt);
    add_attachment_metadata(&mut message_metadata, &attachments);
    commit_agent_run_start_with(cancellation, || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .with_immediate_transaction(|store| {
                delete_persisted_agent_runtime_snapshot(store, session_id)
                    .map_err(StorageError::new)?;
                append_event(
                    store,
                    &task_id,
                    EventKind::TaskStatusChanged,
                    "Agent task started",
                    start_metadata,
                )?;
                append_message_event_with_metadata(
                    store,
                    &task_id,
                    MessageRole::User,
                    &display_prompt,
                    message_metadata,
                )
            })
            .map_err(|error| error.to_string())
    })?;
    drop(start_gate.take());

    let runtime_config = cancellation.runtime_config();
    let mut runtime = if history.is_empty() {
        start_agent_loop(task_id.clone(), prompt.clone(), runtime_config.clone())
    } else {
        start_agent_loop_with_history(task_id.clone(), prompt.clone(), history, runtime_config)
    };
    if let Some(message) = runtime.messages.last_mut() {
        add_attachment_metadata(&mut message.metadata, &attachments);
        message
            .metadata
            .insert("model_content".to_string(), prompt.clone());
        message
            .metadata
            .insert("display_content".to_string(), display_prompt.clone());
    }
    // Plan-then-confirm gate (High/Xhigh with the Settings plan-first toggle
    // enabled): draft a read-only plan, then pause for the user's explicit
    // decision before any preparation or execution. Plan drafting is charged to
    // the Worker stage budget; a drafting failure proceeds without a plan.
    match crate::agent_plan_mode_runtime::run_plan_mode_gate(
        state,
        &config,
        &root,
        &task_id,
        &run_context,
        &prompt,
        effort,
        cancellation,
    )? {
        crate::agent_plan_mode_runtime::PlanModeGateOutcome::AwaitingConfirmation(state) => {
            return Ok(*state)
        }
        crate::agent_plan_mode_runtime::PlanModeGateOutcome::Stopped => {
            return finish_agent_run_for_control_stop(host, state, &run_context, cancellation);
        }
        crate::agent_plan_mode_runtime::PlanModeGateOutcome::Proceed => {}
    }
    let prepared = match prepare_agent_execution(
        host,
        state,
        &config,
        &task_id,
        &root,
        run_context,
        runtime,
        prompt,
        artifact_manifest,
        effort,
        cancellation,
    ) {
        Ok(prepared) => prepared,
        Err(AgentRunPreparationError::Finished(result)) => return *result,
        Err(AgentRunPreparationError::ControlStop(run_context)) => {
            return finish_agent_run_for_control_stop(host, state, &run_context, cancellation);
        }
        Err(AgentRunPreparationError::Runtime { error, run_context }) => {
            return agent_state_with_error_in_context(state, &run_context, error)
        }
    };
    continue_agent_loop(host, state, &config, &root, prepared, effort, cancellation)
}

/// Runs off the invoke thread: cancelling a run scans the session's events and
/// commits a terminal transaction, and a sync command body would block the UI for
/// all of it (P2-05). The shared `_blocking` path already existed for internal
/// callers, so this only changes where it is called from.
#[tauri::command]
pub(crate) async fn cancel_agent_task(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        cancel_agent_task_blocking(&app, state.inner(), &input.session_id)
    })
    .await
    .map_err(|error| format!("agent task cancellation failed to join: {error}"))?
}

/// Shared cancellation path. A run parked at the plan-confirmation gate is
/// nonterminal and holds no active run control, so the ordinary can-cancel
/// guard is extended to admit exactly that paused plan wait.
pub(crate) fn cancel_agent_task_blocking(
    host: &dyn AgentRunHost,
    state: &AppState,
    session_id: &str,
) -> Result<AgentState, String> {
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
    let telemetry_control = active_agent_run_control(state, Some(session_id))?;
    let active_run_cancelled = request_agent_run_cancel(state, session_id)?;
    clear_suspended_agent_run(state, session_id)?;
    let mut run_context = project_session_metadata_for_session(state, Some(session_id))?;
    let session_id = run_context.get("session_id").cloned();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let current = agent_state_for_session(&store, None, session_id.as_deref())
        .map_err(|error| error.to_string())?;
    if !current.can_cancel && !active_run_cancelled {
        let events = agent_events_for_session(&store, &phase16_task_id(), session_id.as_deref())
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, session_id.as_deref());
        if !crate::agent_plan_mode_runtime::plan_mode_pause_present(&active_events) {
            return Ok(current);
        }
    }
    let events = agent_events_for_session(&store, &phase16_task_id(), session_id.as_deref())
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id.as_deref());
    let start = active_events
        .iter()
        .find(|event| is_agent_run_start_event(event));
    if let Some(start) = start {
        if let Some(identity) = AgentRunIdentity::from_metadata(&start.metadata)
            .map_err(|error| format!("invalid active agent run identity: {error}"))?
        {
            identity
                .insert_into(&mut run_context)
                .map_err(|error| format!("invalid active agent run identity: {error}"))?;
        } else {
            for key in [
                AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY,
                LOGICAL_AGENT_RUN_ID_METADATA_KEY,
                AGENT_RUN_ID_METADATA_KEY,
                SOURCE_AGENT_RUN_ID_METADATA_KEY,
            ] {
                if let Some(value) = start.metadata.get(key) {
                    run_context.insert(key.to_string(), value.clone());
                }
            }
        }
    }
    let steer_epoch = latest_applied_agent_steer_epoch(&active_events);
    run_context.insert("steer_epoch".to_string(), steer_epoch.to_string());
    let mut receipt_context = Metadata::new();
    crate::agent_strategy_receipt_runtime::bind_strategy_receipt_from_events(
        &active_events,
        &run_context,
        &mut receipt_context,
    )?;
    run_context.extend(receipt_context);
    let mut telemetry_request: Option<(Metadata, Arc<AgentRunControl>)> = None;
    let cancelled = if run_context.contains_key(AGENT_RUN_ID_METADATA_KEY) {
        let persisted = persist_agent_terminal_once(
            &mut store,
            &phase16_task_id(),
            &run_context,
            steer_epoch,
            |store, identity| {
                append_event(
                    store,
                    &phase16_task_id(),
                    EventKind::TaskStatusChanged,
                    "Agent task cancelled",
                    metadata_with_context(
                        [("reason".to_string(), "user_cancelled".to_string())]
                            .into_iter()
                            .chain(identity.metadata())
                            .collect(),
                        &run_context,
                    ),
                )?;
                delete_persisted_agent_runtime_snapshot(store, session_id.as_deref())
                    .map_err(StorageError::new)?;
                agent_state_for_session(store, None, session_id.as_deref())
            },
        )
        .map_err(|error| error.to_string())?;
        if persisted.inserted {
            if let Some(control) = telemetry_control {
                let mut telemetry_context = run_context.clone();
                if !telemetry_context.contains_key("agent_effort") {
                    if let Some(effort) = persisted_agent_policy_from_active_events(&active_events)
                        .ok()
                        .map(|policy| policy.label().to_string())
                    {
                        telemetry_context.insert("agent_effort".to_string(), effort);
                    }
                }
                telemetry_request = Some((telemetry_context, control));
            }
        }
        persisted.state
    } else {
        store
            .with_immediate_transaction(|store| {
                append_event(
                    store,
                    &phase16_task_id(),
                    EventKind::TaskStatusChanged,
                    "Agent task cancelled",
                    metadata_with_context(
                        [("reason".to_string(), "user_cancelled".to_string())]
                            .into_iter()
                            .collect(),
                        &run_context,
                    ),
                )?;
                delete_persisted_agent_runtime_snapshot(store, session_id.as_deref())
                    .map_err(StorageError::new)?;
                agent_state_for_session(store, None, session_id.as_deref())
            })
            .map_err(|error| error.to_string())?
    };
    if let Err(error) = refresh_project_memory_after_run(&mut store, &run_context) {
        eprintln!("project memory checkpoint unavailable: {error}");
    }
    drop(store);

    if let Some((telemetry_context, control)) = telemetry_request {
        crate::run_telemetry_runtime::record_run_telemetry_terminal(
            crate::run_telemetry_runtime::RunTelemetryTerminalFacts {
                run_context: &telemetry_context,
                control: &control,
                terminal_path: agent_application::RunTelemetryTerminalPathV1::None,
                stop_reason: "user_cancelled",
            },
        );
    }

    emit_agent_stream_delta(
        host,
        "agent-cancelled",
        session_id.as_deref(),
        "",
        true,
        true,
        None,
    );

    Ok(cancelled)
}

#[tauri::command]
pub(crate) async fn retry_agent_task(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        retry_agent_task_blocking(&app, state.inner(), input)
    })
    .await
    .map_err(|error| format!("agent retry failed to join: {error}"))?
}

pub(crate) fn retry_agent_task_blocking(
    host: &dyn AgentRunHost,
    state: &AppState,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    let session_id = input.session_id.clone();
    let lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
    cancel_background_prompt_evaluations(state)?;
    let recovery_context = project_session_metadata_for_session(state, Some(&session_id))?;
    let (persisted_effort, applied_steer_epoch, durable_recovery) = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", &session_id)
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, Some(&session_id));
        let recovery =
            peek_agent_recovery_envelope(&store, &recovery_context, &[AgentRecoveryState::Paused])?;
        (
            persisted_agent_policy_from_active_events(&active_events)?,
            latest_applied_agent_steer_epoch(&active_events),
            recovery,
        )
    };
    let suspended_effort = suspended_agent_run_policy(state, &session_id)?;
    if suspended_effort.is_some_and(|effort| effort != persisted_effort) {
        return Err("suspended agent policy does not match the active run".to_string());
    }
    let effort = suspended_effort.unwrap_or(persisted_effort);
    let suspended_snapshot = suspended_agent_run_control_snapshot(state, &session_id)?;
    let control = if let Some(snapshot) = suspended_snapshot {
        AgentRunControl::from_snapshot_for_continuation(snapshot)
            .map_err(|reason| format!("agent run cannot continue after {}", reason.code()))?
    } else if let Some(recovery) = durable_recovery
        .as_ref()
        .filter(|recovery| recovery.resource_snapshot.is_some())
    {
        let resources = recovery
            .resource_snapshot
            .clone()
            .expect("filtered durable resource snapshot");
        if recovery.reason != AgentRecoveryReason::AppRestarted {
            AgentRunControl::new_for_continuation_at_steer_epoch_with_resource_snapshot(
                effort.label(),
                applied_steer_epoch,
                resources,
            )
        } else {
            AgentRunControl::new_at_steer_epoch_with_resource_snapshot(
                effort.label(),
                applied_steer_epoch,
                resources,
            )
        }
    } else {
        AgentRunControl::new_at_steer_epoch(effort.label(), applied_steer_epoch)
    };
    let run_control_lease = state
        .agent_run_controls
        .register(&session_id, Arc::new(control))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "agent run is already active for this session".to_string())?;
    let cancellation = run_control_lease.control();
    let suspended = take_suspended_agent_run(state, &session_id)?;
    let mut start_gate = Some(lifecycle);
    let result = if let Some(suspended) = suspended {
        resume_suspended_agent_run(host, state, suspended, &cancellation, &mut start_gate)
    } else {
        retry_agent_task_blocking_inner(host, state, input, &cancellation, &mut start_gate)
    };
    if start_gate.is_some() {
        drop(run_control_lease);
        drop(start_gate.take());
    }
    result
}

pub(crate) fn resume_suspended_agent_run(
    host: &dyn AgentRunHost,
    state: &AppState,
    suspended: SuspendedAgentRun,
    cancellation: &Arc<AgentRunControl>,
    start_gate: &mut Option<std::sync::MutexGuard<'_, ()>>,
) -> Result<AgentState, String> {
    let SuspendedAgentRun {
        mut runtime,
        prompt,
        mut run_context,
        workspace_root,
        collaboration,
        run_control: _,
        last_touched_at_ms: _,
    } = suspended;
    let effort = persisted_agent_policy(run_context.get("agent_effort").map(String::as_str))?;
    let config = clone_provider_config(state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let recovery = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        peek_agent_recovery_envelope(&store, &run_context, &[AgentRecoveryState::Paused])?
    };
    let mut inherited_identity = inherited_agent_run_identity(&run_context)?;
    if let Some(recovery) = recovery.as_ref() {
        inherited_identity = Some((
            recovery.identity.logical_run_id().to_string(),
            recovery.identity.source_run_id.clone(),
        ));
        run_context.insert(
            "recovery_resume_key".to_string(),
            recovery.identity.resume_key.clone(),
        );
        run_context.insert(
            "recovery_attempts".to_string(),
            recovery.attempts.saturating_add(1).to_string(),
        );
        run_context.insert("continuation".to_string(), "true".to_string());
        if let Some(queue_id) = recovery.queue_id.as_ref() {
            run_context.insert("queue_id".to_string(), queue_id.clone());
        }
    }
    if let Some((logical_run_id, source_attempt_run_id)) = inherited_identity {
        assign_continuation_agent_run_identity(
            &mut run_context,
            &logical_run_id,
            &source_attempt_run_id,
        )?;
    } else {
        assign_initial_agent_run_identity(&mut run_context)?;
    }
    run_context.insert(
        "current_time".to_string(),
        normalized_current_time_context(""),
    );
    add_agent_run_budget_metadata(&mut run_context, cancellation);
    cancellation.extend_runtime_budget(&mut runtime);
    cancellation.mark_progress_at(
        run_context_steer_epoch(&run_context),
        "continuation",
        "Resuming saved execution state",
    );
    let recovery_prompt = runtime.user_prompt.clone();
    let display_prompt = runtime
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::User)
        .find_map(|message| message.metadata.get("display_content"))
        .cloned()
        .unwrap_or_else(|| prompt.clone());
    let mut retry_metadata = [
        ("continuation".to_string(), "true".to_string()),
        (
            "context_window_tokens".to_string(),
            config.context_window_tokens.to_string(),
        ),
    ]
    .into_iter()
    .collect();
    insert_run_start_prompts(
        &mut retry_metadata,
        &display_prompt,
        &prompt,
        &recovery_prompt,
    );
    insert_run_objectives(&mut retry_metadata, &run_context);
    commit_agent_run_start_with(cancellation, || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .with_immediate_transaction(|store| {
                let claimed = claim_agent_recovery_envelope_in_transaction(
                    store,
                    &run_context,
                    &[AgentRecoveryState::Paused],
                    AgentRecoveryReason::UserContinued,
                )
                .map_err(StorageError::new)?;
                let claim_matches = match (recovery.as_ref(), claimed.as_ref()) {
                    (None, None) => true,
                    (Some(expected), Some(claimed)) => {
                        claimed.identity.resume_key == expected.identity.resume_key
                            && claimed.attempts == expected.attempts.saturating_add(1)
                    }
                    _ => false,
                };
                if !claim_matches {
                    return Err(StorageError::new(
                        "agent recovery claim changed before durable retry start",
                    ));
                }
                append_event(
                    store,
                    &phase16_task_id(),
                    EventKind::TaskStatusChanged,
                    "Agent task retry started",
                    metadata_with_context(retry_metadata, &run_context),
                )
            })
            .map_err(|error| error.to_string())
    })?;
    drop(start_gate.take());
    let prepared = PreparedAgentExecution {
        base_run_context: run_context.clone(),
        run_context,
        runtime,
        prompt,
        collaboration,
    };
    continue_agent_loop(
        host,
        state,
        &config,
        &workspace_root,
        prepared,
        effort,
        cancellation,
    )
}

pub(crate) fn retry_agent_task_blocking_inner(
    host: &dyn AgentRunHost,
    state: &AppState,
    input: SessionActionInput,
    cancellation: &Arc<AgentRunControl>,
    start_gate: &mut Option<std::sync::MutexGuard<'_, ()>>,
) -> Result<AgentState, String> {
    clear_suspended_agent_run(state, &input.session_id)?;
    let mut run_context = project_session_metadata_for_session(state, Some(&input.session_id))?;
    run_context.insert(
        "current_time".to_string(),
        normalized_current_time_context(""),
    );
    let config = clone_provider_config(state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(state)?);
    let session_id = run_context.get("session_id").cloned();
    let task_id = phase16_task_id();
    let (
        prompt,
        display_prompt,
        recovery_prompt,
        effort,
        recovery,
        applied_steer_epoch,
        initial_objective,
        effective_objective,
        prompt_contract_epoch,
        inherited_identity,
        history,
        artifact_manifest,
        plan_mode_requested,
    ) = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = agent_events_for_session(&store, &task_id, session_id.as_deref())
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, session_id.as_deref());
        let prompt = latest_agent_prompt_from_active_events(&active_events)
            .ok_or_else(|| "No previous agent prompt to retry".to_string())?;
        let display_prompt = latest_agent_display_prompt_from_active_events(&active_events)
            .unwrap_or_else(|| prompt.clone());
        let recovery_prompt = agent_recovery_prompt_from_active_events(&active_events)
            .unwrap_or_else(|| prompt.clone());
        let effort = persisted_agent_policy_from_active_events(&active_events)?;
        let active_inherited_identity = match active_events
            .iter()
            .find(|event| is_agent_run_start_event(event))
        {
            Some(event)
                if AgentRunIdentity::from_metadata(&event.metadata)
                    .map_err(|error| format!("invalid agent run identity: {error}"))?
                    .is_some() =>
            {
                inherited_agent_run_identity_from_events(&[], &event.metadata)?
            }
            Some(event) => {
                let lineage_events = session_id
                    .as_deref()
                    .map(|session_id| agent_session_events(&events, session_id))
                    .unwrap_or_else(|| events.clone());
                inherited_agent_run_identity_from_events(&lineage_events, &event.metadata)?
            }
            None => None,
        };
        let recovery =
            peek_agent_recovery_envelope(&store, &run_context, &[AgentRecoveryState::Paused])?;
        let applied_steer_epoch = latest_applied_agent_steer_epoch(&active_events);
        let initial_objective = initial_agent_objective_from_events(&active_events)
            .unwrap_or_else(|| display_prompt.clone());
        let effective_objective = effective_prompt_objective_for_messages(
            &initial_objective,
            &recovery_safe_transcript(&active_events),
        );
        let prompt_contract_epoch = recovery
            .as_ref()
            .and_then(|recovery| recovery.task_state.as_ref())
            .and_then(|snapshot| snapshot.prepared_task_state.as_ref())
            .map(|prepared| prepared.contract_epoch)
            .unwrap_or(applied_steer_epoch);
        let inherited_identity = recovery
            .as_ref()
            .map(|recovery| {
                (
                    recovery.identity.logical_run_id().to_string(),
                    recovery.identity.source_run_id.clone(),
                )
            })
            .or(active_inherited_identity);
        let session_events = session_id
            .as_deref()
            .map(|session_id| agent_session_events(&events, session_id))
            .unwrap_or_default();
        let history = recovery_safe_transcript(&session_events);
        let artifact_manifest = artifact_manifest_message(&session_events);
        let plan_mode_requested =
            crate::agent_plan_mode_runtime::plan_mode_requested_in_events(&active_events);
        (
            prompt,
            display_prompt,
            recovery_prompt,
            effort,
            recovery,
            applied_steer_epoch,
            initial_objective,
            effective_objective,
            prompt_contract_epoch,
            inherited_identity,
            history,
            artifact_manifest,
            plan_mode_requested,
        )
    };
    run_context.insert("steer_epoch".to_string(), applied_steer_epoch.to_string());
    run_context.insert("initial_prompt_objective".to_string(), initial_objective);
    run_context.insert(
        "effective_prompt_objective".to_string(),
        effective_objective,
    );
    run_context.insert(
        "prompt_contract_epoch".to_string(),
        prompt_contract_epoch.to_string(),
    );
    if let Some(recovery) = recovery.as_ref() {
        run_context.insert(
            "recovery_resume_key".to_string(),
            recovery.identity.resume_key.clone(),
        );
        run_context.insert(
            "recovery_attempts".to_string(),
            recovery.attempts.saturating_add(1).to_string(),
        );
        run_context.insert("continuation".to_string(), "true".to_string());
        if let Some(queue_id) = recovery.queue_id.as_ref() {
            run_context.insert("queue_id".to_string(), queue_id.clone());
        }
    }
    if let Some((logical_run_id, source_attempt_run_id)) = inherited_identity {
        assign_continuation_agent_run_identity(
            &mut run_context,
            &logical_run_id,
            &source_attempt_run_id,
        )?;
    } else {
        assign_initial_agent_run_identity(&mut run_context)?;
    }
    let requested_policy = effort.requested_policy();
    run_context.insert("agent_effort".to_string(), effort.label().to_string());
    // A resumed plan-mode run re-derives its request flag from the durable
    // start event so preparation can inject the confirmed plan.
    if plan_mode_requested {
        run_context.insert(
            crate::agent_plan_mode_runtime::PLAN_MODE_REQUESTED_KEY.to_string(),
            "true".to_string(),
        );
    }
    add_image_generation_run_context(&mut run_context, &config, &prompt);
    add_agent_run_budget_metadata(&mut run_context, cancellation);
    run_context.insert(
        "requested_policy".to_string(),
        requested_policy.label().to_string(),
    );
    let recovered_prepared_task_state =
        crate::prepared_task_state_metadata::prepared_task_state_from_legacy_metadata(
            &run_context,
            &recovery_prompt,
            agent_runtime::prompt_completion_intent(&run_context),
        );
    crate::prepared_task_state_metadata::project_prepared_task_state_to_legacy_metadata(
        &recovered_prepared_task_state,
        &mut run_context,
    );
    let (restored_task_state, checkpoint_fallback) = if let Some(snapshot) = recovery
        .as_ref()
        .and_then(|recovery| recovery.task_state.as_ref())
    {
        match snapshot.restore_with_effective_objective(
            recovery_prompt.clone(),
            history.clone(),
            recovered_prepared_task_state.effective_objective(),
        ) {
            Ok(runtime) => (Some(runtime), None),
            Err(error) => (
                None,
                Some(metadata_with_context(
                    [
                        (
                            "recovery_code".to_string(),
                            "task_state_lineage_mismatch".to_string(),
                        ),
                        ("recovery_detail".to_string(), error.to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    &run_context,
                )),
            ),
        }
    } else {
        (None, None)
    };
    let mut start_metadata = run_context.clone();
    insert_run_start_prompts(
        &mut start_metadata,
        &display_prompt,
        &prompt,
        &recovery_prompt,
    );
    start_metadata.insert(
        "context_window_tokens".to_string(),
        config.context_window_tokens.to_string(),
    );
    let mut continuation_metadata = merge_persistable_run_context(Metadata::new(), &run_context);
    continuation_metadata.insert("continuation_replay".to_string(), "true".to_string());
    continuation_metadata.insert("display_content".to_string(), display_prompt.clone());
    insert_user_message_model_prompt(&mut continuation_metadata, &display_prompt, &prompt);
    commit_agent_run_start_with(cancellation, || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .with_immediate_transaction(|store| {
                let claimed = claim_agent_recovery_envelope_in_transaction(
                    store,
                    &run_context,
                    &[AgentRecoveryState::Paused],
                    AgentRecoveryReason::UserContinued,
                )
                .map_err(StorageError::new)?;
                let claim_matches = match (recovery.as_ref(), claimed.as_ref()) {
                    (None, None) => true,
                    (Some(expected), Some(claimed)) => {
                        claimed.identity.resume_key == expected.identity.resume_key
                            && claimed.attempts == expected.attempts.saturating_add(1)
                    }
                    _ => false,
                };
                if !claim_matches {
                    return Err(StorageError::new(
                        "agent recovery claim changed before durable retry start",
                    ));
                }
                append_event(
                    store,
                    &task_id,
                    EventKind::TaskStatusChanged,
                    "Agent task retry started",
                    start_metadata,
                )?;
                append_message_event_with_metadata(
                    store,
                    &task_id,
                    MessageRole::User,
                    &display_prompt,
                    continuation_metadata,
                )?;
                if let Some(metadata) = checkpoint_fallback {
                    append_event(
                        store,
                        &task_id,
                        EventKind::TaskStatusChanged,
                        "Agent task checkpoint fallback",
                        metadata,
                    )?;
                }
                Ok(())
            })
            .map_err(|error| error.to_string())
    })?;
    drop(start_gate.take());
    let runtime_config = cancellation.runtime_config();
    let mut runtime = if let Some(mut runtime) = restored_task_state {
        runtime.messages = history;
        runtime.messages.push(Message {
            role: MessageRole::User,
            content: prompt.clone(),
            metadata: Metadata::new(),
        });
        cancellation.extend_runtime_budget(&mut runtime);
        runtime
    } else if history.is_empty() {
        start_agent_loop(task_id.clone(), prompt.clone(), runtime_config.clone())
    } else {
        start_agent_loop_with_history(task_id.clone(), prompt.clone(), history, runtime_config)
    };
    runtime.user_prompt = recovery_prompt;
    if let Some(message) = runtime.messages.last_mut() {
        message
            .metadata
            .insert("continuation_replay".to_string(), "true".to_string());
        message
            .metadata
            .insert("display_content".to_string(), display_prompt);
        message
            .metadata
            .insert("model_content".to_string(), prompt.clone());
    }
    let prepared = match prepare_agent_execution(
        host,
        state,
        &config,
        &task_id,
        &root,
        run_context,
        runtime,
        prompt,
        artifact_manifest,
        effort,
        cancellation,
    ) {
        Ok(prepared) => prepared,
        Err(AgentRunPreparationError::Finished(result)) => return *result,
        Err(AgentRunPreparationError::ControlStop(run_context)) => {
            return finish_agent_run_for_control_stop(host, state, &run_context, cancellation);
        }
        Err(AgentRunPreparationError::Runtime { error, run_context }) => {
            return agent_state_with_error_in_context(state, &run_context, error)
        }
    };
    continue_agent_loop(host, state, &config, &root, prepared, effort, cancellation)
}

/// Resolve a run parked at the plan-then-confirm gate. `approve` resumes the
/// run with the confirmed plan injected as protected context; `discard`
/// resumes ordinary execution without it; `cancel` cancels the whole run. The
/// decision is persisted with the run events before any resume or cancel, and
/// resuming reuses the ordinary paused-run continuation path so the plan
/// phase's Worker-stage budget stays charged to the same logical run.
#[tauri::command]
pub(crate) async fn resolve_agent_plan_confirmation(
    app: tauri::AppHandle,
    session_id: String,
    decision: String,
    resolved_by: String,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        resolve_agent_plan_confirmation_blocking(
            &app,
            state.inner(),
            session_id,
            decision,
            resolved_by,
        )
    })
    .await
    .map_err(|error| format!("agent plan confirmation failed to join: {error}"))?
}

pub(crate) fn resolve_agent_plan_confirmation_blocking(
    host: &dyn AgentRunHost,
    state: &AppState,
    session_id: String,
    decision: String,
    resolved_by: String,
) -> Result<AgentState, String> {
    let decision = crate::agent_plan_mode_runtime::parse_plan_confirmation_decision(&decision)?;
    let resolved_by = crate::agent_plan_mode_runtime::parse_plan_resolved_by(&resolved_by)?;
    if resolved_by == crate::agent_plan_mode_runtime::PLAN_RESOLVED_BY_AUTO_TIMEOUT {
        // The idle auto-approve timer may only start an execution while the
        // strict approval policy still gates every effect individually. Under
        // session/all policies the run trends fully automatic, so the plan
        // gate must stay an explicit user decision there (fail-closed).
        let config = clone_provider_config(state)?;
        if config.approval_policy != "strict" {
            return Err("automatic plan approval requires the strict approval policy".to_string());
        }
    }
    let mut run_context = project_session_metadata_for_session(state, Some(&session_id))?;
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = agent_events_for_session(&store, &phase16_task_id(), Some(&session_id))
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, Some(&session_id));
        // Compare-and-set under one lock: the resolution is appended only while
        // the same plan is still pending, so concurrent resolutions cannot both
        // write contradictory durable decisions.
        let pending = crate::agent_plan_mode_runtime::pending_plan_confirmation(&active_events)
            .ok_or_else(|| "no plan is awaiting confirmation for this session".to_string())?;
        if let Some(start) = active_events
            .iter()
            .find(|event| is_agent_run_start_event(event))
        {
            if let Some(identity) = AgentRunIdentity::from_metadata(&start.metadata)
                .map_err(|error| format!("invalid active agent run identity: {error}"))?
            {
                identity
                    .insert_into(&mut run_context)
                    .map_err(|error| format!("invalid active agent run identity: {error}"))?;
            }
        }
        append_event(
            &mut store,
            &phase16_task_id(),
            crate::agent_plan_mode_runtime::plan_event_kind(),
            crate::agent_plan_mode_runtime::PLAN_RESOLVED_SUMMARY,
            metadata_with_context(
                crate::agent_plan_mode_runtime::plan_resolved_event_metadata(
                    decision,
                    &pending.plan_digest,
                    resolved_by,
                ),
                &run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    if decision == crate::agent_plan_mode_runtime::PlanConfirmationDecision::Cancelled {
        return cancel_agent_task_blocking(host, state, &session_id);
    }
    retry_agent_task_blocking(host, state, SessionActionInput { session_id })
}

#[cfg(test)]
#[path = "../agent_product_path_tests.rs"]
mod product_path_tests;
