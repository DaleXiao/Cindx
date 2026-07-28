use super::*;

const SUSPENDED_AGENT_RUN_LIMIT: usize = 16;
const SUSPENDED_AGENT_RUN_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

fn purge_expired_suspended_agent_runs(runs: &mut BTreeMap<String, SuspendedAgentRun>, now_ms: u64) {
    runs.retain(|_, run| {
        now_ms.saturating_sub(run.last_touched_at_ms) <= SUSPENDED_AGENT_RUN_TTL_MS
    });
}

pub(crate) fn remember_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    mut run: SuspendedAgentRun,
) -> Result<(), String> {
    let Some(session_id) = run.run_context.get("session_id").cloned() else {
        return Ok(());
    };
    let now_ms = current_time_millis();
    run.last_touched_at_ms = now_ms;
    let mut runs = state
        .suspended_agent_runs
        .lock()
        .map_err(|error| format!("suspended agent runs lock poisoned: {error}"))?;
    purge_expired_suspended_agent_runs(&mut runs, now_ms);
    if !runs.contains_key(&session_id) && runs.len() >= SUSPENDED_AGENT_RUN_LIMIT {
        if let Some(oldest_session_id) = runs
            .iter()
            .min_by_key(|(_, run)| run.last_touched_at_ms)
            .map(|(session_id, _)| session_id.clone())
        {
            runs.remove(&oldest_session_id);
        }
    }
    runs.insert(session_id, run);
    Ok(())
}

pub(crate) fn take_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<SuspendedAgentRun>, String> {
    let now_ms = current_time_millis();
    let mut runs = state
        .suspended_agent_runs
        .lock()
        .map_err(|error| format!("suspended agent runs lock poisoned: {error}"))?;
    purge_expired_suspended_agent_runs(&mut runs, now_ms);
    Ok(runs.remove(session_id))
}

pub(crate) fn suspended_agent_run_control_snapshot(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<RunControlSnapshot>, String> {
    let now_ms = current_time_millis();
    let mut runs = state
        .suspended_agent_runs
        .lock()
        .map_err(|error| format!("suspended agent runs lock poisoned: {error}"))?;
    purge_expired_suspended_agent_runs(&mut runs, now_ms);
    Ok(runs.get(session_id).map(|run| run.run_control.clone()))
}

pub(crate) fn clear_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<(), String> {
    let _ = take_suspended_agent_run(state, session_id)?;
    Ok(())
}

pub(crate) fn clear_suspended_agent_run_for_context(
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
) -> Result<(), String> {
    if let Some(session_id) = run_context.get("session_id") {
        clear_suspended_agent_run(state, session_id)?;
    }
    Ok(())
}

pub(crate) fn append_observations_to_suspended_run(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
    observations: &[ResolvedToolObservation],
) -> Result<(), String> {
    if session_id.is_empty() || observations.is_empty() {
        return Ok(());
    }
    let Some(mut suspended) = take_suspended_agent_run(state, session_id)? else {
        return Ok(());
    };
    for resolved in observations {
        let request = agent_runtime::AgentToolRequest {
            call_id: resolved.call_id.clone(),
            tool_name: resolved.tool_name.clone(),
            input: resolved.input_json.clone(),
        };
        AgentKernel::new(&mut suspended.runtime, &[]).apply_tool_observation(
            &request,
            &resolved.status,
            None,
            &resolved.observation,
        );
        append_visual_reference_message(
            &mut suspended.runtime,
            &resolved.tool_name,
            &resolved.image_paths,
        );
    }
    if let Some(control) = active_agent_run_control(state, Some(session_id))? {
        suspended.run_control = control.snapshot();
    }
    remember_suspended_agent_run(state, suspended)
}
