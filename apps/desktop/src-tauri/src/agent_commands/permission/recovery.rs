use super::observations::{
    copy_replayed_contract_evidence_metadata, permission_checkpoint_message_boundary,
    persisted_permission_observations, restore_permission_snapshot_from_boundary,
};
use crate::agent_preparation_runtime::effective_prompt_objective_for_messages;
use crate::agent_read_model::{
    agent_recovery_prompt_from_active_events, agent_runtime_transcript_from_active_events,
};
use crate::agent_recovery_service::initial_agent_objective_from_events;
use crate::app_state::AgentRecoveryEnvelope;
use agent_core::{Event, Metadata, ToolOutcomeStatus, ToolSpec};
use agent_runtime::{AgentKernel, AgentTaskStateSnapshot};

pub(crate) fn recovery_task_state_with_persisted_permission_denials(
    active_events: &[Event],
    run_context: &Metadata,
    recovery: &AgentRecoveryEnvelope,
    persisted_task_state: Option<&AgentTaskStateSnapshot>,
) -> Option<AgentTaskStateSnapshot> {
    let snapshot = recovery.task_state.as_ref()?;
    let mut transcript = agent_runtime_transcript_from_active_events(active_events);
    let observations = persisted_permission_observations(&transcript, run_context);
    let replay_boundary =
        permission_checkpoint_message_boundary(active_events, recovery, snapshot)?;
    let denied_observations = observations
        .iter()
        .filter(|observation| {
            observation.message_index >= replay_boundary
                && matches!(observation.status, ToolOutcomeStatus::Denied)
        })
        .collect::<Vec<_>>();
    if denied_observations.is_empty() {
        return None;
    }
    if let Some(persisted_task_state) = persisted_task_state.filter(|task_state| {
        denied_observations.iter().all(|observation| {
            task_state.task_contract.has_user_permission_finalization(
                &observation.tool_name,
                &observation.input_fingerprint,
            )
        })
    }) {
        return Some(persisted_task_state.clone());
    }
    let recovery_prompt = agent_recovery_prompt_from_active_events(active_events)
        .unwrap_or_else(|| "Continue the agent task.".to_string());
    let effective_objective = initial_agent_objective_from_events(active_events)
        .map(|initial| effective_prompt_objective_for_messages(&initial, &transcript))
        .unwrap_or_else(|| {
            crate::runtime_values::effective_agent_objective(run_context, &recovery_prompt)
                .to_string()
        });
    let (mut runtime, _) = restore_permission_snapshot_from_boundary(
        snapshot,
        &recovery_prompt,
        &effective_objective,
        &transcript,
        replay_boundary,
    )?;
    let tools = Vec::<ToolSpec>::new();
    for observation in denied_observations {
        AgentKernel::new(&mut runtime, &tools).apply_persisted_tool_observation_with_denial(
            observation.call_id.clone(),
            &observation.tool_name,
            &observation.input_fingerprint,
            observation.target_witness.as_deref(),
            None,
            &observation.status,
            None,
            &observation.observation,
            Some(&agent_runtime::AgentActionDenialFeedback::user_permission()),
        );
        copy_replayed_contract_evidence_metadata(
            &runtime,
            &mut transcript,
            observation.message_index,
        );
    }
    runtime.messages = transcript;
    Some(crate::agent_runtime_snapshot::capture_persistable_agent_task_state(&runtime))
}
