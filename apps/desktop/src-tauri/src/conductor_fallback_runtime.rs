use crate::{
    collaboration_service::truncate_for_collaboration, event_persistence::append_event,
    project_session_persistence::metadata_with_context,
};
use agent_core::{EventKind, Metadata, TaskId};
use agent_storage::SqliteStore;
use orchestrator::{ConductorHarness, WorkflowPlanIr};
use std::sync::Mutex;

pub(crate) fn deterministic_conductor_fallback(
    store: &Mutex<SqliteStore>,
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
    let mut store = store
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

#[cfg(test)]
mod tests {
    use super::deterministic_conductor_fallback;
    use agent_core::{EventKind, Metadata, TaskId};
    use agent_storage::{EventStore, SqliteStore};
    use orchestrator::{
        ConductorExecutionContract, ConductorHarness, ConductorPromptGenome, ConductorRequest,
        ConductorRoleHints, OrchestrationPolicy, RoutingContext, WorkflowBudget,
    };
    use std::sync::Mutex;

    fn test_harness() -> ConductorHarness {
        let routing = RoutingContext::from_prompt(
            "Compare two implementation strategies with evidence",
            Vec::new(),
        );
        ConductorHarness::new(ConductorRequest {
            workflow_id: "fallback-boundary".to_string(),
            objective: "Compare two implementation strategies with evidence".to_string(),
            recent_context: String::new(),
            effort: "auto".to_string(),
            policy: "best_of_n".to_string(),
            conductor_model: "planner".to_string(),
            primary_model: "worker-a".to_string(),
            worker_models: vec!["worker-a".to_string(), "worker-b".to_string()],
            role_hints: ConductorRoleHints {
                planner: "worker-a".to_string(),
                executor: "worker-b".to_string(),
                reviewer: "worker-b".to_string(),
                synthesizer: "worker-a".to_string(),
            },
            budget: WorkflowBudget {
                max_steps: 3,
                max_models: 2,
                max_model_turns_per_step: 2,
                max_tool_calls_per_step: 4,
                max_output_tokens_per_step: 2_048,
            },
            execution_contract: ConductorExecutionContract::from_routing(
                &routing,
                "auto",
                OrchestrationPolicy::BestOfN { candidates: 2 },
            ),
            prior_hint: None,
            prompt_evolution_enabled: true,
            prompt_genome: ConductorPromptGenome::seed_for_effort("auto"),
        })
    }

    #[test]
    fn deterministic_fallback_uses_only_the_store_and_preserves_its_event_contract() {
        let store = Mutex::new(SqliteStore::in_memory().expect("store should open"));
        let task_id = TaskId("fallback-task".to_string());
        let run_context: Metadata = [
            ("project_id".to_string(), "project-a".to_string()),
            ("session_id".to_string(), "session-a".to_string()),
        ]
        .into_iter()
        .collect();

        let plan = deterministic_conductor_fallback(
            &store,
            &task_id,
            &run_context,
            "collaboration-a",
            &test_harness(),
            2,
            "provider timeout",
        )
        .expect("fallback should succeed");

        let store = store.lock().expect("store lock should remain healthy");
        let events = store
            .list_by_task(&task_id)
            .expect("fallback event should load");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.kind, EventKind::TaskStatusChanged);
        assert_eq!(
            event.summary,
            "Conductor deterministic collaboration fallback applied"
        );
        assert_eq!(
            event.metadata.get("conductor_fallback").map(String::as_str),
            Some("deterministic_harness_dag")
        );
        assert_eq!(
            event.metadata.get("fallback_reason").map(String::as_str),
            Some("provider timeout")
        );
        let workflow_steps = plan.steps.len().to_string();
        assert_eq!(
            event.metadata.get("workflow_steps").map(String::as_str),
            Some(workflow_steps.as_str())
        );
        assert_eq!(
            event.metadata.get("session_id").map(String::as_str),
            Some("session-a")
        );
    }
}
