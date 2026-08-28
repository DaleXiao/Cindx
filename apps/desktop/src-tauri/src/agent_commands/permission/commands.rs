use super::observations::{
    copy_replayed_contract_evidence_metadata, current_permission_observation_boundary,
    permission_checkpoint_message_boundary, persisted_permission_observations,
    restore_permission_snapshot_from_boundary,
};
use super::resolution::resolve_agent_permission_request;
use super::restore_permission_run_context;
use crate::agent_preparation_runtime::effective_prompt_objective_for_messages;
use crate::agent_run_engine::PreparedAgentExecution;
use crate::suspended_run_runtime::{
    append_observations_to_suspended_run, remember_suspended_agent_run,
    suspended_agent_run_control_snapshot, suspended_agent_run_policy, take_suspended_agent_run,
};
use crate::*;
use agent_core::{
    AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_IDENTITY_V1_SCHEMA,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};

#[tauri::command]
pub(crate) async fn resolve_agent_permission(
    app: tauri::AppHandle,
    request_id: String,
    decision: String,
    session_id: String,
    grant_command_prefix: Option<bool>,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        resolve_agent_permission_blocking(
            &app,
            state,
            request_id,
            decision,
            session_id,
            grant_command_prefix.unwrap_or(false),
        )
    })
    .await
    .map_err(|error| format!("agent permission resume failed to join: {error}"))?
}

pub(crate) fn resolve_agent_permission_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
    grant_command_prefix: bool,
) -> Result<AgentState, String> {
    // A write subagent's patch approval belongs to a run that is still
    // executing (parked inside the delegating tool batch): resolve it in
    // place without run-control registration or a loop resume.
    if let Some(agent_state) = super::resolution::resolve_subagent_permission_if_pending(
        &state,
        &request_id,
        &decision,
        &session_id,
    )? {
        return Ok(agent_state);
    }
    let snapshot = suspended_agent_run_control_snapshot(&state, &session_id)?;
    let recovery_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let (request_effort, durable_recovery, applied_steer_epoch) = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let persisted_effort = store
            .get_permission_request(&PermissionRequestId(request_id.clone()))
            .map_err(|error| error.to_string())?
            .and_then(|request| request.metadata.get("agent_effort").cloned());
        let effort = persisted_effort
            .as_deref()
            .map(|value| persisted_agent_policy(Some(value)))
            .transpose()?;
        let recovery = if snapshot.is_some() {
            None
        } else {
            peek_agent_recovery_envelope(&store, &recovery_context, &[AgentRecoveryState::Blocked])?
        };
        let events = agent_events_for_session(&store, &phase16_task_id(), Some(&session_id))
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, Some(&session_id));
        (
            effort,
            recovery,
            latest_applied_agent_steer_epoch(&active_events),
        )
    };
    let suspended_effort = suspended_agent_run_policy(&state, &session_id)?;
    let effort = match (request_effort, suspended_effort) {
        (Some(requested), Some(suspended)) if requested != suspended => {
            return Err("suspended agent policy does not match the permission request".to_string())
        }
        (Some(requested), _) => requested,
        (None, Some(suspended)) => suspended,
        (None, None) => AgentPolicy::Default,
    };
    let run_control_lease = if let Some(snapshot) = snapshot {
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), Some(snapshot))?
    } else if let Some(resources) = durable_recovery.and_then(|recovery| recovery.resource_snapshot)
    {
        begin_agent_run_control_from_persisted_resources(
            &state,
            &session_id,
            effort.label(),
            applied_steer_epoch,
            resources,
            false,
        )?
    } else {
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), None)?
    };
    let cancellation = run_control_lease.control();
    let outcome = resolve_agent_permission_blocking_inner(
        app,
        state.clone(),
        request_id,
        decision,
        session_id.clone(),
        effort,
        grant_command_prefix,
        &cancellation,
    );
    let Err(original) = outcome else {
        return outcome;
    };
    let recovery_result = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))
        .and_then(|mut store| {
            pause_permission_recovery_after_handoff_error(&mut store, &recovery_context)
        });
    if let Err(recovery_error) = recovery_result {
        return Err(format!(
            "{original}; failed to preserve permission recovery after handoff error: {recovery_error}"
        ));
    }
    Err(original)
}

pub(crate) fn resolve_agent_permission_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
    effort: AgentPolicy,
    grant_command_prefix: bool,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let mut run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let mut request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;
    if matches!(&decision, PermissionDecision::AllowForSession)
        && !permission_can_allow_session(&request)
    {
        return agent_state_for_session(
            &store,
            Some("this permission can only be allowed once".to_string()),
            Some(&session_id),
        )
        .map_err(|error| error.to_string());
    }
    restore_permission_run_context(&mut run_context, &request.metadata);
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();

    if request.task_id != phase16_task_id() {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the agent loop".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }
    let current =
        agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?;
    if !current
        .pending_approvals
        .iter()
        .any(|approval| approval.request_id == request_id.0)
    {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the active session".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }

    // "Allow prefix for this session": record a `command_prefix` marker on the
    // approved request before its resolution rows persist. The prefix is
    // derived from the approved command's own clean shell tokens; dangerous or
    // dynamically-structured commands never derive a prefix, so the approval
    // degrades to the exact-command session grant (fail-closed narrowing).
    if grant_command_prefix && matches!(&decision, PermissionDecision::AllowForSession) {
        if matches!(request.action.as_str(), "shell.run" | "process.start") {
            let prefix = request
                .metadata
                .get("command")
                .map(String::as_str)
                .and_then(agent_core::command_prefix_for_grant);
            if let Some(prefix) = prefix {
                request
                    .metadata
                    .insert("command_prefix".to_string(), prefix);
                store
                    .update_permission_request_metadata(&request)
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    drop(store);

    let mut resolved_permission_ids = BTreeSet::from([request.id.0.clone()]);
    let mut resolved_observations = vec![resolve_agent_permission_request(
        &state,
        &request,
        &decision,
        "local-user",
        &root,
        &run_context,
        cancellation,
    )?];

    if matches!(&decision, PermissionDecision::AllowForSession) {
        let pending = {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            pending_agent_permissions_for_run(
                &store,
                &phase16_task_id(),
                session_id,
                run_context.get("agent_run_id").map(String::as_str),
            )
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|pending| permission_capability_matches(&request, pending))
            .collect::<Vec<_>>()
        };
        for pending_request in pending {
            resolved_permission_ids.insert(pending_request.id.0.clone());
            resolved_observations.push(resolve_agent_permission_request(
                &state,
                &pending_request,
                &PermissionDecision::AllowForSession,
                "session-grant",
                &root,
                &run_context,
                cancellation,
            )?);
        }
    }

    let hot_resume_ready = match append_observations_to_suspended_run(
        &state,
        session_id.unwrap_or_default(),
        &resolved_observations,
    ) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "hot permission recovery unavailable; rebuilding from the durable checkpoint: {error}"
            );
            false
        }
    };

    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let pending = pending_agent_permissions_for_run(
        &store,
        &phase16_task_id(),
        session_id,
        run_context.get("agent_run_id").map(String::as_str),
    )
    .map_err(|error| error.to_string())?;
    if !pending.is_empty() {
        return agent_state_for_session(&store, None, session_id)
            .map_err(|error| error.to_string());
    }

    let recovery =
        peek_agent_recovery_envelope(&store, &run_context, &[AgentRecoveryState::Blocked])?;
    if let Some(recovery) = recovery.as_ref() {
        run_context = permission_recovery_run_context(
            &run_context,
            recovery,
            recovery.attempts.saturating_add(1),
        );
    }
    let events = match session_id {
        Some(session_id) => store.list_by_task_and_metadata_or_unscoped(
            &phase16_task_id(),
            "session_id",
            session_id,
        ),
        None => store.list_by_task(&phase16_task_id()),
    }
    .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let prompt = latest_agent_prompt_from_active_events(&active_events)
        .unwrap_or_else(|| "Continue the agent task.".to_string());
    let recovery_prompt =
        agent_recovery_prompt_from_active_events(&active_events).unwrap_or_else(|| prompt.clone());
    let mut transcript = agent_runtime_transcript_from_active_events(&active_events);
    let persisted_permission_observations =
        persisted_permission_observations(&transcript, &run_context);
    let permission_observation_boundary = current_permission_observation_boundary(
        &persisted_permission_observations,
        &resolved_permission_ids,
        transcript.len(),
    );
    drop(store);

    let hot_suspended = if hot_resume_ready {
        match session_id {
            Some(session_id) => match take_suspended_agent_run(&state, session_id) {
                Ok(suspended) => suspended,
                Err(error) => {
                    eprintln!(
                        "hot permission handoff unavailable; rebuilding from the durable checkpoint: {error}"
                    );
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };
    if let Some(mut suspended) = hot_suspended {
        let claimed_context = match commit_permission_recovery_claim(
            &state,
            &request,
            &request_id,
            &decision,
            &run_context,
        ) {
            Ok(context) => context,
            Err(error) => {
                remember_suspended_agent_run(&state, suspended).map_err(|restore_error| {
                    format!(
                        "{error}; failed to restore the unclaimed suspended run: {restore_error}"
                    )
                })?;
                return Err(error);
            }
        };
        for (key, value) in &claimed_context {
            suspended.run_context.insert(key.clone(), value.clone());
        }
        let workspace_root = suspended.workspace_root.clone();
        let prepared = PreparedAgentExecution {
            base_run_context: suspended.run_context.clone(),
            run_context: suspended.run_context,
            runtime: suspended.runtime,
            prompt: suspended.prompt,
            collaboration: suspended.collaboration,
        };
        return continue_agent_loop(
            app,
            &state,
            &config,
            &workspace_root,
            prepared,
            effort,
            cancellation,
        );
    }

    let recovered_effective_objective = initial_agent_objective_from_events(&active_events)
        .map(|initial| effective_prompt_objective_for_messages(&initial, &transcript))
        .unwrap_or_else(|| {
            crate::runtime_values::effective_agent_objective(&run_context, &recovery_prompt)
                .to_string()
        });
    run_context.insert(
        "effective_prompt_objective".to_string(),
        recovered_effective_objective,
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
    let restored = recovery.as_ref().and_then(|recovery| {
        let snapshot = recovery.task_state.as_ref()?;
        let checkpoint_boundary =
            permission_checkpoint_message_boundary(&active_events, recovery, snapshot)
                .unwrap_or(permission_observation_boundary);
        restore_permission_snapshot_from_boundary(
            snapshot,
            &recovery_prompt,
            recovered_prepared_task_state.effective_objective(),
            &transcript,
            checkpoint_boundary,
        )
    });
    let (mut runtime, replay_boundary) = restored.unwrap_or_else(|| {
        (
            resume_agent_loop_from_messages(
                phase16_task_id(),
                recovery_prompt.clone(),
                transcript[..permission_observation_boundary].to_vec(),
                cancellation.runtime_config(),
            ),
            0,
        )
    });
    let registry = tool_registry_for_state(&state, &root)?;
    let catalog = registry.specs();
    let (tools, completion_intent) = crate::agent_loop_runtime::planned_agent_tools(
        &registry,
        &run_context,
        crate::runtime_values::effective_agent_objective(&run_context, &prompt),
        config.context_window_tokens,
    );
    apply_run_task_contract_with_completion_intent(
        &mut runtime,
        &run_context,
        &tools,
        &catalog,
        None,
        &completion_intent,
    )?;
    let replayed_permission_observations = persisted_permission_observations
        .iter()
        .filter(|observation| observation.message_index >= replay_boundary)
        .collect::<Vec<_>>();
    let mut recovered_goal_deltas = Vec::new();
    for resolved in &replayed_permission_observations {
        let risk = registry
            .get(&resolved.tool_name)
            .map(|tool| tool.spec().risk);
        let denial = matches!(resolved.status, ToolOutcomeStatus::Denied)
            .then(agent_runtime::AgentActionDenialFeedback::user_permission);
        let goal_delta = AgentKernel::new(&mut runtime, &tools)
            .apply_persisted_tool_observation_with_denial(
                resolved.call_id.clone(),
                &resolved.tool_name,
                &resolved.input_fingerprint,
                resolved.target_witness.as_deref(),
                resolved.effect_witness.as_ref(),
                &resolved.status,
                risk.as_ref(),
                &resolved.observation,
                denial.as_ref(),
            );
        recovered_goal_deltas.extend(goal_delta);
        copy_replayed_contract_evidence_metadata(&runtime, &mut transcript, resolved.message_index);
    }
    runtime.messages = transcript;
    let objective_epoch = run_context_steer_epoch(&run_context);
    persist_cold_permission_recovery(
        &recovered_goal_deltas,
        !replayed_permission_observations.is_empty(),
        || {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            crate::agent_runtime_snapshot::persist_agent_runtime_snapshot(
                &mut store,
                &runtime,
                &run_context,
            )
        },
    )?;
    run_context =
        commit_permission_recovery_claim(&state, &request, &request_id, &decision, &run_context)?;
    for delta in &recovered_goal_deltas {
        cancellation.record_goal_delta_at(objective_epoch, delta);
    }
    let prepared = PreparedAgentExecution {
        base_run_context: run_context.clone(),
        run_context,
        runtime,
        prompt,
        collaboration: None,
    };
    continue_agent_loop(app, &state, &config, &root, prepared, effort, cancellation)
}

pub(super) fn permission_recovery_run_context(
    run_context: &Metadata,
    recovery: &AgentRecoveryEnvelope,
    attempts: u32,
) -> Metadata {
    let mut recovered = run_context.clone();
    recovered.insert(
        AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
        AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
    );
    recovered.insert(
        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
        recovery.identity.logical_run_id().to_string(),
    );
    recovered.insert(
        "recovery_resume_key".to_string(),
        recovery.identity.resume_key.clone(),
    );
    recovered.insert(
        "source_agent_run_id".to_string(),
        recovery.identity.source_run_id.clone(),
    );
    recovered.insert("recovery_attempts".to_string(), attempts.to_string());
    recovered.insert("continuation".to_string(), "true".to_string());
    if let Some(queue_id) = recovery.queue_id.as_ref() {
        recovered.insert("queue_id".to_string(), queue_id.clone());
    }
    recovered
}

fn commit_permission_recovery_claim(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    request_id: &PermissionRequestId,
    decision: &PermissionDecision,
    run_context: &Metadata,
) -> Result<Metadata, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .with_immediate_transaction(|store| {
            let claimed = claim_agent_recovery_envelope_in_transaction(
                store,
                run_context,
                &[AgentRecoveryState::Blocked],
                AgentRecoveryReason::PermissionResolved,
            )
            .map_err(StorageError::new)?;
            if run_context.contains_key("recovery_resume_key") && claimed.is_none() {
                return Err(StorageError::new(
                    "agent recovery checkpoint disappeared before permission resume",
                ));
            }
            let claimed_context = claimed
                .as_ref()
                .map(|recovery| {
                    permission_recovery_run_context(run_context, recovery, recovery.attempts)
                })
                .unwrap_or_else(|| run_context.clone());
            append_event(
                store,
                &request.task_id,
                EventKind::TaskStatusChanged,
                "Agent task resumed after permission",
                metadata_with_context(
                    [
                        ("permission_id".to_string(), request_id.0.clone()),
                        (
                            "decision".to_string(),
                            permission_decision_label(decision).to_string(),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                    &claimed_context,
                ),
            )?;
            Ok(claimed_context)
        })
        .map_err(|error| error.to_string())
}

pub(super) fn persist_cold_permission_recovery(
    recovered_goal_deltas: &[agent_runtime::AgentGoalDelta],
    recovered_state_changed: bool,
    commit_recovered_runtime: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if recovered_goal_deltas.is_empty() && !recovered_state_changed {
        return Ok(());
    }
    // Cold recovery has no in-memory suspended snapshot to fall back to. Make
    // the rebuilt contract state durable before the caller claims the recovery
    // and credits any Goal Delta to the live control.
    commit_recovered_runtime()?;
    Ok(())
}
