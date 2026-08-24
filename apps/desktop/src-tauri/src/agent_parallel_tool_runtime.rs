use super::*;
use crate::agent_runtime_snapshot_cursor::AgentRuntimeSnapshotCursor;
use crate::agent_tool_runtime::{
    agent_tool_batch_outcome_after_commit, commit_agent_tool_observation, paused_agent_tools,
    AgentToolBatchOutcome,
};

#[derive(Clone)]
struct PreparedParallelToolCall {
    call: AgentToolRequest,
    invocation: ToolInvocation,
    risk: ToolRisk,
    effect_spec: ToolSpec,
    input_fingerprint: String,
}

struct ParallelToolCandidate {
    call: AgentToolRequest,
    invocation: ToolInvocation,
    input_fingerprint: String,
}

struct ParallelToolContract {
    effect_spec: ToolSpec,
}

struct ParallelToolBodyResult {
    result: ToolResult,
    elapsed: Duration,
}

struct ParallelToolReservationGuard {
    control: Arc<AgentRunControl>,
    epoch: u64,
    remaining: usize,
}

impl ParallelToolReservationGuard {
    fn new(control: &Arc<AgentRunControl>, epoch: u64, remaining: usize) -> Self {
        Self {
            control: Arc::clone(control),
            epoch,
            remaining,
        }
    }

    fn finish_one(&mut self) {
        if self.remaining > 0 {
            self.control.finish_tool_call_at(self.epoch);
            self.remaining -= 1;
        }
    }
}

impl Drop for ParallelToolReservationGuard {
    fn drop(&mut self) {
        while self.remaining > 0 {
            self.control.finish_tool_call_at(self.epoch);
            self.remaining -= 1;
        }
    }
}

fn tool_spec_allows_independent_read(spec: &ToolSpec) -> bool {
    matches!(
        spec.execution_concurrency,
        agent_core::ToolExecutionConcurrency::IndependentRead
    ) && matches!(spec.risk, ToolRisk::ReadOnly)
        && matches!(
            spec.effect_semantics,
            agent_core::ToolEffectSemantics::ReadOnly
        )
        && matches!(spec.source, agent_core::ToolSource::BuiltIn)
}

fn parallel_tool_batch_contracts<'a>(
    registry: &ToolRegistry,
    invocation_count: usize,
    invocations: impl IntoIterator<Item = &'a ToolInvocation>,
) -> Option<Vec<ParallelToolContract>> {
    if !(2..=crate::parallel_execution::MAX_GLOBAL_TOOL_WORKERS).contains(&invocation_count) {
        return None;
    }
    invocations
        .into_iter()
        .map(|invocation| {
            let tool = registry.get(&invocation.tool_name)?;
            let effect_spec = tool.effect_spec(invocation);
            if !tool_spec_allows_independent_read(&effect_spec)
                || tool.permission_request(invocation).is_some()
            {
                return None;
            }
            Some(ParallelToolContract { effect_spec })
        })
        .collect()
}

fn prepare_parallel_tool_batch(
    state: &tauri::State<'_, AppState>,
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
    workspace_root: &Path,
    registry: &ToolRegistry,
    tools: &[ToolSpec],
    calls: &[AgentToolRequest],
) -> Result<Option<Vec<PreparedParallelToolCall>>, String> {
    if !(2..=crate::parallel_execution::MAX_GLOBAL_TOOL_WORKERS).contains(&calls.len()) {
        return Ok(None);
    }

    let mut call_ids = BTreeSet::new();
    let mut signatures = BTreeSet::new();
    let mut candidates = Vec::with_capacity(calls.len());
    for call in calls {
        if !call_ids.insert(call.call_id.0.clone())
            || !signatures.insert((call.tool_name.clone(), call.input.clone()))
            || AgentKernel::new(&mut *runtime, tools).repeated_tool_failure_count(call)
                >= MAX_IDENTICAL_TOOL_FAILURES
        {
            return Ok(None);
        }
        let mut invocation = AgentKernel::new(&mut *runtime, tools).tool_invocation(call);
        for (key, value) in run_context {
            invocation
                .metadata
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        let input_fingerprint =
            tool_input_fingerprint(&invocation.tool_name, &invocation.input_json);
        candidates.push(ParallelToolCandidate {
            call: call.clone(),
            invocation,
            input_fingerprint,
        });
    }
    let Some(contracts) = parallel_tool_batch_contracts(
        registry,
        candidates.len(),
        candidates.iter().map(|item| &item.invocation),
    ) else {
        return Ok(None);
    };
    let prepared = candidates
        .into_iter()
        .zip(contracts)
        .map(|(candidate, contract)| {
            let mut invocation = candidate.invocation;
            agent_runtime::apply_tool_spec_runtime_metadata(&mut invocation, &contract.effect_spec);
            PreparedParallelToolCall {
                call: candidate.call,
                invocation,
                risk: contract.effect_spec.risk.clone(),
                effect_spec: contract.effect_spec,
                input_fingerprint: candidate.input_fingerprint,
            }
        })
        .collect::<Vec<_>>();

    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    for item in &prepared {
        match completed_tool_result(&store, &item.invocation, workspace_root) {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => return Ok(None),
        }
    }
    Ok(Some(prepared))
}

/// Certifies that every prepared call is a permissionless read-only tool
/// invocation, which is the exact admission contract the run-control batch
/// reservation relaxes for. The stricter parallel batch contract already
/// implies it; this re-check keeps the flag honest if the contract widens.
fn parallel_batch_is_permissionless_read_only(
    registry: &ToolRegistry,
    prepared: &[PreparedParallelToolCall],
) -> bool {
    prepared
        .iter()
        .all(|item| registry.permissionless_read_tool(&item.invocation).is_ok())
}

fn run_parallel_tool_bodies(
    registry: &ToolRegistry,
    cancellation: &Arc<AgentRunControl>,
    epoch_lease: agent_runtime::RunEpochLease,
    prepared: &[PreparedParallelToolCall],
) -> Vec<ParallelToolBodyResult> {
    let jobs = prepared
        .iter()
        .map(|item| {
            let registry = registry.clone();
            let invocation = item.invocation.clone();
            let tool_call_id = invocation.id.clone();
            let tool_name = invocation.tool_name.clone();
            let cancellation = Arc::clone(cancellation);
            Box::new(move || {
                let started_at = Instant::now();
                let tool_control = ToolExecutionControl::new({
                    let cancellation = Arc::clone(&cancellation);
                    move || {
                        cancellation.should_stop()
                            || !cancellation.execution_epoch_lease_is_current(epoch_lease)
                    }
                });
                let result = match registry.get(&tool_name) {
                    Some(tool) => tool
                        .execute_with_control(invocation, &tool_control)
                        .unwrap_or_else(|error| failed_tool_result(tool_call_id, error)),
                    None => ToolResult::failed(tool_call_id, "unknown tool"),
                };
                ParallelToolBodyResult {
                    result,
                    elapsed: started_at.elapsed(),
                }
            }) as agent_runtime::ParallelJob<ParallelToolBodyResult>
        })
        .collect();

    crate::parallel_execution::run_tool_jobs_ordered(jobs)
        .into_iter()
        .zip(prepared)
        .map(|(result, item)| match result {
            Ok(result) => result,
            Err(error) => ParallelToolBodyResult {
                result: ToolResult::failed(
                    item.invocation.id.clone(),
                    format!("tool worker failed: {error}"),
                ),
                elapsed: Duration::ZERO,
            },
        })
        .collect()
}

fn append_parallel_tool_started_events(
    store: &mut SqliteStore,
    prepared: &[PreparedParallelToolCall],
    run_context: &Metadata,
) -> Result<(), StorageError> {
    for item in prepared {
        append_tool_proposed_event(store, &item.invocation, Some(run_context))?;
        append_event(
            store,
            &item.invocation.task_id,
            EventKind::ToolCallStarted,
            format!("Tool call started: {}", item.invocation.tool_name),
            metadata_with_context(
                tool_invocation_event_metadata(&item.invocation),
                run_context,
            ),
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_prepared_parallel_tool_batch(
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
    prepared: Vec<PreparedParallelToolCall>,
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<Option<AgentToolBatchOutcome>, String> {
    let scope = run_context
        .get("stage")
        .or_else(|| run_context.get("collaboration_stage"))
        .map(String::as_str)
        .unwrap_or("executor");
    let admission = prepared
        .iter()
        .map(|item| {
            (
                item.invocation.tool_name.as_str(),
                item.invocation.input_json.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let permissionless_read_only = parallel_batch_is_permissionless_read_only(registry, &prepared);
    match cancellation.begin_tool_call_batch_with_epoch(
        epoch_lease,
        scope,
        &admission,
        permissionless_read_only,
    ) {
        agent_runtime::RunToolCallBatchStart::Started { .. } => {}
        agent_runtime::RunToolCallBatchStart::SerialRequired => return Ok(None),
        agent_runtime::RunToolCallBatchStart::RestartAfterSteer
        | agent_runtime::RunToolCallBatchStart::TerminalCommitted => {
            return Ok(Some(AgentToolBatchOutcome::RestartAfterSteer));
        }
        agent_runtime::RunToolCallBatchStart::Stopped(_) => {
            return Ok(Some(paused_agent_tools(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                &*runtime,
                prompt,
                run_context,
                active_collaboration,
                cancellation,
            )?)));
        }
    }

    let mut reservations =
        ParallelToolReservationGuard::new(cancellation, epoch_lease.epoch(), prepared.len());
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_parallel_tool_started_events(&mut store, &prepared, run_context)
            .map_err(|error| error.to_string())?;
    }

    let body_results = run_parallel_tool_bodies(registry, cancellation, epoch_lease, &prepared);
    let mut results = Vec::with_capacity(prepared.len());
    for (item, body) in prepared.iter().zip(body_results) {
        reservations.finish_one();
        cancellation.mark_progress_at(
            epoch_lease.epoch(),
            "tool_result",
            &item.invocation.tool_name,
        );
        let mut result = body.result;
        let progress = cancellation.progress();
        result.metadata.insert(
            "run_checkpoints".to_string(),
            progress.checkpoints.to_string(),
        );
        result.metadata.insert(
            "run_observations".to_string(),
            progress.observations.to_string(),
        );
        result.metadata.insert(
            "run_budget_extensions".to_string(),
            progress.budget_extensions.to_string(),
        );
        materialize_tool_result_artifacts(&mut result, workspace_root)?;
        finalize_tool_result(
            &mut result,
            &item.invocation.id,
            &item.input_fingerprint,
            body.elapsed,
        );
        if !cancellation.execution_epoch_lease_is_current(epoch_lease) {
            result
                .metadata
                .insert("superseded_by_steer".to_string(), "true".to_string());
        }
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_tool_finished_event(
                &mut store,
                &item.invocation.task_id,
                &item.invocation.id.0,
                &item.invocation.tool_name,
                tool_outcome_label(&result.status),
                &result.output,
                result.metadata.clone(),
                Some(run_context),
            )
            .map_err(|error| error.to_string())?;
        }
        results.push(result);
    }
    drop(reservations);

    if !cancellation.execution_epoch_lease_is_current(epoch_lease) {
        if agent_run_should_stop(cancellation) {
            return Ok(Some(paused_agent_tools(pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                &*runtime,
                prompt,
                run_context,
                active_collaboration,
                cancellation,
            )?)));
        }
        return Ok(Some(AgentToolBatchOutcome::RestartAfterSteer));
    }

    for (item, result) in prepared.iter().zip(results) {
        let postcondition_evidence = registry
            .get(&item.invocation.tool_name)
            .and_then(|tool| tool.postcondition_evidence(&item.invocation, &result));
        let observation = observation_from_agent_tool_result(&item.invocation.tool_name, &result);
        let image_paths = tool_result_image_paths(&result);
        let commit = commit_agent_tool_observation(
            state,
            runtime,
            run_context,
            cancellation,
            epoch_lease,
            tools,
            &item.call,
            &result.status,
            Some(&item.risk),
            Some(&item.effect_spec),
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
            return Ok(Some(outcome));
        }
    }
    Ok(Some(AgentToolBatchOutcome::Continue))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_execute_parallel_agent_tool_batch(
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
    calls: &[AgentToolRequest],
    snapshot_cursor: &mut AgentRuntimeSnapshotCursor,
) -> Result<Option<AgentToolBatchOutcome>, String> {
    let Some(prepared) = prepare_parallel_tool_batch(
        state,
        runtime,
        run_context,
        workspace_root,
        registry,
        tools,
        calls,
    )?
    else {
        return Ok(None);
    };
    execute_prepared_parallel_tool_batch(
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
        prepared,
        snapshot_cursor,
    )
}

#[cfg(test)]
#[path = "agent_parallel_tool_runtime_tests.rs"]
mod tests;
