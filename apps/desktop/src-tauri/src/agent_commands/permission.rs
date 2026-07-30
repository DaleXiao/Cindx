use crate::agent_run_engine::PreparedAgentExecution;
use crate::*;

const PERMISSION_TOOL_OBSERVATION_SCHEMA: &str = "cindx.permission-tool-observation.v1";
const PERMISSION_TOOL_OBSERVATION_PROVENANCE: &str = "runtime_permission_resolution";

const PERMISSION_RUN_CONTEXT_KEYS: &[&str] = &[
    "agent_run_id",
    "agent_effort",
    "agent_model",
    "requested_policy",
    "collaboration_policy",
    "current_time",
    "effective_prompt_objective",
    "steer_epoch",
    "prompt_contract_epoch",
    "task_class",
    "collaboration_profile",
    "conductor_contract",
    "run_decision",
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

fn permission_prompt_contract_epoch(run_context: &Metadata) -> u64 {
    run_context
        .get("prompt_contract_epoch")
        .or_else(|| run_context.get("steer_epoch"))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

fn permission_tool_observation_metadata(
    request_id: &PermissionRequestId,
    tool_call_id: &str,
    tool_name: &str,
    status: &ToolOutcomeStatus,
    tool_input: &str,
    run_context: &Metadata,
) -> Metadata {
    let metadata = [
        ("kind".to_string(), "tool_observation".to_string()),
        ("tool_call_id".to_string(), tool_call_id.to_string()),
        ("tool".to_string(), tool_name.to_string()),
        ("status".to_string(), tool_outcome_label(status).to_string()),
        ("permission_id".to_string(), request_id.0.clone()),
        (
            "permission_observation_schema".to_string(),
            PERMISSION_TOOL_OBSERVATION_SCHEMA.to_string(),
        ),
        (
            "permission_observation_provenance".to_string(),
            PERMISSION_TOOL_OBSERVATION_PROVENANCE.to_string(),
        ),
        (
            "tool_input_fingerprint".to_string(),
            agent_runtime::tool_input_fingerprint(tool_name, tool_input),
        ),
        (
            "prompt_contract_epoch".to_string(),
            permission_prompt_contract_epoch(run_context).to_string(),
        ),
    ]
    .into_iter()
    .collect();
    metadata_with_context(metadata, run_context)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistedPermissionObservation {
    message_index: usize,
    permission_id: PermissionRequestId,
    call_id: agent_core::ToolCallId,
    tool_name: String,
    input_fingerprint: String,
    status: ToolOutcomeStatus,
    observation: String,
}

fn persisted_tool_outcome_status(value: &str) -> Option<ToolOutcomeStatus> {
    match value {
        "succeeded" => Some(ToolOutcomeStatus::Succeeded),
        "failed" => Some(ToolOutcomeStatus::Failed),
        "cancelled" => Some(ToolOutcomeStatus::Cancelled),
        "denied" => Some(ToolOutcomeStatus::Denied),
        _ => None,
    }
}

fn persisted_permission_observations(
    transcript: &[Message],
    run_context: &Metadata,
) -> Vec<PersistedPermissionObservation> {
    let Some(session_id) = run_context.get("session_id") else {
        return Vec::new();
    };
    let Some(agent_run_id) = run_context.get("agent_run_id") else {
        return Vec::new();
    };
    let prompt_contract_epoch = permission_prompt_contract_epoch(run_context).to_string();

    transcript
        .iter()
        .enumerate()
        .filter_map(|(message_index, message)| {
            if message.role != MessageRole::Tool
                || message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
                || message
                    .metadata
                    .get("permission_observation_schema")
                    .map(String::as_str)
                    != Some(PERMISSION_TOOL_OBSERVATION_SCHEMA)
                || message
                    .metadata
                    .get("permission_observation_provenance")
                    .map(String::as_str)
                    != Some(PERMISSION_TOOL_OBSERVATION_PROVENANCE)
                || message.metadata.get("session_id") != Some(session_id)
                || message.metadata.get("agent_run_id") != Some(agent_run_id)
                || message
                    .metadata
                    .get("prompt_contract_epoch")
                    .map(String::as_str)
                    != Some(prompt_contract_epoch.as_str())
                || message
                    .metadata
                    .get("permission_id")
                    .is_none_or(|value| value.trim().is_empty())
            {
                return None;
            }
            let permission_id = message
                .metadata
                .get("permission_id")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let call_id = message
                .metadata
                .get("tool_call_id")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let tool_name = message
                .metadata
                .get("tool")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let input_fingerprint = message
                .metadata
                .get("tool_input_fingerprint")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let status = persisted_tool_outcome_status(message.metadata.get("status")?)?;
            Some(PersistedPermissionObservation {
                message_index,
                permission_id: PermissionRequestId(permission_id),
                call_id: agent_core::ToolCallId(call_id),
                tool_name,
                input_fingerprint,
                status,
                observation: message.content.clone(),
            })
        })
        .collect()
}

fn current_permission_observation_boundary(
    observations: &[PersistedPermissionObservation],
    current_permission_ids: &BTreeSet<String>,
    transcript_len: usize,
) -> usize {
    observations
        .iter()
        .find(|observation| current_permission_ids.contains(&observation.permission_id.0))
        .map(|observation| observation.message_index)
        .unwrap_or(transcript_len)
}

fn permission_checkpoint_message_boundary(
    events: &[Event],
    recovery: &AgentRecoveryEnvelope,
    snapshot: &AgentTaskStateSnapshot,
) -> Option<usize> {
    let mut message_count = 0usize;
    for event in events {
        if event.kind == EventKind::MessageAdded {
            let role = event
                .metadata
                .get("role")
                .and_then(|role| message_role_from_label(role));
            let has_content = if role == Some(MessageRole::User) {
                event.metadata.contains_key("model_content")
                    || event.metadata.contains_key("content")
            } else {
                role.is_some() && event.metadata.contains_key("content")
            };
            if has_content {
                message_count = message_count.saturating_add(1);
            }
        }
        if event
            .metadata
            .get("recovery_resume_key")
            .map(String::as_str)
            != Some(recovery.resume_key.as_str())
            || event.metadata.get("recovery_state").map(String::as_str) != Some("blocked")
        {
            continue;
        }
        let matches_snapshot = event
            .metadata
            .get("recovery_envelope")
            .and_then(|encoded| serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok())
            .is_some_and(|checkpoint| checkpoint.task_state.as_ref() == Some(snapshot));
        if matches_snapshot {
            return Some(message_count);
        }
    }
    None
}

fn restore_permission_snapshot_from_boundary(
    snapshot: &AgentTaskStateSnapshot,
    recovery_prompt: &str,
    transcript: &[Message],
    boundary: usize,
) -> Option<(agent_runtime::AgentLoopState, usize)> {
    let boundary = boundary.min(transcript.len());
    snapshot
        .restore(recovery_prompt.to_string(), transcript[..boundary].to_vec())
        .ok()
        .map(|runtime| (runtime, boundary))
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
                ("permission_id".to_string(), request_id.0.clone()),
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
    let persisted_permission_observations =
        persisted_permission_observations(&transcript, &run_context);
    let permission_observation_boundary = current_permission_observation_boundary(
        &persisted_permission_observations,
        &resolved_permission_ids,
        transcript.len(),
    );
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

    let restored = recovery.as_ref().and_then(|recovery| {
        let snapshot = recovery.task_state.as_ref()?;
        let checkpoint_boundary =
            permission_checkpoint_message_boundary(&active_events, recovery, snapshot)
                .unwrap_or(permission_observation_boundary);
        restore_permission_snapshot_from_boundary(
            snapshot,
            &recovery_prompt,
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
    let mut tools = registry
        .exposure_plan(
            crate::runtime_values::effective_agent_objective(&run_context, &prompt),
            config.context_window_tokens,
        )
        .inline;
    let evidence_scopes = crate::agent_grounding_policy::pin_prompt_evidence_tools(
        &run_context,
        &registry.specs(),
        &mut tools,
    );
    apply_run_task_contract_with_evidence_scopes(
        &mut runtime,
        &run_context,
        &tools,
        None,
        &evidence_scopes,
    )?;
    for resolved in persisted_permission_observations
        .iter()
        .filter(|observation| observation.message_index >= replay_boundary)
    {
        let request = AgentToolRequest {
            call_id: resolved.call_id.clone(),
            tool_name: resolved.tool_name.clone(),
            input: serde_json::json!({
                "permission_input_fingerprint": resolved.input_fingerprint,
            })
            .to_string(),
        };
        let risk = registry
            .get(&resolved.tool_name)
            .map(|tool| tool.spec().risk);
        AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
            &request,
            &resolved.status,
            risk.as_ref(),
            &resolved.observation,
        );
    }
    runtime.messages = transcript;
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
                ("permission_id".to_string(), request_id.0.clone()),
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
        append_message_event_with_metadata(
            &mut store,
            &request.task_id,
            MessageRole::Tool,
            &observation,
            permission_tool_observation_metadata(
                &request_id,
                &tool_call_id,
                &tool_name,
                &status,
                &tool_input,
                run_context,
            ),
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
        let status = ToolOutcomeStatus::Denied;
        let observation =
            observation_from_tool_result(&tool_name, "denied", "The user denied this tool call.");
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Agent tool denied",
            metadata_with_context(
                [
                    ("tool_call_id".to_string(), tool_call_id.clone()),
                    ("tool".to_string(), tool_name.clone()),
                    ("status".to_string(), "denied".to_string()),
                    ("output".to_string(), observation.clone()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        append_message_event_with_metadata(
            &mut store,
            &request.task_id,
            MessageRole::Tool,
            &observation,
            permission_tool_observation_metadata(
                &request_id,
                &tool_call_id,
                &tool_name,
                &status,
                &tool_input,
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        (observation, status, Vec::new())
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

    fn permission_run_context(steer_epoch: u64, prompt_contract_epoch: u64) -> Metadata {
        [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
            ("steer_epoch".to_string(), steer_epoch.to_string()),
            (
                "prompt_contract_epoch".to_string(),
                prompt_contract_epoch.to_string(),
            ),
            (
                "effective_prompt_objective".to_string(),
                "核实当前屏幕上的保存按钮".to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    fn message(role: MessageRole, content: &str, metadata: Metadata) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata,
        }
    }

    fn permission_message(
        call_id: &str,
        tool_name: &str,
        status: ToolOutcomeStatus,
        content: &str,
        run_context: &Metadata,
    ) -> Message {
        permission_message_with_id(
            &format!("permission-{call_id}"),
            call_id,
            tool_name,
            status,
            content,
            run_context,
        )
    }

    fn permission_message_with_id(
        permission_id: &str,
        call_id: &str,
        tool_name: &str,
        status: ToolOutcomeStatus,
        content: &str,
        run_context: &Metadata,
    ) -> Message {
        message(
            MessageRole::Tool,
            content,
            permission_tool_observation_metadata(
                &PermissionRequestId(permission_id.to_string()),
                call_id,
                tool_name,
                &status,
                r#"{"secret":"super-secret"}"#,
                run_context,
            ),
        )
    }

    fn permission_recovery_event(sequence: u64, kind: EventKind, metadata: Metadata) -> Event {
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: phase16_task_id(),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: String::new(),
            metadata,
        }
    }

    fn permission_recovery_envelope(
        resume_key: &str,
        task_state: AgentTaskStateSnapshot,
    ) -> AgentRecoveryEnvelope {
        AgentRecoveryEnvelope {
            schema: AGENT_RECOVERY_SCHEMA.to_string(),
            resume_key: resume_key.to_string(),
            project_id: Some("project-a".to_string()),
            session_id: "session-a".to_string(),
            source_run_id: "run-a".to_string(),
            user_turn_sequence: 1,
            prompt_fingerprint: task_state.user_prompt_fingerprint.clone(),
            effort: "auto".to_string(),
            policy: "auto_router".to_string(),
            queue_id: None,
            workflow_resume_key: None,
            state: "blocked".to_string(),
            reason: "waiting_for_permission".to_string(),
            attempts: 0,
            model_calls: 1,
            tool_calls: 0,
            material_checkpoints: 0,
            observations: 0,
            budget_extensions: 0,
            task_state: Some(task_state),
            resource_snapshot: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn permission_checkpoint_boundary_uses_the_matching_blocked_event() {
        let message_event = |sequence, role: &str, content: &str| {
            permission_recovery_event(
                sequence,
                EventKind::MessageAdded,
                [
                    ("role".to_string(), role.to_string()),
                    ("content".to_string(), content.to_string()),
                ]
                .into_iter()
                .collect(),
            )
        };
        let mut runtime = start_agent_loop(
            TaskId("permission-boundary".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let older_snapshot = AgentTaskStateSnapshot::capture(&runtime);
        runtime
            .messages
            .push(message(MessageRole::Assistant, "working", Metadata::new()));
        let active_snapshot = AgentTaskStateSnapshot::capture(&runtime);
        let active_recovery = permission_recovery_envelope("active", active_snapshot.clone());
        let missing_recovery = permission_recovery_envelope("missing", active_snapshot.clone());
        let blocked_event = |sequence, envelope: AgentRecoveryEnvelope| {
            permission_recovery_event(
                sequence,
                EventKind::TaskStatusChanged,
                [
                    (
                        "recovery_resume_key".to_string(),
                        envelope.resume_key.clone(),
                    ),
                    ("recovery_state".to_string(), "blocked".to_string()),
                    (
                        "recovery_envelope".to_string(),
                        serde_json::to_string(&envelope).expect("envelope serializes"),
                    ),
                ]
                .into_iter()
                .collect(),
            )
        };
        let events = vec![
            message_event(1, "user", "inspect"),
            blocked_event(2, permission_recovery_envelope("older", older_snapshot)),
            message_event(3, "assistant", "working"),
            blocked_event(4, active_recovery.clone()),
            message_event(5, "tool", "resolved after the checkpoint"),
            blocked_event(6, active_recovery.clone()),
        ];

        assert_eq!(
            permission_checkpoint_message_boundary(&events, &active_recovery, &active_snapshot),
            Some(2)
        );
        assert_eq!(
            permission_checkpoint_message_boundary(&events, &missing_recovery, &active_snapshot),
            None
        );
    }

    #[test]
    fn permission_observation_marker_is_runtime_only_and_lineage_bound() {
        let run_context = permission_run_context(4, 0);
        let valid = permission_message(
            "call-valid",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "captured",
            &run_context,
        );
        assert!(valid
            .metadata
            .values()
            .all(|value| !value.contains("super-secret")));

        let mut legacy = valid.clone();
        legacy.metadata.remove("permission_observation_schema");
        let mut forged = valid.clone();
        forged.metadata.insert(
            "permission_observation_provenance".to_string(),
            "model_claim".to_string(),
        );
        let mut wrong_run = valid.clone();
        wrong_run
            .metadata
            .insert("agent_run_id".to_string(), "run-b".to_string());
        let mut wrong_contract_epoch = valid.clone();
        wrong_contract_epoch
            .metadata
            .insert("prompt_contract_epoch".to_string(), "4".to_string());
        let mut invalid_status = valid.clone();
        invalid_status
            .metadata
            .insert("status".to_string(), "unknown".to_string());

        let observations = persisted_permission_observations(
            &[
                legacy,
                forged,
                wrong_run,
                wrong_contract_epoch,
                invalid_status,
                valid,
            ],
            &run_context,
        );

        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].call_id.0, "call-valid");
        assert_eq!(observations[0].permission_id.0, "permission-call-valid");
        assert_eq!(observations[0].message_index, 5);
    }

    #[test]
    fn permission_boundary_uses_unique_permission_id_when_call_ids_repeat() {
        let run_context = permission_run_context(4, 0);
        let transcript = vec![
            permission_message_with_id(
                "permission-old",
                "reused-call",
                "computer.screenshot",
                ToolOutcomeStatus::Succeeded,
                "old capture",
                &run_context,
            ),
            permission_message_with_id(
                "permission-current",
                "reused-call",
                "computer.screenshot",
                ToolOutcomeStatus::Succeeded,
                "current capture",
                &run_context,
            ),
        ];
        let observations = persisted_permission_observations(&transcript, &run_context);

        assert_eq!(
            current_permission_observation_boundary(
                &observations,
                &["permission-current".to_string()].into_iter().collect(),
                transcript.len(),
            ),
            1
        );
    }

    #[test]
    fn cold_recovery_replays_all_permission_observations_without_duplicates() {
        let run_context = permission_run_context(4, 0);
        let tools = vec![ToolSpec::builtin(
            "computer.screenshot",
            "computer",
            "Capture the current screen",
            ToolRisk::SensitiveContext,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)];
        let mut original = start_agent_loop(
            TaskId("permission-recovery".to_string()),
            "核实当前屏幕上的保存按钮",
            AgentRuntimeConfig::default(),
        );
        original.messages.push(message(
            MessageRole::Assistant,
            "",
            [("tool_call_ids".to_string(), "call-a,call-b".to_string())]
                .into_iter()
                .collect(),
        ));
        apply_run_task_contract(&mut original, &run_context, &tools, None)
            .expect("contract applies before permission pause");
        let snapshot = AgentTaskStateSnapshot::capture(&original);
        let original_message_count = original.messages.len();

        let mut transcript = original.messages.clone();
        transcript.push(message(
            MessageRole::User,
            "noop control message after the blocked checkpoint",
            [("steer".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
        ));
        transcript.push(permission_message(
            "call-a",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "captured",
            &run_context,
        ));
        transcript.push(message(
            MessageRole::User,
            "Visual reference captured by computer.screenshot.",
            [("kind".to_string(), "visual_reference".to_string())]
                .into_iter()
                .collect(),
        ));
        transcript.push(permission_message(
            "call-b",
            "shell.run",
            ToolOutcomeStatus::Denied,
            "denied",
            &run_context,
        ));

        let observations = persisted_permission_observations(&transcript, &run_context);
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation.call_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["call-a", "call-b"]
        );
        let (mut restored, boundary) = restore_permission_snapshot_from_boundary(
            &snapshot,
            &original.user_prompt,
            &transcript,
            original_message_count,
        )
        .expect("blocked checkpoint should restore from its recorded message boundary");
        assert_eq!(boundary, original_message_count);

        apply_run_task_contract(&mut restored, &run_context, &tools, None)
            .expect("contract reapplies after recovery");
        for observation in observations
            .iter()
            .filter(|observation| observation.message_index >= boundary)
        {
            let request = AgentToolRequest {
                call_id: observation.call_id.clone(),
                tool_name: observation.tool_name.clone(),
                input: serde_json::json!({
                    "permission_input_fingerprint": observation.input_fingerprint,
                })
                .to_string(),
            };
            let risk = (observation.tool_name == "computer.screenshot")
                .then_some(ToolRisk::SensitiveContext);
            AgentKernel::new(&mut restored, &tools).apply_tool_observation(
                &request,
                &observation.status,
                risk.as_ref(),
                &observation.observation,
            );
        }
        restored.messages = transcript.clone();

        assert_eq!(
            AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
            Ok(None)
        );
        assert_eq!(restored.messages, transcript);
        assert_eq!(
            restored
                .messages
                .iter()
                .filter(|message| message.role == MessageRole::Tool)
                .count(),
            2
        );
    }

    #[test]
    fn later_permission_pause_restores_after_prior_permission_evidence() {
        let run_context = permission_run_context(4, 0);
        let tools = vec![ToolSpec::builtin(
            "computer.screenshot",
            "computer",
            "Capture the current screen",
            ToolRisk::SensitiveContext,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)];
        let mut prior = start_agent_loop(
            TaskId("sequential-permission-recovery".to_string()),
            "核实当前屏幕上的保存按钮",
            AgentRuntimeConfig::default(),
        );
        apply_run_task_contract(&mut prior, &run_context, &tools, None)
            .expect("initial contract applies");
        AgentKernel::new(&mut prior, &tools).apply_tool_observation(
            &AgentToolRequest {
                call_id: agent_core::ToolCallId("call-a".to_string()),
                tool_name: "computer.screenshot".to_string(),
                input: r#"{"display":0}"#.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::SensitiveContext),
            "captured first screen",
        );
        prior.messages.pop();
        prior.messages.push(permission_message(
            "call-a",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "captured first screen",
            &run_context,
        ));
        let snapshot = AgentTaskStateSnapshot::capture(&prior);
        let snapshot_boundary = prior.messages.len();

        let mut transcript = prior.messages.clone();
        transcript.push(message(
            MessageRole::Assistant,
            "continuing after the first approval",
            Metadata::new(),
        ));
        transcript.push(permission_message(
            "call-b",
            "shell.run",
            ToolOutcomeStatus::Denied,
            "denied",
            &run_context,
        ));
        let observations = persisted_permission_observations(&transcript, &run_context);
        let boundary = current_permission_observation_boundary(
            &observations,
            &["permission-call-b".to_string()].into_iter().collect(),
            transcript.len(),
        );
        assert_eq!(boundary, transcript.len() - 1);

        let (mut restored, restored_boundary) = restore_permission_snapshot_from_boundary(
            &snapshot,
            &prior.user_prompt,
            &transcript,
            snapshot_boundary,
        )
        .expect("latest blocked snapshot should match its recorded message boundary");
        assert_eq!(restored_boundary, snapshot_boundary);
        assert_eq!(
            AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
            Ok(None)
        );
    }
}
