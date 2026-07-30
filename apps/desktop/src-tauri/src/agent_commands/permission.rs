use crate::agent_run_engine::PreparedAgentExecution;
use crate::*;

const PERMISSION_RUN_CONTEXT_KEYS: &[&str] = &[
    "agent_run_id",
    "agent_effort",
    "agent_model",
    "requested_policy",
    "collaboration_policy",
    "current_time",
    "task_class",
    "collaboration_profile",
    "conductor_contract",
    "verification_required",
    "queue_id",
    "recovery_resume_key",
    "recovery_attempts",
    "image_generation_required",
    "configured_image_model",
    "configured_image_endpoint",
];

fn restore_permission_run_context(run_context: &mut Metadata, request_metadata: &Metadata) {
    for key in PERMISSION_RUN_CONTEXT_KEYS {
        if let Some(value) = request_metadata.get(*key) {
            run_context.insert((*key).to_string(), value.clone());
        }
    }
}

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
    let recovery_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let (effort, durable_recovery, applied_steer_epoch) = if snapshot.is_some() {
        (AgentEffort::Auto, None, 0)
    } else {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let effort = store
            .get_permission_request(&PermissionRequestId(request_id.clone()))
            .map_err(|error| error.to_string())?
            .and_then(|request| request.metadata.get("agent_effort").cloned())
            .map(|effort| AgentEffort::parse(&effort))
            .unwrap_or(AgentEffort::Auto);
        let recovery = peek_agent_recovery_envelope(&store, &recovery_context, &["blocked"])?;
        let events = agent_events_for_session(&store, &phase16_task_id(), Some(&session_id))
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, Some(&session_id));
        (
            effort,
            recovery,
            latest_applied_agent_steer_epoch(&active_events),
        )
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
    drop(store);

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

    let recovery = claim_agent_recovery_envelope(
        &mut store,
        &run_context,
        &["blocked"],
        "permission_resolved",
    )?;
    if let Some(recovery) = recovery.as_ref() {
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
        if let Some(queue_id) = recovery.queue_id.as_ref() {
            run_context.insert("queue_id".to_string(), queue_id.clone());
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
    let transcript = agent_runtime_transcript_from_active_events(&active_events);
    let resolved_call_ids = resolved_observations
        .iter()
        .map(|observation| observation.call_id.0.clone())
        .collect::<BTreeSet<_>>();
    let checkpoint_transcript =
        checkpoint_transcript_before_resolved_tools(&transcript, &resolved_call_ids);
    drop(store);

    if let Some(session_id) = session_id {
        if let Some(mut suspended) = take_suspended_agent_run(&state, session_id)? {
            cancellation.extend_runtime_budget(&mut suspended.runtime);
            for (key, value) in &run_context {
                suspended.run_context.insert(key.clone(), value.clone());
            }
            let effort = AgentEffort::parse(
                suspended
                    .run_context
                    .get("agent_effort")
                    .map(String::as_str)
                    .unwrap_or("auto"),
            );
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
                &suspended.workspace_root,
                prepared,
                effort,
                cancellation,
            );
        }
    }

    let mut runtime = recovery
        .as_ref()
        .and_then(|recovery| recovery.task_state.as_ref())
        .and_then(|snapshot| {
            snapshot
                .restore(recovery_prompt.clone(), checkpoint_transcript.clone())
                .ok()
        })
        .map(|mut runtime| {
            for resolved in &resolved_observations {
                let request = AgentToolRequest {
                    call_id: resolved.call_id.clone(),
                    tool_name: resolved.tool_name.clone(),
                    input: resolved.input_json.clone(),
                };
                AgentKernel::new(&mut runtime, &[]).apply_tool_observation(
                    &request,
                    &resolved.status,
                    None,
                    &resolved.observation,
                );
                append_visual_reference_message(
                    &mut runtime,
                    &resolved.tool_name,
                    &resolved.image_paths,
                );
            }
            runtime.messages = transcript.clone();
            runtime
        })
        .unwrap_or_else(|| {
            resume_agent_loop_from_messages(
                phase16_task_id(),
                recovery_prompt,
                transcript,
                cancellation.runtime_config(),
            )
        });
    cancellation.extend_runtime_budget(&mut runtime);
    let effort = AgentEffort::parse(
        run_context
            .get("agent_effort")
            .map(String::as_str)
            .unwrap_or("auto"),
    );
    let prepared = PreparedAgentExecution {
        base_run_context: run_context.clone(),
        run_context,
        runtime,
        prompt,
        collaboration: None,
    };
    continue_agent_loop(app, &state, &config, &root, prepared, effort, cancellation)
}

fn checkpoint_transcript_before_resolved_tools(
    transcript: &[Message],
    resolved_call_ids: &BTreeSet<String>,
) -> Vec<Message> {
    let boundary = transcript
        .iter()
        .position(|message| {
            message.role == MessageRole::Tool
                && message
                    .metadata
                    .get("tool_call_id")
                    .is_some_and(|call_id| resolved_call_ids.contains(call_id))
        })
        .unwrap_or(transcript.len());
    transcript[..boundary].to_vec()
}

pub(crate) fn resolve_agent_permission_request(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    decision: &PermissionDecision,
    resolved_by: &str,
    root: &Path,
    run_context: &Metadata,
    cancellation: &Arc<AgentRunControl>,
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
        let (observation, status, image_paths) =
            match execute_agent_tool_invocation_for_objective_epoch(
                state,
                &registry,
                invocation,
                root,
                run_context,
                cancellation,
                run_context_steer_epoch(run_context),
            )? {
                AgentToolInvocationOutcome::Completed(result) => (
                    observation_from_agent_tool_result(&tool_name, &result),
                    result.status.clone(),
                    tool_result_image_paths(&result),
                ),
                AgentToolInvocationOutcome::RestartAfterSteer => (
                    observation_from_tool_result(
                        &tool_name,
                        "cancelled",
                        "Permission-approved tool call was superseded by user steering before its result could enter the agent transcript.",
                    ),
                    ToolOutcomeStatus::Cancelled,
                    Vec::new(),
                ),
            };
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_tool_message_event(
            &mut store,
            &request.task_id,
            &tool_call_id,
            &tool_name,
            tool_outcome_label(&status),
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
        (observation, status, image_paths)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_cold_recovery_preserves_verification_contract() {
        let mut run_context = Metadata::new();
        let conductor_contract = AgentRunDecision::direct("executor")
            .execution_contract("auto")
            .to_json()
            .expect("contract serializes");
        let request_metadata = [
            ("conductor_contract".to_string(), conductor_contract.clone()),
            ("tool_input".to_string(), "sensitive".to_string()),
        ]
        .into_iter()
        .collect();

        restore_permission_run_context(&mut run_context, &request_metadata);

        assert_eq!(
            run_context.get("conductor_contract").map(String::as_str),
            Some(conductor_contract.as_str())
        );
        assert!(!run_context.contains_key("tool_input"));
        assert_eq!(
            workspace_verification_policy_for_run_context(&run_context)
                .expect("restored contract decodes"),
            WorkspaceVerificationPolicy::RequiredAfterMutation
        );
    }

    fn message(role: MessageRole, content: &str, metadata: Metadata) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata,
        }
    }

    #[test]
    fn permission_recovery_checkpoint_excludes_resolved_tool_suffix() {
        let transcript = vec![
            message(MessageRole::User, "capture the page", Metadata::new()),
            message(
                MessageRole::Assistant,
                "",
                [("raw_tool_calls_json".to_string(), "[]".to_string())]
                    .into_iter()
                    .collect(),
            ),
            message(
                MessageRole::Tool,
                "captured",
                [("tool_call_id".to_string(), "call-1".to_string())]
                    .into_iter()
                    .collect(),
            ),
            message(
                MessageRole::User,
                "Visual reference captured by browser.capture.",
                [("kind".to_string(), "visual_reference".to_string())]
                    .into_iter()
                    .collect(),
            ),
        ];

        let checkpoint = checkpoint_transcript_before_resolved_tools(
            &transcript,
            &["call-1".to_string()].into_iter().collect(),
        );

        assert_eq!(checkpoint.len(), 2);
        assert_eq!(checkpoint[0].content, "capture the page");
    }
}
