use super::*;

pub(crate) enum AgentToolBatchOutcome {
    Continue,
    RestartAfterSteer,
    Paused(Box<AgentState>),
}

fn paused_agent_tools(state: AgentState) -> AgentToolBatchOutcome {
    AgentToolBatchOutcome::Paused(Box::new(state))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_agent_tool_batch(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    active_collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    registry: &ToolRegistry,
    tools: &[ToolSpec],
    calls: Vec<AgentToolRequest>,
) -> Result<AgentToolBatchOutcome, String> {
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    if cancellation.has_pending_steer() && !agent_run_should_stop(cancellation) {
        return Ok(AgentToolBatchOutcome::RestartAfterSteer);
    }
    let mut waiting_for_permission = false;
    for call in calls {
        if cancellation.has_pending_steer() && !agent_run_should_stop(cancellation) {
            return Ok(AgentToolBatchOutcome::RestartAfterSteer);
        }
        if agent_run_should_stop(cancellation) {
            return Ok(paused_agent_tools(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                &*runtime,
                prompt,
                run_context,
                active_collaboration,
                cancellation,
            )?));
        }
        let mut invocation = AgentKernel::new(&mut *runtime, tools).tool_invocation(&call);
        let tool = registry.get(&call.tool_name);
        if let Some(tool) = tool {
            let effect_spec = tool.effect_spec(&invocation);
            agent_runtime::apply_tool_spec_runtime_metadata(&mut invocation, &effect_spec);
        }
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_tool_proposed_event(&mut store, &invocation, Some(run_context))
            .map_err(|error| error.to_string())?;

        if AgentKernel::new(&mut *runtime, tools).repeated_tool_failure_count(&call)
            >= MAX_IDENTICAL_TOOL_FAILURES
        {
            let observation = observation_from_tool_result(
                            &call.tool_name,
                            "failed",
                            "Cindx blocked this identical tool call after repeated failures. Change the arguments or use a different approach.",
                        );
            append_tool_finished_event(
                &mut store,
                &runtime.task_id,
                &call.call_id.0,
                &call.tool_name,
                "failed",
                &observation,
                [(
                    "failure_code".to_string(),
                    "repeated_call_blocked".to_string(),
                )]
                .into_iter()
                .collect(),
                Some(run_context),
            )
            .map_err(|error| error.to_string())?;
            let previous_message_count = runtime.messages.len();
            AgentKernel::new(&mut *runtime, tools).apply_tool_observation(
                &call,
                &ToolOutcomeStatus::Failed,
                None,
                &observation,
            );
            persist_new_runtime_messages(
                &mut store,
                &runtime.task_id,
                &runtime.messages,
                previous_message_count,
                run_context,
            )
            .map_err(|error| error.to_string())?;
            persist_agent_runtime_snapshot(&mut store, runtime, run_context)?;
            continue;
        }

        let Some(tool) = tool else {
            let observation = observation_from_tool_result(
                &call.tool_name,
                "failed",
                "Unknown tool requested by model.",
            );
            append_tool_finished_event(
                &mut store,
                &runtime.task_id,
                &call.call_id.0,
                &call.tool_name,
                "failed",
                &observation,
                Metadata::new(),
                Some(run_context),
            )
            .map_err(|error| error.to_string())?;
            let previous_message_count = runtime.messages.len();
            AgentKernel::new(&mut *runtime, tools).apply_tool_observation(
                &call,
                &ToolOutcomeStatus::Failed,
                None,
                &observation,
            );
            persist_new_runtime_messages(
                &mut store,
                &runtime.task_id,
                &runtime.messages,
                previous_message_count,
                run_context,
            )
            .map_err(|error| error.to_string())?;
            persist_agent_runtime_snapshot(&mut store, runtime, run_context)?;
            continue;
        };
        let tool_risk = tool.spec().risk;

        if let Some(mut request) = tool.permission_request(&invocation) {
            request.id = PermissionRequestId(unique_id("agent-perm"));
            request
                .metadata
                .insert("phase".to_string(), "16".to_string());
            request
                .metadata
                .entry("tool_input".to_string())
                .or_insert_with(|| invocation.input_json.clone());
            request
                .metadata
                .insert("tool_call_id".to_string(), invocation.id.0.clone());
            request
                .metadata
                .entry("tool_name".to_string())
                .or_insert_with(|| invocation.tool_name.clone());
            request
                .metadata
                .insert("agent_prompt".to_string(), prompt.to_string());
            for (key, value) in run_context {
                request
                    .metadata
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            if !agent_session_permission_granted(&store, &phase16_task_id(), &request, session_id)
                .map_err(|error| error.to_string())?
            {
                store
                    .save_permission_request(request.clone(), current_time_millis())
                    .map_err(|error| error.to_string())?;
                append_event(
                    &mut store,
                    &runtime.task_id,
                    EventKind::PermissionRequested,
                    format!("Agent permission requested for {}", request.action),
                    metadata_with_context(
                        [
                            ("permission_id".to_string(), request.id.0),
                            ("tool_call_id".to_string(), invocation.id.0),
                            ("tool".to_string(), request.action),
                            (
                                "risk".to_string(),
                                permission_risk_label(&request.risk).to_string(),
                            ),
                            ("scope".to_string(), request.scope),
                            ("agent_prompt".to_string(), prompt.to_string()),
                        ]
                        .into_iter()
                        .collect(),
                        run_context,
                    ),
                )
                .map_err(|error| error.to_string())?;
                waiting_for_permission = true;
                continue;
            }
            append_event(
                &mut store,
                &runtime.task_id,
                EventKind::PermissionResolved,
                format!("Session permission reused for {}", request.action),
                metadata_with_context(
                    [
                        ("decision".to_string(), "allow_for_session".to_string()),
                        ("tool_call_id".to_string(), invocation.id.0.clone()),
                        ("tool".to_string(), request.action),
                        ("scope".to_string(), request.scope),
                    ]
                    .into_iter()
                    .collect(),
                    run_context,
                ),
            )
            .map_err(|error| error.to_string())?;
        }

        let tool_name = invocation.tool_name.clone();
        drop(store);
        let result = execute_agent_tool_invocation(
            state,
            registry,
            invocation,
            workspace_root,
            run_context,
        )?;
        let observation = observation_from_agent_tool_result(&tool_name, &result);
        let image_paths = tool_result_image_paths(&result);
        let previous_message_count = runtime.messages.len();
        AgentKernel::new(&mut *runtime, tools).apply_tool_observation(
            &call,
            &result.status,
            Some(&tool_risk),
            &observation,
        );
        append_visual_reference_message(runtime, &tool_name, &image_paths);
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        persist_new_runtime_messages(
            &mut store,
            &runtime.task_id,
            &runtime.messages,
            previous_message_count,
            run_context,
        )
        .map_err(|error| error.to_string())?;
        persist_agent_runtime_snapshot(&mut store, runtime, run_context)?;
        drop(store);
        if cancellation.has_pending_steer() && !agent_run_should_stop(cancellation) {
            return Ok(AgentToolBatchOutcome::RestartAfterSteer);
        }
        if agent_run_should_stop(cancellation) {
            return Ok(paused_agent_tools(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                &*runtime,
                prompt,
                run_context,
                active_collaboration,
                cancellation,
            )?));
        }
    }
    if waiting_for_permission {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let events = agent_events_for_session(&store, &phase16_task_id(), session_id)
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, session_id);
        let task_state = capture_persistable_agent_task_state(runtime);
        let recovery_metadata = agent_recovery_metadata_with_task_state(
            &active_events,
            run_context,
            "blocked",
            "waiting_for_permission",
            Metadata::new(),
            Some(&task_state),
        )?;
        append_event(
            &mut store,
            &runtime.task_id,
            EventKind::TaskStatusChanged,
            "Agent task waiting for permission",
            recovery_metadata,
        )
        .map_err(|error| error.to_string())?;
        remember_suspended_agent_run(
            state,
            SuspendedAgentRun {
                runtime: runtime.clone(),
                prompt: prompt.to_string(),
                run_context: run_context.clone(),
                workspace_root: workspace_root.to_path_buf(),
                collaboration: active_collaboration.cloned(),
                run_control: cancellation.snapshot(),
                last_touched_at_ms: current_time_millis(),
            },
        )?;
        return Ok(paused_agent_tools(
            agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?,
        ));
    }
    Ok(AgentToolBatchOutcome::Continue)
}
