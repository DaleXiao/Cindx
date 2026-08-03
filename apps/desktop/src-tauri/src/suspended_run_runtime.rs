use crate::{
    agent_query_commands::active_agent_run_control,
    app_state::{AppState, ResolvedToolObservation},
    collaboration_service::AgentCollaboration,
    configuration_models::persisted_agent_policy,
    runtime_values::current_time_millis,
    tool_execution::append_visual_reference_message,
};
use agent_core::{MessageRole, Metadata};
use agent_runtime::{AgentKernel, RunControlSnapshot};
use orchestrator::AgentPolicy;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

const SUSPENDED_AGENT_RUN_LIMIT: usize = 16;
const SUSPENDED_AGENT_RUN_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone)]
pub(crate) struct SuspendedAgentRun {
    pub(crate) runtime: agent_runtime::AgentLoopState,
    pub(crate) prompt: String,
    pub(crate) run_context: Metadata,
    pub(crate) workspace_root: PathBuf,
    pub(crate) collaboration: Option<AgentCollaboration>,
    pub(crate) run_control: RunControlSnapshot,
    pub(crate) last_touched_at_ms: u64,
}

#[derive(Default)]
pub(crate) struct SuspendedRunStore {
    runs: Mutex<BTreeMap<String, SuspendedAgentRun>>,
}

impl SuspendedRunStore {
    fn with_runs<T>(
        &self,
        operation: impl FnOnce(&mut BTreeMap<String, SuspendedAgentRun>) -> T,
    ) -> Result<T, String> {
        let mut runs = self
            .runs
            .lock()
            .map_err(|error| format!("suspended agent runs lock poisoned: {error}"))?;
        Ok(operation(&mut runs))
    }

    fn purge_expired(runs: &mut BTreeMap<String, SuspendedAgentRun>, now_ms: u64) {
        runs.retain(|_, run| {
            now_ms.saturating_sub(run.last_touched_at_ms) <= SUSPENDED_AGENT_RUN_TTL_MS
        });
    }

    fn remember(&self, mut run: SuspendedAgentRun, now_ms: u64) -> Result<(), String> {
        let Some(session_id) = run.run_context.get("session_id").cloned() else {
            return Ok(());
        };
        run.last_touched_at_ms = now_ms;
        self.with_runs(|runs| {
            Self::purge_expired(runs, now_ms);
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
        })
    }

    fn take(&self, session_id: &str, now_ms: u64) -> Result<Option<SuspendedAgentRun>, String> {
        self.with_runs(|runs| {
            Self::purge_expired(runs, now_ms);
            runs.remove(session_id)
        })
    }

    fn control_snapshot(
        &self,
        session_id: &str,
        now_ms: u64,
    ) -> Result<Option<RunControlSnapshot>, String> {
        self.with_runs(|runs| {
            Self::purge_expired(runs, now_ms);
            runs.get(session_id).map(|run| run.run_control.clone())
        })
    }

    fn agent_policy(
        &self,
        session_id: &str,
        now_ms: u64,
    ) -> Result<Option<AgentPolicy>, String> {
        self.with_runs(|runs| {
            Self::purge_expired(runs, now_ms);
            runs.get(session_id).map(|run| {
                persisted_agent_policy(run.run_context.get("agent_effort").map(String::as_str))
            })
        })?
        .transpose()
    }

    pub(crate) fn contains_any(&self, session_ids: &[String]) -> Result<bool, String> {
        let now_ms = current_time_millis();
        self.with_runs(|runs| {
            Self::purge_expired(runs, now_ms);
            session_ids
                .iter()
                .any(|session_id| runs.contains_key(session_id))
        })
    }

    pub(crate) fn remove_many(&self, session_ids: &[String]) -> Result<(), String> {
        self.with_runs(|runs| {
            for session_id in session_ids {
                runs.remove(session_id);
            }
        })
    }
}

pub(crate) fn remember_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    run: SuspendedAgentRun,
) -> Result<(), String> {
    state
        .suspended_agent_runs
        .remember(run, current_time_millis())
}

pub(crate) fn take_suspended_agent_run(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<SuspendedAgentRun>, String> {
    state
        .suspended_agent_runs
        .take(session_id, current_time_millis())
}

pub(crate) fn suspended_agent_run_control_snapshot(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<RunControlSnapshot>, String> {
    state
        .suspended_agent_runs
        .control_snapshot(session_id, current_time_millis())
}

pub(crate) fn suspended_agent_run_policy(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<Option<AgentPolicy>, String> {
    state
        .suspended_agent_runs
        .agent_policy(session_id, current_time_millis())
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
        if let Some(message) = suspended
            .runtime
            .messages
            .last_mut()
            .filter(|message| message.role == MessageRole::Tool)
        {
            message.metadata.extend(resolved.message_metadata.clone());
        }
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
