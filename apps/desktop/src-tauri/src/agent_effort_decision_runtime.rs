use crate::agent_effort_planner::EffortRunPlan;
use crate::app_state::AppState;
use crate::collaboration_stage_runtime::CollaborationStageError;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use agent_application::{insert_run_objectives, AgentStrategyDecisionReceipt};
use agent_core::{decode_event_type, DecodedEventType, EventKind, EventTypeV1, Metadata, TaskId};
use agent_runtime::{AgentRunControl, RunPreparationCheckpoint};

/// Commits the effort-tier plan as the run's selected decision: one typed
/// "Agent run decision selected" event plus the strategy receipt, written
/// atomically behind the preparation epoch checkpoint. The receipt binds the
/// effort plan digest in place of the retired execution-plan semantic digest.
pub(crate) fn record_effort_plan_decision(
    state: &AppState,
    task_id: &TaskId,
    run_context: &mut Metadata,
    plan: &EffortRunPlan,
    cancellation: &AgentRunControl,
) -> Result<(), CollaborationStageError> {
    let plan_sha256 = plan
        .plan_digest()
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
            "effort_tier_planner".to_string(),
        ),
        ("decision_attempts".to_string(), "0".to_string()),
        ("execution_plan_semantic_sha256".to_string(), plan_sha256),
    ]
    .into_iter()
    .collect::<Metadata>();
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
                    return Ok(());
                }
                append_event(
                    store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Agent run decision selected",
                    metadata_with_context(decision_metadata, run_context),
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
