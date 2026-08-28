use super::*;
use crate::agent_runtime_snapshot::persist_runtime_append_and_snapshot;
use crate::agent_runtime_snapshot_cursor::AgentRuntimeSnapshotCursor;
use crate::suspended_run_runtime::{remember_suspended_agent_run, SuspendedAgentRun};
use agent_application::{insert_run_objectives, merge_persistable_run_context};

pub(crate) enum AgentToolBatchOutcome {
    Continue,
    RestartAfterSteer,
    Paused(Box<AgentState>),
}

fn agent_tool_batch_contains_active_denial(
    runtime: &mut agent_runtime::AgentLoopState,
    registry: &ToolRegistry,
    tools: &[ToolSpec],
    calls: &[AgentToolRequest],
) -> bool {
    calls.iter().any(|call| {
        let risk = registry.get(&call.tool_name).map(|tool| tool.spec().risk);
        AgentKernel::new(&mut *runtime, tools)
            .action_denial_for_invocation(call, risk.as_ref())
            .is_some()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentToolPermissionGateOutcome {
    Pending,
    Reused,
}

pub(super) struct AgentToolObservationCommit {
    goal_delta: Option<agent_runtime::AgentGoalDelta>,
    continuation: Option<AgentToolContinuation>,
}

struct AgentToolContinuation {
    scope: String,
    tool_name: String,
    input: String,
    effect_semantics: agent_core::ToolEffectSemantics,
    evidence_complete: Option<bool>,
}

fn pending_permission_matches_exact_invocation(
    pending: &PermissionRequest,
    candidate: &PermissionRequest,
) -> bool {
    const IDENTITY_KEYS: [&str; 9] = [
        "tool_call_id",
        "tool_name",
        "tool_input_fingerprint",
        "project_id",
        "session_id",
        "agent_run_id",
        "collaboration_id",
        "steer_epoch",
        "prompt_contract_epoch",
    ];

    pending.task_id == candidate.task_id
        && pending.action == candidate.action
        && pending.risk == candidate.risk
        && pending.scope == candidate.scope
        && IDENTITY_KEYS.iter().all(|key| {
            pending.metadata.get(*key).map(String::as_str)
                == candidate.metadata.get(*key).map(String::as_str)
        })
}

fn runtime_has_tool_observation(runtime: &agent_runtime::AgentLoopState, call_id: &str) -> bool {
    runtime.messages.iter().any(|message| {
        message.role == MessageRole::Tool
            && message.metadata.get("tool_call_id").map(String::as_str) == Some(call_id)
    })
}

pub(super) fn paused_agent_tools(state: AgentState) -> AgentToolBatchOutcome {
    AgentToolBatchOutcome::Paused(Box::new(state))
}

fn evaluate_agent_tool_permission(
    store: &mut SqliteStore,
    runtime_task_id: &TaskId,
    run_context: &Metadata,
    session_id: Option<&str>,
    invocation: &ToolInvocation,
    mut request: PermissionRequest,
) -> Result<AgentToolPermissionGateOutcome, String> {
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
    request.metadata.insert(
        "tool_input_fingerprint".to_string(),
        tool_input_fingerprint(&invocation.tool_name, &invocation.input_json),
    );
    request.metadata = merge_persistable_run_context(request.metadata, run_context);
    insert_run_objectives(&mut request.metadata, run_context);

    if !agent_session_permission_granted(store, &phase16_task_id(), &request, session_id)
        .map_err(|error| error.to_string())?
    {
        let pending = pending_agent_permissions_for_run(
            store,
            &request.task_id,
            session_id,
            request.metadata.get("agent_run_id").map(String::as_str),
        )
        .map_err(|error| error.to_string())?;
        if pending
            .iter()
            .any(|pending| pending_permission_matches_exact_invocation(pending, &request))
        {
            return Ok(AgentToolPermissionGateOutcome::Pending);
        }

        request.id = PermissionRequestId(unique_id("agent-perm"));
        store
            .save_permission_request(request.clone(), current_time_millis())
            .map_err(|error| error.to_string())?;
        let mut permission_metadata = [
            ("permission_id".to_string(), request.id.0.clone()),
            ("tool_call_id".to_string(), invocation.id.0.clone()),
            ("tool".to_string(), request.action.clone()),
            (
                "risk".to_string(),
                permission_risk_label(&request.risk).to_string(),
            ),
            ("scope".to_string(), request.scope.clone()),
        ]
        .into_iter()
        .collect();
        insert_run_objectives(&mut permission_metadata, run_context);
        append_event(
            store,
            runtime_task_id,
            EventKind::PermissionRequested,
            format!("Agent permission requested for {}", request.action),
            metadata_with_context(permission_metadata, run_context),
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
    effect_spec: Option<&ToolSpec>,
    postcondition_evidence: Option<&agent_core::ToolPostconditionEvidence>,
    observation: &str,
    denial: Option<&agent_runtime::AgentActionDenialFeedback>,
    tool_result: Option<&ToolResult>,
    image_paths: &[String],
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<agent_runtime::RunExecutionStepCommit<AgentToolObservationCommit>, String> {
    let postcondition_scope = agent_runtime::postcondition_lineage_scope(run_context);
    let continuation = effect_spec.map(|spec| AgentToolContinuation {
        scope: run_context
            .get("stage")
            .or_else(|| run_context.get("collaboration_stage"))
            .map(String::as_str)
            .unwrap_or("executor")
            .to_string(),
        tool_name: call.tool_name.clone(),
        input: call.input.clone(),
        effect_semantics: spec.effect_semantics.clone(),
        evidence_complete: tool_result
            .filter(|result| matches!(result.status, ToolOutcomeStatus::Succeeded))
            .and_then(|result| result.model_observation.as_ref())
            .filter(|observation| observation.schema == agent_core::TOOL_OBSERVATION_V2_SCHEMA)
            .map(|observation| observation.evidence_complete),
    });
    cancellation.commit_execution_step_with(epoch_lease, || {
        let mut transaction = AgentLoopAppendTransaction::begin(runtime);
        let previous_message_count = transaction.original_message_count();
        let goal_delta = transaction.with_append_only_mutation(|next_runtime| {
            let mut kernel = AgentKernel::new(next_runtime, tools)
                .with_postcondition_scope(postcondition_scope.as_deref());
            let transition = match tool_result {
                Some(result) => kernel.apply_tool_result_transition_with_contract(
                    call,
                    risk,
                    effect_spec,
                    postcondition_evidence,
                    result,
                    observation,
                    denial,
                ),
                None => kernel.apply_tool_observation_transition_with_contract(
                    call,
                    status,
                    risk,
                    effect_spec,
                    postcondition_evidence,
                    observation,
                    denial,
                ),
            };
            crate::agent_result_evidence::annotate_latest_tool_observation(
                next_runtime,
                tools,
                call,
                effect_spec,
                status,
                risk,
                epoch_lease.epoch(),
                transition.postcondition_verification.as_ref(),
                transition.goal_delta.as_ref(),
            );
            append_visual_reference_message(next_runtime, &call.tool_name, image_paths);
            transition.goal_delta
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
        Ok(AgentToolObservationCommit {
            goal_delta,
            continuation,
        })
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn agent_tool_batch_outcome_after_commit(
    commit: agent_runtime::RunExecutionStepCommit<AgentToolObservationCommit>,
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    active_collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    epoch_lease: agent_runtime::RunEpochLease,
) -> Result<Option<AgentToolBatchOutcome>, String> {
    match commit {
        agent_runtime::RunExecutionStepCommit::Committed(committed) => {
            if let Some(delta) = committed.goal_delta.as_ref() {
                cancellation.record_goal_delta_at(epoch_lease.epoch(), delta);
            }
            if let Some(continuation) = committed.continuation {
                cancellation.record_tool_continuation_at(
                    epoch_lease.epoch(),
                    &continuation.scope,
                    &continuation.tool_name,
                    &continuation.input,
                    &continuation.effect_semantics,
                    continuation.evidence_complete,
                );
            }
            Ok(None)
        }
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
    let contains_active_denial =
        agent_tool_batch_contains_active_denial(runtime, registry, tools, &calls);
    if !contains_active_denial {
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
        for (key, value) in run_context {
            invocation
                .metadata
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        let tool = registry.get(&call.tool_name);
        let effect_spec = tool.map(|tool| tool.effect_spec(&invocation));
        if let Some(effect_spec) = effect_spec.as_ref() {
            agent_runtime::apply_tool_spec_runtime_metadata(&mut invocation, effect_spec);
        }
        let permission_request = tool.and_then(|tool| tool.permission_request(&invocation));
        let tool_risk = effect_spec.as_ref().map(|spec| spec.risk.clone());
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let completed_result = if permission_request.is_some() {
            completed_exact_tool_result(&store, &invocation).map_err(|error| error.to_string())?
        } else {
            None
        };
        if completed_result.is_some() && runtime_has_tool_observation(runtime, &call.call_id.0) {
            continue;
        }
        if completed_result.is_none() {
            append_tool_proposed_event(&mut store, &invocation, Some(run_context))
                .map_err(|error| error.to_string())?;
        }

        if completed_result.is_none() {
            let prior_denial = AgentKernel::new(&mut *runtime, tools)
                .action_denial_for_invocation(&call, tool_risk.as_ref());
            if let Some(denial) = prior_denial {
                let observation = observation_from_tool_result(
                    &call.tool_name,
                    "denied",
                    "Cindx blocked this action because the current objective already contains a trusted denial. Report the blocker or wait for new user guidance.",
                );
                append_tool_finished_event(
                    &mut store,
                    &runtime.task_id,
                    &call.call_id.0,
                    &call.tool_name,
                    "denied",
                    &observation,
                    [("failure_code".to_string(), denial.code.to_string())]
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
                    &ToolOutcomeStatus::Denied,
                    tool_risk.as_ref(),
                    effect_spec.as_ref(),
                    None,
                    &observation,
                    Some(&denial),
                    None,
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
                    epoch_lease,
                )? {
                    return Ok(outcome);
                }
                continue;
            }
        }

        if completed_result.is_none()
            && AgentKernel::new(&mut *runtime, tools).repeated_tool_failure_count(&call)
                >= MAX_IDENTICAL_TOOL_FAILURES
        {
            let denial = agent_runtime::AgentActionDenialFeedback::runtime_policy(
                "repeated_tool_failure",
                agent_runtime::AgentActionRecovery::Replan,
            );
            let observation = observation_from_tool_result(
                &call.tool_name,
                "denied",
                "Cindx blocked this identical tool call after repeated failures. Change the arguments once or report the blocker.",
            );
            append_tool_finished_event(
                &mut store,
                &runtime.task_id,
                &call.call_id.0,
                &call.tool_name,
                "denied",
                &observation,
                [("failure_code".to_string(), denial.code.to_string())]
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
                &ToolOutcomeStatus::Denied,
                tool_risk.as_ref(),
                effect_spec.as_ref(),
                None,
                &observation,
                Some(&denial),
                None,
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
                epoch_lease,
            )? {
                return Ok(outcome);
            }
            continue;
        }

        let Some(tool) = tool else {
            let denial = agent_runtime::AgentActionDenialFeedback::capability_unavailable(
                "tool_capability_unavailable",
            );
            let observation = observation_from_tool_result(
                &call.tool_name,
                "denied",
                "Unknown tool requested by model.",
            );
            append_tool_finished_event(
                &mut store,
                &runtime.task_id,
                &call.call_id.0,
                &call.tool_name,
                "denied",
                &observation,
                [("failure_code".to_string(), denial.code.to_string())]
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
                &ToolOutcomeStatus::Denied,
                None,
                None,
                None,
                &observation,
                Some(&denial),
                None,
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
                epoch_lease,
            )? {
                return Ok(outcome);
            }
            continue;
        };
        let tool_risk = tool_risk.expect("registered tool has a risk classification");

        if completed_result.is_none() {
            if let Some(request) = permission_request {
                let guardian_request = request.clone();
                let gate = evaluate_agent_tool_permission(
                    &mut store,
                    &runtime.task_id,
                    run_context,
                    session_id,
                    &invocation,
                    request,
                )?;
                if gate == AgentToolPermissionGateOutcome::Pending {
                    // Guardian auto-approval (default off): before pausing for
                    // the user, a model-distinct reviewer may explicitly allow
                    // the pending request. Deny, timeout, malformed answers,
                    // unavailable reviewers, and destructive risk all fall
                    // back to the ordinary user prompt (fail-closed). The
                    // store lock is released across the bounded review call.
                    drop(store);
                    let context_excerpt =
                        crate::guardian_runtime::guardian_context_excerpt(&runtime.messages);
                    let guardian_approved =
                        crate::guardian_runtime::guardian_auto_approve_pending_permission(
                            app,
                            state,
                            run_context,
                            &guardian_request,
                            prompt,
                            &context_excerpt,
                        )?;
                    store = state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    if !guardian_approved {
                        waiting_for_permission = true;
                        continue;
                    }
                }
            }
        }

        let tool_name = invocation.tool_name.clone();
        let verification_invocation = invocation.clone();
        drop(store);
        let result = if let Some(result) = completed_result {
            result
        } else {
            match execute_agent_tool_invocation_for_epoch(
                state,
                registry,
                invocation,
                workspace_root,
                run_context,
                cancellation,
                epoch_lease,
            )? {
                AgentToolInvocationOutcome::Completed(result) => *result,
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
            }
        };
        let postcondition_evidence = tool.postcondition_evidence(&verification_invocation, &result);
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
            effect_spec.as_ref(),
            postcondition_evidence.as_ref(),
            &observation,
            None,
            Some(&result),
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
            epoch_lease,
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
                AgentRecoveryState::Blocked,
                AgentRecoveryReason::WaitingForPermission,
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
    use agent_core::{PermissionDecision, PermissionResolution, PermissionRisk, ToolCallId};

    fn run_context_for(
        session_id: &str,
        agent_run_id: &str,
        prompt_contract_epoch: u64,
    ) -> Metadata {
        [
            ("project_id".to_string(), "project-a".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), agent_run_id.to_string()),
            ("steer_epoch".to_string(), prompt_contract_epoch.to_string()),
            (
                "prompt_contract_epoch".to_string(),
                prompt_contract_epoch.to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    fn run_context(session_id: &str) -> Metadata {
        run_context_for(session_id, "run-a", 0)
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

    fn pending_match_candidate(
        invocation: &ToolInvocation,
        context: &Metadata,
    ) -> PermissionRequest {
        let mut request = permission_request(invocation);
        request
            .metadata
            .insert("tool_call_id".to_string(), invocation.id.0.clone());
        request
            .metadata
            .insert("tool_name".to_string(), invocation.tool_name.clone());
        request.metadata.insert(
            "tool_input_fingerprint".to_string(),
            tool_input_fingerprint(&invocation.tool_name, &invocation.input_json),
        );
        request.metadata.extend(context.clone());
        request
    }

    #[test]
    fn exact_replay_does_not_reapply_an_observation_already_in_the_runtime() {
        let mut runtime = start_agent_loop(
            TaskId("replay-observation".to_string()),
            "write notes",
            AgentRuntimeConfig::default(),
        );
        runtime.messages.push(Message {
            role: MessageRole::Tool,
            content: "written".to_string(),
            metadata: [("tool_call_id".to_string(), "call-a".to_string())]
                .into_iter()
                .collect(),
        });

        assert!(runtime_has_tool_observation(&runtime, "call-a"));
        assert!(!runtime_has_tool_observation(&runtime, "call-b"));
    }

    #[test]
    fn active_denial_forces_a_read_batch_out_of_the_parallel_fast_path() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let registry = ToolRegistry::with_workspace_tools(workspace.path());
        let tools = registry.specs();
        let mut runtime = start_agent_loop(
            TaskId("parallel-denial".to_string()),
            "read README.md",
            AgentRuntimeConfig::default(),
        );
        let call = AgentToolRequest {
            call_id: ToolCallId("read-denied".to_string()),
            tool_name: "file.read".to_string(),
            input: r#"{"path":"README.md"}"#.to_string(),
        };
        runtime.task_contract.record_action_denial(
            &call.tool_name,
            &tool_input_fingerprint(&call.tool_name, &call.input),
            &agent_runtime::AgentActionDenialFeedback::runtime_policy(
                "read_policy_denied",
                agent_runtime::AgentActionRecovery::FinalizeBlocked,
            ),
        );

        assert!(agent_tool_batch_contains_active_denial(
            &mut runtime,
            &registry,
            &tools,
            std::slice::from_ref(&call),
        ));
    }

    #[test]
    fn pending_permission_match_requires_exact_capability_and_invocation_identity() {
        let invocation = invocation();
        let candidate = pending_match_candidate(&invocation, &run_context("session-a"));
        assert!(pending_permission_matches_exact_invocation(
            &candidate, &candidate
        ));

        let mut changed = candidate.clone();
        changed.action = "file.delete".to_string();
        assert!(!pending_permission_matches_exact_invocation(
            &changed, &candidate
        ));
        let mut changed = candidate.clone();
        changed.risk = PermissionRisk::Destructive;
        assert!(!pending_permission_matches_exact_invocation(
            &changed, &candidate
        ));
        let mut changed = candidate.clone();
        changed.scope = "other.md".to_string();
        assert!(!pending_permission_matches_exact_invocation(
            &changed, &candidate
        ));
        for key in [
            "tool_call_id",
            "tool_name",
            "tool_input_fingerprint",
            "project_id",
            "session_id",
            "agent_run_id",
            "collaboration_id",
            "steer_epoch",
            "prompt_contract_epoch",
        ] {
            let mut changed = candidate.clone();
            changed
                .metadata
                .insert(key.to_string(), "other".to_string());
            assert!(
                !pending_permission_matches_exact_invocation(&changed, &candidate),
                "{key} must be part of the exact pending identity"
            );
        }
    }

    #[test]
    fn pending_permission_gate_coalesces_only_exact_canonical_call_and_lineage() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let invocation = invocation();
        let context = run_context("session-a");

        for input_json in [
            r#"{"path":"notes.md","content":"safe"}"#,
            r#"{"content":"safe","path":"notes.md"}"#,
        ] {
            let mut equivalent = invocation.clone();
            equivalent.input_json = input_json.to_string();
            assert_eq!(
                evaluate_agent_tool_permission(
                    &mut store,
                    &phase16_task_id(),
                    &context,
                    Some("session-a"),
                    &equivalent,
                    permission_request(&equivalent),
                )
                .expect("equivalent request should reach the pending gate"),
                AgentToolPermissionGateOutcome::Pending
            );
        }

        let mut different_call = invocation.clone();
        different_call.id = ToolCallId("call-b".to_string());
        evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            &context,
            Some("session-a"),
            &different_call,
            permission_request(&different_call),
        )
        .expect("a different call id should remain independently permissioned");

        let next_epoch = run_context_for("session-a", "run-a", 1);
        evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            &next_epoch,
            Some("session-a"),
            &invocation,
            permission_request(&invocation),
        )
        .expect("a different prompt contract epoch should remain independent");

        let next_run = run_context_for("session-a", "run-b", 0);
        evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            &next_run,
            Some("session-a"),
            &invocation,
            permission_request(&invocation),
        )
        .expect("a different run should remain independent");

        let next_session = run_context_for("session-b", "run-a", 0);
        evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            &next_session,
            Some("session-b"),
            &invocation,
            permission_request(&invocation),
        )
        .expect("a different session should remain independent");

        let events = store
            .list_by_task(&phase16_task_id())
            .expect("permission events should be readable");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::PermissionRequested)
                .count(),
            5,
            "the canonical duplicate must not create a sixth permission event"
        );
        assert_eq!(
            pending_agent_permissions_for_run(
                &store,
                &phase16_task_id(),
                Some("session-a"),
                Some("run-a")
            )
            .expect("run-a permissions should be queryable")
            .len(),
            3
        );
    }

    #[test]
    fn production_permission_gate_blocks_then_reuses_only_the_resolved_session_capability() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let invocation = invocation();
        let context = run_context("session-a");

        let first = evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
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
            &other_run_context,
            Some("session-b"),
            &invocation,
            permission_request(&invocation),
        )
        .expect("another session should receive its own pending request");
        assert_eq!(other_session, AgentToolPermissionGateOutcome::Pending);
    }
}
