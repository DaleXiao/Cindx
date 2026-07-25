use crate::*;

#[tauri::command]
pub(crate) async fn resolve_agent_permission(
    app: tauri::AppHandle,
    request_id: String,
    decision: String,
    session_id: String,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        resolve_agent_permission_blocking(&app, state, request_id, decision, session_id)
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
) -> Result<AgentState, String> {
    let snapshot = suspended_agent_run_control_snapshot(&state, &session_id)?;
    let effort = if snapshot.is_some() {
        AgentEffort::Auto
    } else {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .get_permission_request(&PermissionRequestId(request_id.clone()))
            .map_err(|error| error.to_string())?
            .and_then(|request| request.metadata.get("agent_effort").cloned())
            .map(|effort| AgentEffort::parse(&effort))
            .unwrap_or(AgentEffort::Auto)
    };
    let run_control_lease =
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), snapshot)?;
    let cancellation = run_control_lease.control();
    resolve_agent_permission_blocking_inner(
        app,
        state.clone(),
        request_id,
        decision,
        session_id.clone(),
        &cancellation,
    )
}

pub(crate) fn resolve_agent_permission_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
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
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;
    if matches!(&decision, PermissionDecision::AllowForSession)
        && matches!(&request.risk, PermissionRisk::Destructive)
    {
        return agent_state_for_session(
            &store,
            Some("destructive permissions can only be allowed once".to_string()),
            Some(&session_id),
        )
        .map_err(|error| error.to_string());
    }
    for key in [
        "agent_run_id",
        "agent_effort",
        "agent_model",
        "requested_policy",
        "collaboration_policy",
        "current_time",
        "task_class",
        "collaboration_profile",
        "queue_id",
        "recovery_resume_key",
        "recovery_attempts",
        "image_generation_required",
        "configured_image_model",
        "configured_image_endpoint",
    ] {
        if let Some(value) = request.metadata.get(key) {
            run_context.insert(key.to_string(), value.clone());
        }
    }
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
    drop(store);

    let mut resolved_observations = vec![resolve_agent_permission_request(
        &state,
        &request,
        &decision,
        "local-user",
        &root,
        &run_context,
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
            .filter(|pending| !matches!(&pending.risk, PermissionRisk::Destructive))
            .collect::<Vec<_>>()
        };
        for pending_request in pending {
            resolved_observations.push(resolve_agent_permission_request(
                &state,
                &pending_request,
                &PermissionDecision::AllowForSession,
                "session-grant",
                &root,
                &run_context,
            )?);
        }
    }

    append_observations_to_suspended_run(
        &state,
        session_id.unwrap_or_default(),
        &resolved_observations,
    )?;

    let mut store = state
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

    if let Some(recovery) = claim_agent_recovery_envelope(
        &mut store,
        &run_context,
        &["blocked"],
        "permission_resolved",
    )? {
        run_context.insert(
            "recovery_resume_key".to_string(),
            recovery.resume_key.clone(),
        );
        run_context.insert(
            "source_agent_run_id".to_string(),
            recovery.source_run_id.clone(),
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

    append_event(
        &mut store,
        &request.task_id,
        EventKind::TaskStatusChanged,
        "Agent task resumed after permission",
        metadata_with_context(
            [
                ("permission_id".to_string(), request_id.0),
                (
                    "decision".to_string(),
                    permission_decision_label(&decision).to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    let events = store
        .list_by_task(&phase16_task_id())
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let prompt = latest_agent_prompt_from_active_events(&active_events)
        .unwrap_or_else(|| "Continue the agent task.".to_string());
    let transcript = agent_transcript_from_active_events(&active_events);
    drop(store);

    if let Some(session_id) = session_id {
        if let Some(mut suspended) = take_suspended_agent_run(&state, session_id)? {
            cancellation.extend_runtime_budget(&mut suspended.runtime);
            for (key, value) in &run_context {
                suspended.run_context.insert(key.clone(), value.clone());
            }
            return continue_agent_loop(
                app,
                &state,
                &config,
                &suspended.workspace_root,
                suspended.runtime,
                suspended.prompt,
                suspended.run_context,
                suspended.collaboration.as_ref(),
                cancellation,
            );
        }
    }

    let mut runtime = resume_agent_loop_from_messages(
        phase16_task_id(),
        prompt.clone(),
        transcript,
        cancellation.runtime_config(),
    );
    cancellation.extend_runtime_budget(&mut runtime);
    continue_agent_loop(
        app,
        &state,
        &config,
        &root,
        runtime,
        prompt,
        run_context,
        None,
        cancellation,
    )
}

pub(crate) fn resolve_agent_permission_request(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    decision: &PermissionDecision,
    resolved_by: &str,
    root: &Path,
    run_context: &Metadata,
) -> Result<ResolvedToolObservation, String> {
    let request_id = request.id.clone();
    let tool_call_id = request
        .metadata
        .get("tool_call_id")
        .cloned()
        .unwrap_or_else(|| unique_id("agent-tool"));
    let tool_name = request
        .metadata
        .get("tool_name")
        .cloned()
        .unwrap_or_else(|| request.action.clone());
    let tool_input = request
        .metadata
        .get("tool_input")
        .cloned()
        .unwrap_or_default();
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: decision.clone(),
            resolved_at_ms: current_time_millis(),
            resolved_by: resolved_by.to_string(),
        })
        .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(decision)),
        metadata_with_context(
            [
                ("permission_id".to_string(), request_id.0),
                (
                    "decision".to_string(),
                    permission_decision_label(decision).to_string(),
                ),
                ("tool".to_string(), request.action.clone()),
                ("resolved_by".to_string(), resolved_by.to_string()),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;

    let (observation, status, image_paths) = if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(tool_call_id.clone()),
            task_id: request.task_id.clone(),
            tool_name: tool_name.clone(),
            input_json: tool_input.clone(),
            proposed_by_model: "agent-loop".to_string(),
            metadata: Metadata::new(),
        };
        drop(store);
        let registry = tool_registry_for_state(state, root)?;
        let result =
            execute_agent_tool_invocation(state, &registry, invocation, root, run_context)?;
        let observation = observation_from_agent_tool_result(&tool_name, &result);
        let image_paths = tool_result_image_paths(&result);
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_tool_message_event(
            &mut store,
            &request.task_id,
            &tool_call_id,
            &tool_name,
            tool_outcome_label(&result.status),
            &observation,
            Some(run_context),
        )
        .map_err(|error| error.to_string())?;
        if !image_paths.is_empty() {
            append_visual_reference_event(
                &mut store,
                &request.task_id,
                &tool_name,
                &image_paths,
                run_context,
            )?;
        }
        (observation, result.status, image_paths)
    } else {
        let observation = observation_from_tool_result(
            &request.action,
            "denied",
            "The user denied this tool call.",
        );
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Agent tool denied",
            metadata_with_context(
                [
                    ("tool_call_id".to_string(), tool_call_id.clone()),
                    ("tool".to_string(), request.action.clone()),
                    ("status".to_string(), "denied".to_string()),
                    ("output".to_string(), observation.clone()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        append_tool_message_event(
            &mut store,
            &request.task_id,
            &tool_call_id,
            &request.action,
            "denied",
            &observation,
            Some(run_context),
        )
        .map_err(|error| error.to_string())?;
        (observation, ToolOutcomeStatus::Denied, Vec::new())
    };
    Ok(ResolvedToolObservation {
        call_id: agent_core::ToolCallId(tool_call_id),
        tool_name,
        input_json: tool_input,
        status,
        observation,
        image_paths,
    })
}
