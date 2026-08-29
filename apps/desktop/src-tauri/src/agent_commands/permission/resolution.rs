use super::observations::permission_tool_observation_metadata;
use super::restore_permission_run_context;
use crate::*;

pub(crate) fn persist_permission_resolution_rows(
    store: &mut SqliteStore,
    request: &PermissionRequest,
    resolution: &PermissionResolution,
    run_context: &Metadata,
) -> Result<(), StorageError> {
    store.resolve_permission_in_transaction(resolution)?;
    let mut metadata = [
        ("permission_id".to_string(), resolution.request_id.0.clone()),
        (
            "decision".to_string(),
            permission_decision_label(&resolution.decision).to_string(),
        ),
        ("tool".to_string(), request.action.clone()),
        ("resolved_by".to_string(), resolution.resolved_by.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(prefix) = request.metadata.get("command_prefix") {
        metadata.insert("command_prefix".to_string(), prefix.clone());
    }
    append_event(
        store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!(
            "Permission {}",
            permission_decision_past_tense(&resolution.decision)
        ),
        metadata_with_context(metadata, run_context),
    )
}

/// Resolve a write subagent's patch approval in place. The parent run is still
/// executing — parked inside the delegating tool batch — so there is no
/// suspended run to resume and no run control to register: the waiting
/// subagent thread observes the durable decision and executes an approved
/// patch itself. Only the resolution rows and the audit event are persisted;
/// no tool executes here and no message enters the parent transcript.
/// Returns `None` when the request is not subagent-originated so the caller
/// falls through to the ordinary suspended-run resolution path.
pub(super) fn resolve_subagent_permission_if_pending(
    state: &tauri::State<'_, AppState>,
    request_id: &str,
    decision: &str,
    session_id: &str,
) -> Result<Option<AgentState>, String> {
    let request = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .get_permission_request(&PermissionRequestId(request_id.to_string()))
            .map_err(|error| error.to_string())?
    };
    let Some(request) = request else { return Ok(None) };
    if request
        .metadata
        .get(crate::agent_subagent_runtime::SUBAGENT_PERMISSION_ORIGIN_KEY)
        .map(String::as_str)
        != Some(crate::agent_subagent_runtime::SUBAGENT_PERMISSION_ORIGIN_VALUE)
    {
        return Ok(None);
    }
    let decision = parse_permission_decision(decision).map_err(|error| error.to_string())?;
    let mut run_context = project_session_metadata_for_session(state, Some(session_id))?;
    restore_permission_run_context(&mut run_context, &request.metadata);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    resolve_subagent_permission_in_store(&mut store, &request, &decision, &run_context, session_id)
        .map(Some)
}

/// Store-level core of the subagent approval branch: validate the request the
/// same way the suspended-run path does (session-eligibility, agent-loop
/// ownership, pending membership), persist the resolution rows, and return the
/// refreshed state. Nothing executes and nothing resumes here.
pub(super) fn resolve_subagent_permission_in_store(
    store: &mut SqliteStore,
    request: &PermissionRequest,
    decision: &PermissionDecision,
    run_context: &Metadata,
    session_id: &str,
) -> Result<AgentState, String> {
    if matches!(decision, PermissionDecision::AllowForSession)
        && !permission_can_allow_session(request)
    {
        return agent_state_for_session(
            store,
            Some("this permission can only be allowed once".to_string()),
            Some(session_id),
        )
        .map_err(|error| error.to_string());
    }
    if request.task_id != phase16_task_id() {
        return agent_state_for_session(
            store,
            Some("permission does not belong to the agent loop".to_string()),
            Some(session_id),
        )
        .map_err(|error| error.to_string());
    }
    let current =
        agent_state_for_session(store, None, Some(session_id)).map_err(|error| error.to_string())?;
    if !current
        .pending_approvals
        .iter()
        .any(|approval| approval.request_id == request.id.0)
    {
        return agent_state_for_session(
            store,
            Some("permission does not belong to the active session".to_string()),
            Some(session_id),
        )
        .map_err(|error| error.to_string());
    }
    let resolution = PermissionResolution {
        request_id: request.id.clone(),
        decision: decision.clone(),
        resolved_at_ms: current_time_millis(),
        resolved_by: "local-user".to_string(),
    };
    store
        .with_immediate_transaction(|store| {
            persist_permission_resolution_rows(store, request, &resolution, run_context)
        })
        .map_err(|error| error.to_string())?;
    agent_state_for_session(store, None, Some(session_id)).map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn persist_denied_permission_resolution_rows(
    store: &mut SqliteStore,
    request: &PermissionRequest,
    resolution: &PermissionResolution,
    tool_call_id: &str,
    tool_name: &str,
    observation: &str,
    message_metadata: &Metadata,
    run_context: &Metadata,
) -> Result<(), StorageError> {
    persist_permission_resolution_rows(store, request, resolution, run_context)?;
    append_event(
        store,
        &request.task_id,
        EventKind::ToolCallFinished,
        "Agent tool denied",
        metadata_with_context(
            [
                ("tool_call_id".to_string(), tool_call_id.to_string()),
                ("tool".to_string(), tool_name.to_string()),
                ("status".to_string(), "denied".to_string()),
                (
                    "failure_code".to_string(),
                    "user_permission_denied".to_string(),
                ),
                ("output".to_string(), observation.to_string()),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )?;
    append_message_event_with_metadata(
        store,
        &request.task_id,
        MessageRole::Tool,
        observation,
        message_metadata.clone(),
    )
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
    let resolution = PermissionResolution {
        request_id: request_id.clone(),
        decision: decision.clone(),
        resolved_at_ms: current_time_millis(),
        resolved_by: resolved_by.to_string(),
    };
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    if matches!(decision, PermissionDecision::Deny) {
        let status = ToolOutcomeStatus::Denied;
        let observation =
            observation_from_tool_result(&tool_name, "denied", "The user denied this tool call.");
        let message_metadata = permission_tool_observation_metadata(
            &request_id,
            &tool_call_id,
            &tool_name,
            &status,
            &tool_input,
            None,
            None,
            None,
            run_context,
        );
        store
            .with_immediate_transaction(|store| {
                persist_denied_permission_resolution_rows(
                    store,
                    request,
                    &resolution,
                    &tool_call_id,
                    &tool_name,
                    &observation,
                    &message_metadata,
                    run_context,
                )
            })
            .map_err(|error| error.to_string())?;
        return Ok(ResolvedToolObservation {
            call_id: agent_core::ToolCallId(tool_call_id),
            tool_name,
            input_json: tool_input,
            risk: None,
            effect_spec: None,
            postcondition_evidence: None,
            status,
            observation,
            image_paths: Vec::new(),
            message_metadata,
        });
    }

    store
        .with_immediate_transaction(|store| {
            persist_permission_resolution_rows(store, request, &resolution, run_context)
        })
        .map_err(|error| error.to_string())?;

    let (
        observation,
        status,
        image_paths,
        message_metadata,
        risk,
        effect_spec,
        postcondition_evidence,
    ) = if matches!(
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
        let registered_tool = registry.get(&tool_name);
        let risk = registered_tool.map(|tool| tool.spec().risk);
        let effect_spec = registered_tool.map(|tool| tool.effect_spec(&invocation));
        let verification_invocation = invocation.clone();
        let (observation, status, image_paths, postcondition_evidence) =
            match execute_agent_tool_invocation_for_objective_epoch(
                state,
                &registry,
                invocation,
                root,
                run_context,
                cancellation,
                run_context_steer_epoch(run_context),
            )? {
                AgentToolInvocationOutcome::Completed(result) => {
                    let postcondition_evidence = registered_tool.and_then(|tool| {
                        tool.postcondition_evidence(&verification_invocation, &result)
                    });
                    (
                        observation_from_agent_tool_result(&tool_name, &result),
                        result.status.clone(),
                        tool_result_image_paths(&result),
                        postcondition_evidence,
                    )
                }
                AgentToolInvocationOutcome::RestartAfterSteer => (
                    observation_from_tool_result(
                        &tool_name,
                        "cancelled",
                        "Permission-approved tool call was superseded by user steering before its result could enter the agent transcript.",
                    ),
                    ToolOutcomeStatus::Cancelled,
                    Vec::new(),
                    None,
                ),
            };
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let message_metadata = permission_tool_observation_metadata(
            &request_id,
            &tool_call_id,
            &tool_name,
            &status,
            &tool_input,
            risk.as_ref(),
            effect_spec.as_ref(),
            postcondition_evidence.as_ref(),
            run_context,
        );
        append_message_event_with_metadata(
            &mut store,
            &request.task_id,
            MessageRole::Tool,
            &observation,
            message_metadata.clone(),
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
        (
            observation,
            status,
            image_paths,
            message_metadata,
            risk,
            effect_spec,
            postcondition_evidence,
        )
    } else {
        unreachable!("deny decisions return after their atomic persistence transaction")
    };
    Ok(ResolvedToolObservation {
        call_id: agent_core::ToolCallId(tool_call_id),
        tool_name,
        input_json: tool_input,
        risk,
        effect_spec,
        postcondition_evidence,
        status,
        observation,
        image_paths,
        message_metadata,
    })
}
