use super::*;

pub(crate) fn workflow_prior_for_run(
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    allowed_models: &[String],
    max_models: usize,
) -> Result<Option<WorkflowTopologyPrior>, String> {
    let events = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?
        .list_by_task(&phase16_task_id())
        .map_err(|error| error.to_string())?;
    let telemetry = workflow_execution_telemetry_from_events(&events, allowed_models);
    let teacher = WorkflowSearchTeacher::train(&telemetry);
    let Some(task_class) = run_context
        .get("task_class")
        .and_then(|value| parse_task_class_label(value))
    else {
        return Ok(None);
    };
    let effort = run_context
        .get("agent_effort")
        .map(String::as_str)
        .unwrap_or("auto");
    Ok(teacher
        .best_prior(&task_class, effort, allowed_models, max_models)
        .cloned())
}

pub(crate) fn parse_task_class_label(value: &str) -> Option<TaskClass> {
    match value {
        "general" => Some(TaskClass::General),
        "coding" => Some(TaskClass::Coding),
        "research" => Some(TaskClass::Research),
        "retrieval" => Some(TaskClass::Retrieval),
        "browser" => Some(TaskClass::Browser),
        "computer" => Some(TaskClass::Computer),
        _ => None,
    }
}

pub(crate) fn append_router_decision_event(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    context: &RoutingContext,
    decision: &RoutingDecision,
    learned_examples: usize,
) -> Result<(), StorageError> {
    let mut metadata = decision.metadata.clone();
    metadata.insert("policy".to_string(), decision.policy.label().to_string());
    metadata.insert("model".to_string(), decision.model.clone());
    metadata.insert(
        "retrieval_mode".to_string(),
        decision.retrieval_mode.clone(),
    );
    metadata.insert("explanation".to_string(), decision.explanation.clone());
    metadata.insert(
        "prompt_length".to_string(),
        context.prompt_length.to_string(),
    );
    metadata.insert(
        "routing_signature".to_string(),
        context.learning_signature(),
    );
    metadata.insert("learned_examples".to_string(), learned_examples.to_string());
    append_event(
        store,
        task_id,
        EventKind::TaskStatusChanged,
        "Agent router selected collaboration policy",
        metadata_with_context(metadata, run_context),
    )
}

pub(crate) fn orchestration_model_for_step(
    config: &ProviderConfig,
    role: &ModelRole,
    routing_decision: &RoutingDecision,
) -> String {
    if *role == ModelRole::Executor && routing_decision.policy == OrchestrationPolicy::Single {
        config.model_for_agent_policy(&routing_decision.policy)
    } else if *role == ModelRole::Executor && !routing_decision.model.trim().is_empty() {
        routing_decision.model.clone()
    } else {
        config.model_for_role(role)
    }
}
