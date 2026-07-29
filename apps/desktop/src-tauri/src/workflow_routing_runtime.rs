use super::*;

pub(crate) fn workflow_prior_for_run(
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    allowed_models: &[String],
    max_models: usize,
) -> Result<Option<WorkflowTopologyPrior>, String> {
    let telemetry = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        load_workflow_telemetry_read_model(&mut store, allowed_models)
            .map_err(|error| error.to_string())?
    };
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
    let routing_signature = run_context
        .get("routing_signature")
        .map(String::as_str)
        .unwrap_or_default();
    Ok(teacher
        .best_prior_for_signature(
            &task_class,
            effort,
            allowed_models,
            max_models,
            routing_signature,
        )
        .cloned())
}

pub(crate) fn conductor_historical_evidence(
    state: &tauri::State<'_, AppState>,
    allowed_models: &[String],
) -> Result<String, String> {
    let telemetry = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        load_routing_telemetry_read_model(&mut store).map_err(|error| error.to_string())?
    };
    let allowed = allowed_models
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let evidence = LearnedModelRouter::train(&telemetry)
        .calibrated_evidence()
        .into_iter()
        .filter(|route| allowed.contains(route.model.as_str()))
        .take(12)
        .map(|route| {
            format!(
                "class={} execution={} model={} samples={} success={:.0}% lower_confidence={:.2} quality={} verification={} latency_ms={}",
                route.task_class.label(),
                route.policy.label(),
                route.model,
                route.examples,
                route.success_rate * 100.0,
                route.success_confidence,
                route
                    .average_quality_score
                    .map(|score| format!("{score:.2}"))
                    .unwrap_or_else(|| "unrated".to_string()),
                route
                    .verification_rate
                    .map(|rate| format!("{:.0}%", rate * 100.0))
                    .unwrap_or_else(|| "unrated".to_string()),
                route.average_latency_ms,
            )
        })
        .collect::<Vec<_>>();
    Ok(evidence.join("\n"))
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
    .map(|(role, name, cost_tier, latency_tier)| {
        let supports_vision = provider_model_supports_vision(&config.provider_id, &name)
            .unwrap_or_else(|| model_supports_vision_content(&name));
        let supports_tools =
            provider_model_supports_tools(&config.provider_id, &name).unwrap_or(true);
        ModelCandidate {
            name,
            role,
            supports_tools,
            supports_vision,
            cost_tier,
            latency_tier,
        }
    })
    .collect()
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
