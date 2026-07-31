use super::*;

#[tauri::command]
pub(crate) fn get_phase5_state(state: tauri::State<'_, AppState>) -> Result<Phase5State, String> {
    let root = active_workspace_root(&state)?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase5_state(&store, None, &root).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn run_tool(
    state: tauri::State<'_, AppState>,
    input: ToolRunInput,
) -> Result<Phase5State, String> {
    let root = active_workspace_root(&state)?;
    let run_context = project_session_metadata_for_session(&state, None)?;
    let tool_name = input.tool_name.trim().to_string();
    let task_id = phase5_task_id();
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId(unique_id("tool")),
        task_id: task_id.clone(),
        tool_name: tool_name.clone(),
        input_json: input.input,
        proposed_by_model: "local-user".to_string(),
        metadata: run_context,
    };
    let registry = tool_registry_for_state(&state, &root)?;
    let Some(tool) = registry.get(&tool_name) else {
        return phase5_state_with_error(&state, format!("unknown tool: {tool_name}"));
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_tool_proposed_event(&mut store, &invocation, None).map_err(|error| error.to_string())?;

    if let Some(mut request) = tool.permission_request(&invocation) {
        request.id = PermissionRequestId(unique_id("perm"));
        request
            .metadata
            .insert("phase".to_string(), "5".to_string());
        request
            .metadata
            .insert("tool_input".to_string(), invocation.input_json.clone());
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0.clone());
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name.clone());
        for (key, value) in &invocation.metadata {
            request
                .metadata
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        store
            .save_permission_request(request.clone(), current_time_millis())
            .map_err(|error| error.to_string())?;
        append_event(
            &mut store,
            &task_id,
            EventKind::PermissionRequested,
            format!("Permission requested for {}", request.action),
            [
                ("permission_id".to_string(), request.id.0),
                ("tool_call_id".to_string(), invocation.id.0),
                ("tool".to_string(), request.action),
                (
                    "risk".to_string(),
                    permission_risk_label(&request.risk).to_string(),
                ),
                ("scope".to_string(), request.scope),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;

        return phase5_state(&store, None, &root).map_err(|error| error.to_string());
    }

    drop(store);
    execute_manual_tool_invocation(
        &state.manual_tool_execution_gate,
        &state.store,
        &registry,
        invocation,
        &root,
        None,
    )?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    phase5_state(&store, None, &root).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn resolve_tool_permission(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
) -> Result<Phase5State, String> {
    let root = active_workspace_root(&state)?;
    let registry = tool_registry_for_state(&state, &root)?;
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;

    store
        .resolve_permission(PermissionResolution {
            request_id: request_id.clone(),
            decision: decision.clone(),
            resolved_at_ms: current_time_millis(),
            resolved_by: "local-user".to_string(),
        })
        .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!("Permission {}", permission_decision_past_tense(&decision)),
        [
            ("permission_id".to_string(), request_id.0.clone()),
            (
                "decision".to_string(),
                permission_decision_label(&decision).to_string(),
            ),
            ("tool".to_string(), request.action.clone()),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(
                request
                    .metadata
                    .get("tool_call_id")
                    .cloned()
                    .unwrap_or_else(|| unique_id("tool")),
            ),
            task_id: request.task_id.clone(),
            tool_name: request
                .metadata
                .get("tool_name")
                .cloned()
                .unwrap_or(request.action),
            input_json: request
                .metadata
                .get("tool_input")
                .cloned()
                .unwrap_or_default(),
            proposed_by_model: "local-user".to_string(),
            metadata: Metadata::new(),
        };
        drop(store);
        execute_manual_tool_invocation(
            &state.manual_tool_execution_gate,
            &state.store,
            &registry,
            invocation,
            &root,
            None,
        )?;
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        return phase5_state(&store, None, &root).map_err(|error| error.to_string());
    } else {
        append_event(
            &mut store,
            &request.task_id,
            EventKind::ToolCallFinished,
            "Tool call denied",
            [
                (
                    "tool_call_id".to_string(),
                    request
                        .metadata
                        .get("tool_call_id")
                        .cloned()
                        .unwrap_or_default(),
                ),
                (
                    "tool".to_string(),
                    request
                        .metadata
                        .get("tool_name")
                        .cloned()
                        .unwrap_or(request.action),
                ),
                ("status".to_string(), "denied".to_string()),
                ("output".to_string(), "denied by user".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    phase5_state(&store, None, &root).map_err(|error| error.to_string())
}
