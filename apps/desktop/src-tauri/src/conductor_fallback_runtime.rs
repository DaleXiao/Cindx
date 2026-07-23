use super::*;

pub(crate) fn deterministic_conductor_fallback(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    harness: &ConductorHarness,
    attempts: usize,
    reason: &str,
) -> Result<WorkflowPlanIr, String> {
    let plan = harness.fallback_plan().map_err(|fallback_error| {
        format!(
            "Conductor failed and its deterministic collaboration fallback was invalid: {fallback_error}; original failure: {reason}"
        )
    })?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Conductor deterministic collaboration fallback applied",
        metadata_with_context(
            [
                ("collaboration_id".to_string(), collaboration_id.to_string()),
                ("conductor_attempts".to_string(), attempts.to_string()),
                (
                    "conductor_fallback".to_string(),
                    "deterministic_harness_dag".to_string(),
                ),
                (
                    "fallback_reason".to_string(),
                    truncate_for_collaboration(reason, 2_000),
                ),
                ("workflow_steps".to_string(), plan.steps.len().to_string()),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(plan)
}
