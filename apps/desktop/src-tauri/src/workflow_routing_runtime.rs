use super::*;

pub(crate) fn workflow_prior_for_run(
    _state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    allowed_models: &[String],
    max_models: usize,
) -> Result<Option<WorkflowTopologyPrior>, String> {
    let events = open_app_read_store()?
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

pub(crate) fn model_candidates_for_config(config: &ProviderConfig) -> Vec<ModelCandidate> {
    [
        (ModelRole::Executor, config.model.clone(), 1, 1),
        (
            ModelRole::Planner,
            config.model_for_role(&ModelRole::Planner),
            3,
            2,
        ),
        (
            ModelRole::Executor,
            config.model_for_role(&ModelRole::Executor),
            2,
            1,
        ),
        (
            ModelRole::Reviewer,
            config.model_for_role(&ModelRole::Reviewer),
            2,
            2,
        ),
        (
            ModelRole::Summarizer,
            config.model_for_role(&ModelRole::Summarizer),
            1,
            1,
        ),
    ]
    .into_iter()
    .map(|(role, name, cost_tier, latency_tier)| ModelCandidate {
        name,
        role,
        supports_tools: true,
        supports_vision: true,
        cost_tier,
        latency_tier,
    })
    .collect()
}

pub(crate) fn route_with_local_telemetry(
    _state: &tauri::State<'_, AppState>,
    context: &RoutingContext,
) -> Result<(RoutingDecision, usize), String> {
    let mut store = open_app_read_store()?;
    let telemetry = load_routing_telemetry_read_model_snapshot(&mut store)
        .map_err(|error| error.to_string())?;
    let router = LearnedModelRouter::train(&telemetry);
    let learned_examples = router
        .learned_route_for_context(context)
        .map(|route| route.examples)
        .unwrap_or(0);
    let learned_evidence_ready = router
        .learned_route_for_context(context)
        .is_some_and(|route| route.evidence_ready());
    let learned_model_available = router
        .learned_route_for_context(context)
        .map(|route| {
            context
                .model_candidates
                .iter()
                .any(|candidate| candidate.name == route.model)
        })
        .unwrap_or(false);
    let decision = if learned_evidence_ready && learned_model_available {
        router.route(context)
    } else {
        let mut decision = RuleBasedRouter.route(context);
        decision
            .metadata
            .entry("router".to_string())
            .or_insert_with(|| "rule_based_v2".to_string());
        decision
    };
    Ok((decision, learned_examples))
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
