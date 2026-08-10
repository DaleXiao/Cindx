use super::{causal_route, requirements, PlannedAgentRun};
use crate::app_state::AppState;
use crate::collaboration_service::truncate_for_collaboration;
use crate::collaboration_stage_runtime::CollaborationStageError;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::workflow_routing_runtime::append_router_decision_event;
use agent_application::{insert_run_objectives, AgentStrategyDecisionReceipt};
use agent_core::{decode_event_type, DecodedEventType, EventKind, EventTypeV1, Metadata, TaskId};
use agent_runtime::{AgentRunControl, RunPreparationCheckpoint};

pub(super) fn record_planned_agent_run(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &mut Metadata,
    planned: &PlannedAgentRun,
    profile_source: &str,
    cancellation: &AgentRunControl,
) -> Result<(), CollaborationStageError> {
    let decision = planned.execution_plan.action();
    let causal_route_metadata =
        causal_route::causal_route_event_metadata(run_context, &planned.execution_plan)
            .map_err(CollaborationStageError::Failed)?;
    let plan_sha256 = planned
        .execution_plan
        .semantic_digest()
        .map_err(CollaborationStageError::Failed)?;
    let receipt = AgentStrategyDecisionReceipt::new(task_id, run_context, &plan_sha256)
        .map_err(|error| CollaborationStageError::Failed(error.to_string()))?;
    let mut committed_context = run_context.clone();
    receipt
        .insert_into(&mut committed_context)
        .map_err(|error| CollaborationStageError::Failed(error.to_string()))?;
    let mut decision_metadata = [
        (
            "decision_source".to_string(),
            planned.source.label().to_string(),
        ),
        (
            "decision_attempts".to_string(),
            planned.attempts.to_string(),
        ),
        (
            "conductor_models_attempted".to_string(),
            planned.attempted_conductor_models.join(","),
        ),
        (
            "conductor_selected_model".to_string(),
            planned.selected_conductor_model.clone().unwrap_or_default(),
        ),
        (
            "decision".to_string(),
            serde_json::to_string(decision).unwrap_or_default(),
        ),
        (
            "execution_plan".to_string(),
            serde_json::to_string(&planned.execution_plan).unwrap_or_default(),
        ),
        (
            "execution_plan_sha256".to_string(),
            planned
                .execution_plan
                .digest()
                .map_err(CollaborationStageError::Failed)?,
        ),
        ("execution_plan_semantic_sha256".to_string(), plan_sha256),
        (
            "execution_plan_authority".to_string(),
            planned.execution_plan.authority.label().to_string(),
        ),
        (
            "execution_plan_decision_reason".to_string(),
            planned
                .execution_plan
                .decision_receipt
                .as_ref()
                .map(|receipt| receipt.reason.label())
                .unwrap_or("legacy")
                .to_string(),
        ),
        (
            "prompt_profile".to_string(),
            planned.prompt_genome.id.clone(),
        ),
        ("profile_source".to_string(), profile_source.to_string()),
        (
            "conductor_degraded".to_string(),
            planned.degradation_reason.is_some().to_string(),
        ),
        (
            "conductor_failure".to_string(),
            planned
                .degradation_reason
                .as_deref()
                .map(|reason| truncate_for_collaboration(reason, 1_200))
                .unwrap_or_default(),
        ),
        (
            "decision_rationale".to_string(),
            truncate_for_collaboration(&decision.rationale, 1_200),
        ),
    ]
    .into_iter()
    .chain(causal_route_metadata)
    .chain(requirements::route_decision_metadata(planned))
    .collect();
    insert_run_objectives(&mut decision_metadata, run_context);
    receipt
        .insert_into(&mut decision_metadata)
        .map_err(|error| CollaborationStageError::Failed(error.to_string()))?;
    let checkpoint = cancellation.commit_preparation_checkpoint_with(receipt.steer_epoch(), || {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .with_immediate_transaction(|store| {
                let existing = store.list_by_task_and_metadata(
                    task_id,
                    "agent_run_id",
                    receipt.agent_run_id(),
                )?;
                let mut decisions = existing.iter().filter(|event| {
                    matches!(
                        decode_event_type(event),
                        DecodedEventType::V1(typed)
                            if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
                    ) && event
                        .metadata
                        .get("steer_epoch")
                        .and_then(|value| value.parse::<u64>().ok())
                        == Some(receipt.steer_epoch())
                });
                if let Some(existing) = decisions.next() {
                    if decisions.next().is_some() || !receipt.matches_decision_event(existing) {
                        return Err(agent_storage::StorageError::new(
                            "agent strategy receipt conflicts with a durable decision",
                        ));
                    }
                    #[cfg(feature = "realworld-eval")]
                    crate::collaboration_learning_eval_runtime::append_direct_assignment_if_enabled(
                        store,
                        task_id,
                        &committed_context,
                    )?;
                    return Ok(());
                }
                append_router_decision_event(
                    store,
                    task_id,
                    run_context,
                    &planned.routing_context,
                    &planned.routing_decision,
                    0,
                )?;
                append_event(
                    store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Agent run decision selected",
                    metadata_with_context(decision_metadata, run_context),
                )?;
                #[cfg(feature = "realworld-eval")]
                crate::collaboration_learning_eval_runtime::append_direct_assignment_if_enabled(
                    store,
                    task_id,
                    &committed_context,
                )?;
                Ok(())
            })
            .map_err(|error| error.to_string())
    });
    match checkpoint {
        Ok(RunPreparationCheckpoint::Committed(())) => {
            *run_context = committed_context;
            Ok(())
        }
        Ok(RunPreparationCheckpoint::RestartAfterSteer) => {
            Err(CollaborationStageError::SteerInterrupted)
        }
        Ok(RunPreparationCheckpoint::Stopped(_)) => Err(CollaborationStageError::RunStopped),
        Err(error) => Err(CollaborationStageError::Failed(error)),
    }
}
