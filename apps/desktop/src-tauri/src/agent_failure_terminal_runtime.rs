use crate::{
    agent_loop_runtime::pause_agent_loop_for_control_stop,
    agent_query_commands::emit_agent_stream_delta,
    agent_read_model::{agent_state_for_session, agent_state_with_error_metadata_in_context},
    app_state::AppState,
    collaboration_service::AgentCollaboration,
    suspended_run_runtime::clear_suspended_agent_run_for_context,
    view_models::AgentState,
};
use agent_core::Metadata;
use agent_runtime::{
    AgentFailure, AgentLoopState, AgentRunControl, RunEpochLease, RunTerminalCommit,
};
use std::{path::Path, sync::Arc};

#[allow(clippy::large_enum_variant)]
pub(crate) enum AgentFailureTerminalOutcome {
    Committed(AgentState),
    RestartAfterSteer,
    Stopped,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum AgentFailureLoopOutcome {
    Finished(AgentState),
    RestartAfterSteer,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_loop_failure(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    epoch_lease: RunEpochLease,
    failure: &AgentFailure,
    request_id: &str,
    session_id: Option<&str>,
    streamed_output: bool,
) -> Result<AgentFailureLoopOutcome, String> {
    match commit_agent_failure_terminal(
        state,
        runtime,
        run_context,
        cancellation,
        epoch_lease,
        failure,
        failure.message.clone(),
    )? {
        AgentFailureTerminalOutcome::Committed(agent_state) => {
            Ok(AgentFailureLoopOutcome::Finished(agent_state))
        }
        AgentFailureTerminalOutcome::RestartAfterSteer => {
            if streamed_output {
                emit_agent_stream_delta(app, request_id, session_id, "", false, true, None);
            }
            Ok(AgentFailureLoopOutcome::RestartAfterSteer)
        }
        AgentFailureTerminalOutcome::Stopped => pause_agent_loop_for_control_stop(
            app,
            state,
            workspace_root,
            runtime,
            prompt,
            run_context,
            collaboration,
            cancellation,
        )
        .map(AgentFailureLoopOutcome::Finished),
    }
}

pub(crate) fn commit_agent_failure_terminal(
    state: &tauri::State<'_, AppState>,
    runtime: &AgentLoopState,
    run_context: &Metadata,
    cancellation: &AgentRunControl,
    epoch_lease: RunEpochLease,
    failure: &AgentFailure,
    display_message: String,
) -> Result<AgentFailureTerminalOutcome, String> {
    let failed_ledger = runtime
        .task_contract
        .failed_outcome_ledger(epoch_lease.epoch(), failure);
    let mut terminal_metadata = Metadata::new();
    let outcome_ledger_recorded = failed_ledger.insert_metadata(&mut terminal_metadata);
    terminal_metadata.insert(
        "outcome_ledger_status".to_string(),
        if outcome_ledger_recorded {
            "recorded"
        } else {
            "omitted_invalid"
        }
        .to_string(),
    );

    let session_id = run_context.get("session_id").map(String::as_str);
    let terminal_commit = cancellation.commit_terminal_result_with(epoch_lease, || {
        clear_suspended_agent_run_for_context(state, run_context)?;
        agent_state_with_error_metadata_in_context(
            state,
            run_context,
            display_message,
            terminal_metadata,
        )
    })?;
    match terminal_commit {
        RunTerminalCommit::Committed(agent_state) => {
            Ok(AgentFailureTerminalOutcome::Committed(agent_state))
        }
        RunTerminalCommit::RestartAfterSteer => Ok(AgentFailureTerminalOutcome::RestartAfterSteer),
        RunTerminalCommit::Stopped(_) => Ok(AgentFailureTerminalOutcome::Stopped),
        RunTerminalCommit::AlreadyCommitted => {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            let agent_state = agent_state_for_session(&store, None, session_id)
                .map_err(|error| error.to_string())?;
            Ok(AgentFailureTerminalOutcome::Committed(agent_state))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_failure_epoch_never_runs_the_terminal_writer() {
        let control = AgentRunControl::new("auto");
        let stale_lease = match control.execution_epoch_lease() {
            agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
            outcome => panic!("initial execution lease unavailable: {outcome:?}"),
        };
        assert_eq!(control.request_steer("new-failure-objective"), Ok(true));

        let mut writer_ran = false;
        let result = control
            .commit_terminal_result_with(stale_lease, || {
                writer_ran = true;
                Ok::<_, ()>(())
            })
            .expect("terminal arbitration should not fail");

        assert_eq!(result, RunTerminalCommit::RestartAfterSteer);
        assert!(!writer_ran);
    }
}
