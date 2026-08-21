use crate::{
    agent_query_commands::active_agent_run_control,
    app_state::{AppState, ResolvedToolObservation},
    collaboration_service::AgentCollaboration,
    configuration_models::persisted_agent_policy,
    runtime_values::current_time_millis,
    tool_execution::append_visual_reference_message,
};
use agent_core::{MessageRole, Metadata};
use agent_runtime::{
    run_context_steer_epoch, AgentGoalDelta, AgentKernel, AgentRunControl, RunControlSnapshot,
};
use agent_core::AgentPolicy;
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

    fn agent_policy(&self, session_id: &str, now_ms: u64) -> Result<Option<AgentPolicy>, String> {
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
    let objective_epoch = run_context_steer_epoch(&suspended.run_context);
    let postcondition_scope = agent_runtime::postcondition_lineage_scope(&suspended.run_context);
    let mut goal_deltas = Vec::new();
    for resolved in observations {
        let goal_delta = apply_resolved_tool_observation(
            &mut suspended.runtime,
            resolved,
            postcondition_scope.as_deref(),
        );
        goal_deltas.extend(goal_delta);
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
    let active_control = active_agent_run_control(state, Some(session_id))?;
    if let Some(control) = active_control {
        control.commit_goal_deltas_at_with(objective_epoch, &goal_deltas, move |snapshot| {
            suspended.run_control = snapshot.clone();
            remember_suspended_agent_run(state, suspended)
        })?;
    } else {
        suspended.run_control = staged_control_snapshot_with_goal_deltas(
            suspended.run_control,
            objective_epoch,
            &goal_deltas,
        );
        remember_suspended_agent_run(state, suspended)?;
    }
    Ok(())
}

fn staged_control_snapshot_with_goal_deltas(
    base_snapshot: RunControlSnapshot,
    objective_epoch: u64,
    goal_deltas: &[AgentGoalDelta],
) -> RunControlSnapshot {
    let staged_control = AgentRunControl::from_snapshot(base_snapshot);
    for delta in goal_deltas {
        staged_control.record_goal_delta_at(objective_epoch, delta);
    }
    staged_control.snapshot()
}

fn apply_resolved_tool_observation(
    runtime: &mut agent_runtime::AgentLoopState,
    resolved: &ResolvedToolObservation,
    postcondition_scope: Option<&str>,
) -> Option<AgentGoalDelta> {
    let request = agent_runtime::AgentToolRequest {
        call_id: resolved.call_id.clone(),
        tool_name: resolved.tool_name.clone(),
        input: resolved.input_json.clone(),
    };
    let denial = matches!(resolved.status, agent_core::ToolOutcomeStatus::Denied)
        .then(agent_runtime::AgentActionDenialFeedback::user_permission);
    let tools = resolved.effect_spec.as_slice();
    AgentKernel::new(runtime, tools)
        .with_postcondition_scope(postcondition_scope)
        .apply_tool_observation_transition_with_contract(
            &request,
            &resolved.status,
            resolved.risk.as_ref(),
            resolved.effect_spec.as_ref(),
            resolved.postcondition_evidence.as_ref(),
            &resolved.observation,
            denial.as_ref(),
        )
        .goal_delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{TaskId, ToolCallId, ToolOutcomeStatus, ToolRisk};
    use agent_runtime::{
        start_agent_loop, AgentGoalDeltaKind, AgentRuntimeConfig, OutcomeSatisfaction,
        WorkspaceVerificationPolicy,
    };

    fn resolved(
        id: &str,
        tool_name: &str,
        input_json: &str,
        risk: ToolRisk,
    ) -> ResolvedToolObservation {
        ResolvedToolObservation {
            call_id: ToolCallId(id.to_string()),
            tool_name: tool_name.to_string(),
            input_json: input_json.to_string(),
            risk: Some(risk),
            effect_spec: None,
            postcondition_evidence: None,
            status: ToolOutcomeStatus::Succeeded,
            observation: "succeeded".to_string(),
            image_paths: Vec::new(),
            message_metadata: Metadata::new(),
        }
    }

    fn trusted_workspace_verification_delta() -> AgentGoalDelta {
        let mut runtime = start_agent_loop(
            TaskId("permission-risk".to_string()),
            "change and verify",
            AgentRuntimeConfig::default(),
        );
        runtime.task_contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        let write_spec = agent_core::ToolSpec::builtin(
            "file.write",
            "file",
            "Write a file",
            ToolRisk::WritesWorkspace,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::Verifiable {
            verifier: "workspace_file_content_v1".to_string(),
        });
        let mut write = resolved(
            "write",
            "file.write",
            r#"{"path":"src/lib.rs"}"#,
            ToolRisk::WritesWorkspace,
        );
        write.effect_spec = Some(write_spec);
        assert!(
            apply_resolved_tool_observation(&mut runtime, &write, Some("permission-risk:0"),)
                .is_none()
        );

        assert!(apply_resolved_tool_observation(
            &mut runtime,
            &resolved(
                "verify",
                "mcp.custom_verify",
                r#"{"command":"cargo test"}"#,
                ToolRisk::ExecutesProcess,
            ),
            Some("permission-risk:0"),
        )
        .is_none());
        assert!(
            !runtime.task_contract.latest_mutation_verified(),
            "custom risk labels must not mint verifier authority"
        );

        let read_input = r#"{"path":"src/lib.rs"}"#;
        let read_spec = agent_core::ToolSpec::builtin(
            "file.read",
            "file",
            "Read a file",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)
        .with_postcondition_verifier(
            agent_core::PostconditionVerifierKind::WorkspaceExactReadbackV1,
        );
        let mut read = resolved("read", "file.read", read_input, ToolRisk::ReadOnly);
        read.effect_spec = Some(read_spec);
        read.postcondition_evidence = Some(agent_core::ToolPostconditionEvidence {
            kind: agent_core::PostconditionVerifierKind::WorkspaceExactReadbackV1,
            target_input_json: read_input.to_string(),
        });
        apply_resolved_tool_observation(&mut runtime, &read, Some("permission-risk:0"))
            .expect("the trusted exact-readback evidence should verify the workspace mutation")
    }

    #[test]
    fn same_process_permission_replay_uses_the_resolved_tool_risk() {
        let delta = trusted_workspace_verification_delta();
        assert_eq!(delta.kinds(), &[AgentGoalDeltaKind::WorkspaceVerified]);
    }

    #[test]
    fn suspended_snapshot_records_goal_delta_without_an_active_control() {
        let delta = trusted_workspace_verification_delta();
        let base = AgentRunControl::new("auto").snapshot();
        let staged =
            staged_control_snapshot_with_goal_deltas(base, 0, std::slice::from_ref(&delta));
        let restored = AgentRunControl::from_snapshot(staged);

        assert!(
            !restored.record_goal_delta_at(0, &delta),
            "the suspended snapshot should retain the admitted Goal Delta fingerprint"
        );
    }

    #[test]
    fn same_process_permission_denial_is_a_terminal_constraint_without_goal_credit() {
        let mut runtime = start_agent_loop(
            TaskId("permission-denial".to_string()),
            "write protected.txt",
            AgentRuntimeConfig::default(),
        );
        runtime.task_contract.begin_action_denial_epoch(3);
        runtime.task_contract.require_tool_success("file.write");
        let mut observation = resolved(
            "write",
            "file.write",
            r#"{"path":"protected.txt"}"#,
            ToolRisk::WritesWorkspace,
        );
        observation.status = ToolOutcomeStatus::Denied;
        observation.observation = "The user denied this tool call.".to_string();

        assert!(apply_resolved_tool_observation(&mut runtime, &observation, None).is_none());
        let ledger = runtime.task_contract.outcome_ledger_shadow(3);
        assert_eq!(ledger.obligations.len(), 1);
        assert_eq!(
            ledger.obligations[0].satisfaction,
            OutcomeSatisfaction::Blocked
        );
        assert_eq!(
            ledger.obligations[0]
                .blocker
                .as_ref()
                .map(|blocker| blocker.code.as_str()),
            Some("user_permission_denied")
        );
    }
}
