use super::*;

pub(crate) fn conductor_historical_evidence(
    state: &tauri::State<'_, AppState>,
    allowed_models: &[String],
) -> Result<String, String> {
    let routing_telemetry = {
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
    let routes = LearnedModelRouter::train(&routing_telemetry)
        .calibrated_evidence()
        .into_iter()
        .filter(|route| allowed.contains(route.model.as_str()))
        .take(8)
        .map(|route| {
            format!(
                "route_observation class={} execution={} model={} samples={} success={:.0}% lower_confidence={:.2} quality={} verification={} latency_ms={}",
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
        });
    Ok(routes.collect::<Vec<_>>().join("\n"))
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
    effort_model_candidates(config, "")
}

pub(crate) fn effort_primary_model(config: &ProviderConfig, effort_label: &str) -> Option<String> {
    let pinned = config.effort_default_model(effort_label);
    (!pinned.is_empty()).then_some(pinned)
}

pub(crate) fn effort_model_candidates(
    config: &ProviderConfig,
    effort_label: &str,
) -> Vec<ModelCandidate> {
    let mut candidates = base_model_candidates_for_config(config);
    let pinned = config.effort_default_model(effort_label);
    if !pinned.is_empty() && !candidates.iter().any(|candidate| candidate.name == pinned) {
        let (cost_tier, latency_tier) = match effort_label {
            "fast" => (1, 1),
            "pro" => (3, 1),
            _ => (2, 1),
        };
        let name = pinned;
        let catalog_vision = provider_model_supports_vision(&config.provider_id, &name);
        let catalog_tools = provider_model_supports_tools(&config.provider_id, &name);
        let supports_vision =
            catalog_vision.unwrap_or_else(|| model_supports_vision_content(&name));
        let supports_tools = catalog_tools.unwrap_or(true);
        candidates.push(ModelCandidate {
            name,
            role: ModelRole::Executor,
            supports_tools,
            supports_vision,
            tools_capability_source: if catalog_tools.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
            vision_capability_source: if catalog_vision.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
            cost_tier,
            latency_tier,
        });
    }
    candidates
}

fn base_model_candidates_for_config(config: &ProviderConfig) -> Vec<ModelCandidate> {
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
        let catalog_vision = provider_model_supports_vision(&config.provider_id, &name);
        let catalog_tools = provider_model_supports_tools(&config.provider_id, &name);
        let supports_vision =
            catalog_vision.unwrap_or_else(|| model_supports_vision_content(&name));
        let supports_tools = catalog_tools.unwrap_or(true);
        ModelCandidate {
            name,
            role,
            supports_tools,
            supports_vision,
            tools_capability_source: if catalog_tools.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
            vision_capability_source: if catalog_vision.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
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
