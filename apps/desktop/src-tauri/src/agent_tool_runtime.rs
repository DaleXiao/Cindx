use super::*;
use crate::agent_runtime_snapshot::persist_runtime_append_and_snapshot;
use crate::agent_runtime_snapshot_cursor::AgentRuntimeSnapshotCursor;
use crate::suspended_run_runtime::{remember_suspended_agent_run, SuspendedAgentRun};

pub(crate) enum AgentToolBatchOutcome {
    Continue,
    RestartAfterSteer,
    Paused(Box<AgentState>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentToolPermissionGateOutcome {
    Pending,
    Reused,
}

pub(super) fn paused_agent_tools(state: AgentState) -> AgentToolBatchOutcome {
    AgentToolBatchOutcome::Paused(Box::new(state))
}

fn evaluate_agent_tool_permission(
    store: &mut SqliteStore,
    runtime_task_id: &TaskId,
    prompt: &str,
    run_context: &Metadata,
    session_id: Option<&str>,
    invocation: &ToolInvocation,
    mut request: PermissionRequest,
) -> Result<AgentToolPermissionGateOutcome, String> {
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

    if !agent_session_permission_granted(store, &phase16_task_id(), &request, session_id)
        .map_err(|error| error.to_string())?
    {
        store
            .save_permission_request(request.clone(), current_time_millis())
            .map_err(|error| error.to_string())?;
        append_event(
            store,
            runtime_task_id,
            EventKind::PermissionRequested,
            format!("Agent permission requested for {}", request.action),
            metadata_with_context(
                [
                    ("permission_id".to_string(), request.id.0),
                    ("tool_call_id".to_string(), invocation.id.0.clone()),
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
        return Ok(AgentToolPermissionGateOutcome::Pending);
    }

    append_event(
        store,
        runtime_task_id,
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
    Ok(AgentToolPermissionGateOutcome::Reused)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn commit_agent_tool_observation(
    state: &tauri::State<'_, AppState>,
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
    cancellation: &Arc<AgentRunControl>,
    epoch_lease: agent_runtime::RunEpochLease,
    tools: &[ToolSpec],
    call: &AgentToolRequest,
    status: &ToolOutcomeStatus,
    risk: Option<&ToolRisk>,
    observation: &str,
    image_paths: &[String],
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<agent_runtime::RunExecutionStepCommit<()>, String> {
    cancellation.commit_execution_step_with(epoch_lease, || {
        let mut transaction = AgentLoopAppendTransaction::begin(runtime);
        let previous_message_count = transaction.original_message_count();
        transaction.with_append_only_mutation(|next_runtime| {
            let verified_interactions_before = next_runtime.verified_interactions;
            AgentKernel::new(next_runtime, tools).apply_tool_observation(
                call,
                status,
                risk,
                observation,
            );
            let postcondition_verified =
                next_runtime.verified_interactions > verified_interactions_before;
            crate::agent_result_evidence::annotate_latest_tool_observation(
                next_runtime,
                tools,
                call,
                status,
                risk,
                epoch_lease.epoch(),
                postcondition_verified,
            );
            append_visual_reference_message(next_runtime, &call.tool_name, image_paths);
        });
        let (prepared_snapshot, next_cursor) = snapshot_cursor.prepare_after_append(
            transaction.state(),
            previous_message_count,
            run_context,
        );
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        persist_runtime_append_and_snapshot(
            &mut store,
            transaction.state(),
            previous_message_count,
            run_context,
            &prepared_snapshot,
        )?;
        transaction.commit();
        *snapshot_cursor = next_cursor;
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn agent_tool_batch_outcome_after_commit(
    commit: agent_runtime::RunExecutionStepCommit<()>,
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    active_collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
) -> Result<Option<AgentToolBatchOutcome>, String> {
    match commit {
        agent_runtime::RunExecutionStepCommit::Committed(()) => Ok(None),
        agent_runtime::RunExecutionStepCommit::RestartAfterSteer
        | agent_runtime::RunExecutionStepCommit::TerminalCommitted => {
            Ok(Some(AgentToolBatchOutcome::RestartAfterSteer))
        }
        agent_runtime::RunExecutionStepCommit::Stopped(_) => {
            Ok(Some(paused_agent_tools(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                runtime,
                prompt,
                run_context,
                active_collaboration,
                cancellation,
            )?)))
        }
    }
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
    epoch_lease: agent_runtime::RunEpochLease,
    registry: &ToolRegistry,
    tools: &[ToolSpec],
    calls: Vec<AgentToolRequest>,
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<AgentToolBatchOutcome, String> {
    if let Some(outcome) =
        crate::agent_parallel_tool_runtime::try_execute_parallel_agent_tool_batch(
            app,
            state,
            workspace_root,
            runtime,
            prompt,
            run_context,
            active_collaboration,
            cancellation,
            epoch_lease,
            registry,
            tools,
            &calls,
            snapshot_cursor,
        )?
    {
        return Ok(outcome);
    }
    execute_agent_tool_batch_serial(
        app,
        state,
        workspace_root,
        runtime,
        prompt,
        run_context,
        active_collaboration,
        cancellation,
        epoch_lease,
        registry,
        tools,
        calls,
        snapshot_cursor,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_agent_tool_batch_serial(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    active_collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    epoch_lease: agent_runtime::RunEpochLease,
    registry: &ToolRegistry,
    tools: &[ToolSpec],
    calls: Vec<AgentToolRequest>,
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<AgentToolBatchOutcome, String> {
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    if !cancellation.execution_epoch_lease_is_current(epoch_lease)
        && !agent_run_should_stop(cancellation)
    {
        return Ok(AgentToolBatchOutcome::RestartAfterSteer);
    }
    let mut waiting_for_permission = false;
    for call in calls {
        if !cancellation.execution_epoch_lease_is_current(epoch_lease)
            && !agent_run_should_stop(cancellation)
        {
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
            drop(store);
            let commit = commit_agent_tool_observation(
                state,
                runtime,
                run_context,
                cancellation,
                epoch_lease,
                tools,
                &call,
                &ToolOutcomeStatus::Failed,
                None,
                &observation,
                &[],
                snapshot_cursor,
            )?;
            if let Some(outcome) = agent_tool_batch_outcome_after_commit(
                commit,
                app,
                state,
                workspace_root,
                runtime,
                prompt,
                run_context,
                active_collaboration,
                cancellation,
            )? {
                return Ok(outcome);
            }
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
            drop(store);
            let commit = commit_agent_tool_observation(
                state,
                runtime,
                run_context,
                cancellation,
                epoch_lease,
                tools,
                &call,
                &ToolOutcomeStatus::Failed,
                None,
                &observation,
                &[],
                snapshot_cursor,
            )?;
            if let Some(outcome) = agent_tool_batch_outcome_after_commit(
                commit,
                app,
                state,
                workspace_root,
                runtime,
                prompt,
                run_context,
                active_collaboration,
                cancellation,
            )? {
                return Ok(outcome);
            }
            continue;
        };
        let tool_risk = tool.spec().risk;

        if let Some(request) = tool.permission_request(&invocation) {
            if evaluate_agent_tool_permission(
                &mut store,
                &runtime.task_id,
                prompt,
                run_context,
                session_id,
                &invocation,
                request,
            )? == AgentToolPermissionGateOutcome::Pending
            {
                waiting_for_permission = true;
                continue;
            }
        }

        let tool_name = invocation.tool_name.clone();
        drop(store);
        let result = match execute_agent_tool_invocation_for_epoch(
            state,
            registry,
            invocation,
            workspace_root,
            run_context,
            cancellation,
            epoch_lease,
        )? {
            AgentToolInvocationOutcome::Completed(result) => result,
            AgentToolInvocationOutcome::RestartAfterSteer => {
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
                return Ok(AgentToolBatchOutcome::RestartAfterSteer);
            }
        };
        let observation = observation_from_agent_tool_result(&tool_name, &result);
        let image_paths = tool_result_image_paths(&result);
        let commit = commit_agent_tool_observation(
            state,
            runtime,
            run_context,
            cancellation,
            epoch_lease,
            tools,
            &call,
            &result.status,
            Some(&tool_risk),
            &observation,
            &image_paths,
            snapshot_cursor,
        )?;
        if let Some(outcome) = agent_tool_batch_outcome_after_commit(
            commit,
            app,
            state,
            workspace_root,
            runtime,
            prompt,
            run_context,
            active_collaboration,
            cancellation,
        )? {
            return Ok(outcome);
        }
    }
    if !cancellation.execution_epoch_lease_is_current(epoch_lease) {
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
        return Ok(AgentToolBatchOutcome::RestartAfterSteer);
    }
    if waiting_for_permission {
        let (task_state, next_cursor) = snapshot_cursor.capture_current(runtime, run_context);
        *snapshot_cursor = next_cursor;
        let suspended = SuspendedAgentRun {
            runtime: runtime.clone(),
            prompt: prompt.to_string(),
            run_context: run_context.clone(),
            workspace_root: workspace_root.to_path_buf(),
            collaboration: active_collaboration.cloned(),
            run_control: cancellation.snapshot(),
            last_touched_at_ms: current_time_millis(),
        };
        let resource_snapshot = cancellation.resource_usage();
        let waiting_commit = cancellation.commit_execution_step_with(epoch_lease, || {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            let events = agent_events_for_session(&store, &phase16_task_id(), session_id)
                .map_err(|error| error.to_string())?;
            let active_events = active_agent_events_for_session(&events, session_id);
            let recovery_metadata = agent_recovery_metadata_with_task_state(
                &active_events,
                run_context,
                "blocked",
                "waiting_for_permission",
                Metadata::new(),
                Some(&task_state),
                Some(&resource_snapshot),
            )?;
            append_event(
                &mut store,
                &runtime.task_id,
                EventKind::TaskStatusChanged,
                "Agent task waiting for permission",
                recovery_metadata,
            )
            .map_err(|error| error.to_string())?;
            remember_suspended_agent_run(state, suspended)?;
            agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
        })?;
        return match waiting_commit {
            agent_runtime::RunExecutionStepCommit::Committed(agent_state) => {
                Ok(paused_agent_tools(agent_state))
            }
            agent_runtime::RunExecutionStepCommit::RestartAfterSteer
            | agent_runtime::RunExecutionStepCommit::TerminalCommitted => {
                Ok(AgentToolBatchOutcome::RestartAfterSteer)
            }
            agent_runtime::RunExecutionStepCommit::Stopped(_) => {
                Ok(paused_agent_tools(pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    &*runtime,
                    prompt,
                    run_context,
                    active_collaboration,
                    cancellation,
                )?))
            }
        };
    }
    Ok(AgentToolBatchOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{
        PermissionDecision, PermissionResolution, PermissionRisk, ToolCallId,
    };

    fn run_context(session_id: &str) -> Metadata {
        [
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
        ]
        .into_iter()
        .collect()
    }

    fn invocation() -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("call-a".to_string()),
            task_id: phase16_task_id(),
            tool_name: "file.write".to_string(),
            input_json: r#"{"path":"notes.md","content":"safe"}"#.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        }
    }

    fn permission_request(invocation: &ToolInvocation) -> PermissionRequest {
        PermissionRequest {
            id: PermissionRequestId(String::new()),
            task_id: invocation.task_id.clone(),
            risk: PermissionRisk::Write,
            action: invocation.tool_name.clone(),
            reason: "Write the requested file".to_string(),
            scope: "notes.md".to_string(),
            metadata: Metadata::new(),
        }
    }

    #[test]
    fn production_permission_gate_blocks_then_reuses_only_the_resolved_session_capability() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let invocation = invocation();
        let context = run_context("session-a");

        let first = evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            "write notes",
            &context,
            Some("session-a"),
            &invocation,
            permission_request(&invocation),
        )
        .expect("permission gate should persist a pending request");
        assert_eq!(first, AgentToolPermissionGateOutcome::Pending);

        let pending = pending_agent_permissions_for_run(
            &store,
            &phase16_task_id(),
            Some("session-a"),
            Some("run-a"),
        )
        .expect("pending permission should be queryable");
        assert_eq!(pending.len(), 1);
        store
            .resolve_permission(PermissionResolution {
                request_id: pending[0].id.clone(),
                decision: PermissionDecision::AllowForSession,
                resolved_at_ms: current_time_millis(),
                resolved_by: "test".to_string(),
            })
            .expect("permission should resolve");

        let reused = evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            "write notes",
            &context,
            Some("session-a"),
            &invocation,
            permission_request(&invocation),
        )
        .expect("the exact resolved capability should be reusable");
        assert_eq!(reused, AgentToolPermissionGateOutcome::Reused);

        let other_run_context = run_context("session-b");
        let other_session = evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            "write notes",
            &other_run_context,
            Some("session-b"),
            &invocation,
            permission_request(&invocation),
        )
        .expect("another session should receive its own pending request");
        assert_eq!(other_session, AgentToolPermissionGateOutcome::Pending);
    }
}
