use super::{causal_route, requirements, PlannedAgentRun};
use crate::app_state::AppState;
use crate::collaboration_service::truncate_for_collaboration;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::workflow_routing_runtime::append_router_decision_event;
use agent_core::{EventKind, Metadata, TaskId};

pub(super) fn record_planned_agent_run(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    planned: &PlannedAgentRun,
    profile_source: &str,
) -> Result<(), String> {
    let causal_route_metadata =
        causal_route::causal_route_event_metadata(run_context, &planned.decision)?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_router_decision_event(
        &mut store,
        task_id,
        run_context,
        &planned.routing_context,
        &planned.routing_decision,
        0,
    )
    .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        metadata_with_context(
            [
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
                    serde_json::to_string(&planned.decision).unwrap_or_default(),
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
                    truncate_for_collaboration(&planned.decision.rationale, 1_200),
                ),
            ]
            .into_iter()
            .chain(causal_route_metadata)
            .chain(requirements::route_decision_metadata(planned))
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}
