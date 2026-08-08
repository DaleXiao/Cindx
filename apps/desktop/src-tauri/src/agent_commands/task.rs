use super::queue::*;
use super::run_identity::{
    assign_continuation_agent_run_identity, assign_initial_agent_run_identity,
    inherited_agent_run_identity, inherited_agent_run_identity_from_events,
};
use crate::agent_execution_constraint::AgentExecutionConstraint;
use crate::agent_preparation_runtime::AgentMemoryEvaluationConstraint;
use crate::agent_run_engine::{
    prepare_agent_execution, AgentRunPreparationError, PreparedAgentExecution,
};
use crate::agent_strategy_runtime::effective_prompt_objective_for_messages;
use crate::prompt_profile_serving::prompt_profile_assignment_from_events;
use crate::suspended_run_runtime::{
    clear_suspended_agent_run, suspended_agent_run_control_snapshot, suspended_agent_run_policy,
    take_suspended_agent_run, SuspendedAgentRun,
};
use crate::*;
use agent_application::{
    insert_run_objectives, insert_run_start_prompts, insert_user_message_model_prompt,
    merge_persistable_run_context,
};
use agent_core::AgentRunIdentity;

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

#[tauri::command]
pub(crate) async fn run_agent_task(
    app: tauri::AppHandle,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        run_agent_task_blocking(&app, state, input)
    })
    .await
    .map_err(|error| format!("agent task failed to join: {error}"))?
}

pub(crate) fn run_agent_task_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: AgentTaskInput,
) -> Result<AgentState, String> {
    let session_id = input.session_id.clone();
    let effort = AgentPolicy::parse_ingress(&input.effort);
    let run_control_lease =
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), None)?;
    let cancellation = run_control_lease.control();
    let result = run_agent_task_blocking_inner(app, state.clone(), input, &cancellation);
    if let Ok(agent) = result.as_ref() {
        if agent.status == "completed" {
            if let Ok(Some(refinement)) =
                persist_completed_conversation_title(&state, &session_id, &agent.messages)
            {
                spawn_semantic_session_title_refinement(app.clone(), refinement);
            }
        }
    }
    result
}

pub(crate) fn run_agent_task_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: AgentTaskInput,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    run_agent_task_blocking_inner_with_execution_constraint(
        app,
        state,
        input,
        cancellation,
        AgentExecutionConstraint::Native,
    )
}

pub(crate) fn run_agent_task_blocking_inner_with_execution_constraint(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: AgentTaskInput,
    cancellation: &Arc<AgentRunControl>,
    execution_constraint: AgentExecutionConstraint,
) -> Result<AgentState, String> {
    run_agent_task_blocking_inner_with_evaluation_constraints(
        app,
        state,
        input,
        cancellation,
        execution_constraint,
        AgentMemoryEvaluationConstraint::Native,
    )
}

pub(crate) fn run_agent_task_blocking_inner_with_evaluation_constraints(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: AgentTaskInput,
    cancellation: &Arc<AgentRunControl>,
    execution_constraint: AgentExecutionConstraint,
    memory_constraint: AgentMemoryEvaluationConstraint,
) -> Result<AgentState, String> {
    let effort = AgentPolicy::parse_ingress(&input.effort);
    let user_prompt = input.prompt.trim().to_string();
    let queue_id = input.queue_id.clone();
    let session_id = input.session_id;
    clear_suspended_agent_run(&state, &session_id)?;
    let mut run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let mut config = clone_provider_config(&state)?;
    config.agent_system_prompt = personalized_agent_instructions(
        &load_personalization_config(),
        &config.agent_system_prompt,
    );
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }

    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let attachments = validate_agent_attachments(&root, input.attachments)?;
    if user_prompt.is_empty() && attachments.is_empty() {
        return agent_state_with_error_in_context(&state, &run_context, "agent prompt is empty");
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
    run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    assign_initial_agent_run_identity(&mut run_context)?;
    execution_constraint.write_to_context(&mut run_context);
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
    add_image_generation_run_context(&mut run_context, &config, &prompt);
    add_agent_run_budget_metadata(&mut run_context, cancellation);
    run_context.insert(
        "requested_policy".to_string(),
        requested_policy.label().to_string(),
    );
    let session_id = run_context.get("session_id").map(String::as_str);
    let task_id = phase16_task_id();
    let (history, artifact_manifest) = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        delete_persisted_agent_runtime_snapshot(&mut store, session_id)?;
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
        let mut start_metadata = run_context.clone();
        insert_run_start_prompts(&mut start_metadata, &display_prompt, &prompt, &prompt);
        start_metadata.insert(
            "context_window_tokens".to_string(),
            config.context_window_tokens.to_string(),
        );
        let mut message_metadata = merge_persistable_run_context(Metadata::new(), &run_context);
        insert_user_message_model_prompt(&mut message_metadata, &display_prompt, &prompt);
        add_attachment_metadata(&mut message_metadata, &attachments);
        store
            .with_immediate_transaction(|store| {
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
            .map_err(|error| error.to_string())?;
        (history, artifact_manifest)
    };

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
    let prepared = match prepare_agent_execution(
        app,
        &state,
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
        Err(AgentRunPreparationError::ControlStop(run_context)) => {
            return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
        }
        Err(AgentRunPreparationError::Collaboration { error, run_context }) => {
            return agent_state_with_error_in_context(
                &state,
                &run_context,
                format!("Collaboration failed: {error}"),
            );
        }
        Err(AgentRunPreparationError::Runtime { error, run_context }) => {
            return agent_state_with_error_in_context(&state, &run_context, error)
        }
    };
    continue_agent_loop(app, &state, &config, &root, prepared, effort, cancellation)
}

#[tauri::command]
pub(crate) fn cancel_agent_task(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    let state = app.state::<AppState>();
    let _lifecycle = state
        .session_lifecycle_gate
        .lock()
        .map_err(|error| format!("session lifecycle gate poisoned: {error}"))?;
    let active_run_cancelled = request_agent_run_cancel(&state, &input.session_id)?;
    clear_suspended_agent_run(&state, &input.session_id)?;
    let run_context = project_session_metadata_for_session(&state, Some(&input.session_id))?;
    let session_id = run_context.get("session_id").map(String::as_str);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let current =
        agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?;
    if !current.can_cancel && !active_run_cancelled {
        return Ok(current);
    }

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        metadata_with_context(
            [("reason".to_string(), "user_cancelled".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    delete_persisted_agent_runtime_snapshot(&mut store, session_id)?;
    if let Err(error) = refresh_project_memory_after_run(&mut store, &run_context) {
        eprintln!("project memory checkpoint unavailable: {error}");
    }

    emit_agent_stream_delta(
        &app,
        "agent-cancelled",
        Some(&input.session_id),
        "",
        true,
        true,
        None,
    );

    agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn retry_agent_task(
    app: tauri::AppHandle,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        retry_agent_task_blocking(&app, state, input)
    })
    .await
    .map_err(|error| format!("agent retry failed to join: {error}"))?
}

pub(crate) fn retry_agent_task_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
) -> Result<AgentState, String> {
    let session_id = input.session_id.clone();
    let recovery_context = project_session_metadata_for_session(&state, Some(&session_id))?;
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
    let suspended_effort = suspended_agent_run_policy(&state, &session_id)?;
    if suspended_effort.is_some_and(|effort| effort != persisted_effort) {
        return Err("suspended agent policy does not match the active run".to_string());
    }
    let effort = suspended_effort.unwrap_or(persisted_effort);
    let suspended_snapshot = suspended_agent_run_control_snapshot(&state, &session_id)?;
    let run_control_lease = if let Some(snapshot) = suspended_snapshot {
        begin_agent_run_control_for_continuation(&state, &session_id, snapshot)?
    } else if let Some(recovery) = durable_recovery
        .as_ref()
        .filter(|recovery| recovery.resource_snapshot.is_some())
    {
        begin_agent_run_control_from_persisted_resources(
            &state,
            &session_id,
            effort.label(),
            applied_steer_epoch,
            recovery
                .resource_snapshot
                .clone()
                .expect("filtered durable resource snapshot"),
            recovery.reason != AgentRecoveryReason::AppRestarted,
        )?
    } else {
        begin_agent_run_control_at_steer_epoch(
            &state,
            &session_id,
            effort.label(),
            applied_steer_epoch,
        )?
    };
    let cancellation = run_control_lease.control();
    let suspended = take_suspended_agent_run(&state, &session_id)?;
    if let Some(suspended) = suspended {
        resume_suspended_agent_run(app, &state, suspended, &cancellation)
    } else {
        retry_agent_task_blocking_inner(app, state.clone(), input, &cancellation)
    }
}

pub(crate) fn resume_suspended_agent_run(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    suspended: SuspendedAgentRun,
    cancellation: &Arc<AgentRunControl>,
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
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        claim_agent_recovery_envelope(
            &mut store,
            &run_context,
            &[AgentRecoveryState::Paused],
            AgentRecoveryReason::UserContinued,
        )?
    };
    let mut inherited_identity = inherited_agent_run_identity(&run_context)?;
    if let Some(recovery) = recovery {
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
            recovery.attempts.to_string(),
        );
        run_context.insert("continuation".to_string(), "true".to_string());
        if let Some(queue_id) = recovery.queue_id {
            run_context.insert("queue_id".to_string(), queue_id);
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
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
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
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            metadata_with_context(retry_metadata, &run_context),
        )
        .map_err(|error| error.to_string())?;
    }
    let prepared = PreparedAgentExecution {
        base_run_context: run_context.clone(),
        run_context,
        runtime,
        prompt,
        collaboration,
    };
    continue_agent_loop(
        app,
        state,
        &config,
        &workspace_root,
        prepared,
        effort,
        cancellation,
    )
}

pub(crate) fn retry_agent_task_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SessionActionInput,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    clear_suspended_agent_run(&state, &input.session_id)?;
    let mut run_context = project_session_metadata_for_session(&state, Some(&input.session_id))?;
    run_context.insert(
        "current_time".to_string(),
        normalized_current_time_context(""),
    );
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
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
        prompt_profile_assignment,
    ) = {
        let mut store = state
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
        let recovery = claim_agent_recovery_envelope(
            &mut store,
            &run_context,
            &[AgentRecoveryState::Paused],
            AgentRecoveryReason::UserContinued,
        )?;
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
        let prompt_profile_assignment = inherited_identity
            .as_ref()
            .map(|(logical_run_id, _)| {
                prompt_profile_assignment_from_events(&events, logical_run_id)
            })
            .transpose()?
            .unwrap_or_default();
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
            prompt_profile_assignment,
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
            recovery.attempts.to_string(),
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
    run_context.extend(prompt_profile_assignment);
    let requested_policy = effort.requested_policy();
    run_context.insert("agent_effort".to_string(), effort.label().to_string());
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
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
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
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            start_metadata,
        )
        .map_err(|error| error.to_string())?;
        let mut continuation_metadata =
            merge_persistable_run_context(Metadata::new(), &run_context);
        continuation_metadata.insert("continuation_replay".to_string(), "true".to_string());
        continuation_metadata.insert("display_content".to_string(), display_prompt.clone());
        insert_user_message_model_prompt(&mut continuation_metadata, &display_prompt, &prompt);
        append_message_event_with_metadata(
            &mut store,
            &task_id,
            MessageRole::User,
            &display_prompt,
            continuation_metadata,
        )
        .map_err(|error| error.to_string())?;
    }

    let (history, artifact_manifest, restored_task_state) = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = store
            .list_by_task(&task_id)
            .map_err(|error| error.to_string())?;
        let session_events = session_id
            .as_deref()
            .map(|session_id| agent_session_events(&events, session_id))
            .unwrap_or_default();
        let mut messages = recovery_safe_transcript(&session_events);
        if messages
            .last()
            .map(|message| matches!(message.role, MessageRole::User) && message.content == prompt)
            .unwrap_or(false)
        {
            messages.pop();
        }
        let restored_task_state = recovery
            .as_ref()
            .and_then(|recovery| recovery.task_state.as_ref())
            .and_then(|snapshot| {
                match snapshot.restore_with_effective_objective(
                    recovery_prompt.clone(),
                    messages.clone(),
                    recovered_prepared_task_state.effective_objective(),
                ) {
                    Ok(runtime) => Some(runtime),
                    Err(error) => {
                        let _ = append_event(
                            &mut store,
                            &task_id,
                            EventKind::TaskStatusChanged,
                            "Agent task checkpoint fallback",
                            metadata_with_context(
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
                            ),
                        );
                        None
                    }
                }
            });
        (
            messages,
            artifact_manifest_message(&session_events),
            restored_task_state,
        )
    };
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
        app,
        &state,
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
        Err(AgentRunPreparationError::ControlStop(run_context)) => {
            return finish_agent_run_for_control_stop(app, &state, &run_context, cancellation);
        }
        Err(AgentRunPreparationError::Collaboration { error, run_context }) => {
            return agent_state_with_error_in_context(
                &state,
                &run_context,
                format!("Collaboration failed: {error}"),
            );
        }
        Err(AgentRunPreparationError::Runtime { error, run_context }) => {
            return agent_state_with_error_in_context(&state, &run_context, error)
        }
    };
    continue_agent_loop(app, &state, &config, &root, prepared, effort, cancellation)
}
