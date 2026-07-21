use super::*;

#[tauri::command]
pub(crate) fn get_phase6_state(state: tauri::State<'_, AppState>) -> Result<Phase6State, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    phase6_state(&store, None).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn run_orchestration(
    state: tauri::State<'_, AppState>,
    input: OrchestrationRunInput,
) -> Result<Phase6State, String> {
    let prompt = input.prompt.trim().to_string();
    if prompt.is_empty() {
        return phase6_state_with_error(&state, "orchestration prompt is empty");
    }
    let Some(requested_policy) = parse_policy(input.policy.trim()) else {
        return phase6_state_with_error(&state, format!("unknown policy: {}", input.policy));
    };
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return phase6_state_with_error(&state, "Provider config is incomplete");
    }

    let mut routing_context =
        RoutingContext::from_prompt(&prompt, model_candidates_for_config(&config));
    if requested_policy != OrchestrationPolicy::AutoRouter {
        routing_context.user_policy_override = Some(requested_policy.clone());
    }
    let router = RuleBasedRouter;
    let routing_decision = router.route(&routing_context);
    let policy = if requested_policy == OrchestrationPolicy::AutoRouter {
        routing_decision.policy.clone()
    } else {
        requested_policy.clone()
    };
    let plan = default_plan(policy.clone());
    let task_id = phase6_task_id();
    let orchestration_id = unique_id("orch");
    let mut previous_outputs = Vec::new();

    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            format!("Orchestration started: {}", policy.label()),
            [
                ("orchestration_id".to_string(), orchestration_id.clone()),
                (
                    "requested_policy".to_string(),
                    requested_policy.label().to_string(),
                ),
                ("policy".to_string(), policy.label().to_string()),
                ("router_model".to_string(), routing_decision.model.clone()),
                (
                    "router_retrieval_mode".to_string(),
                    routing_decision.retrieval_mode.clone(),
                ),
                (
                    "router_explanation".to_string(),
                    routing_decision.explanation.clone(),
                ),
                (
                    "task_class".to_string(),
                    routing_context.task_class.label().to_string(),
                ),
                ("prompt".to_string(), prompt.clone()),
            ]
            .into_iter()
            .collect(),
        )
        .map_err(|error| error.to_string())?;
    }

    for (step_index, step) in plan.steps.iter().enumerate() {
        let step_prompt = step_prompt(&plan, step_index, &prompt, &previous_outputs)
            .ok_or_else(|| "orchestration step was missing".to_string())?;
        let model = orchestration_model_for_step(&config, &step.role, &routing_decision);
        let request_id = unique_id("model");
        let started_at_ms = current_time_millis();

        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            append_event(
                &mut store,
                &task_id,
                EventKind::ModelRequestStarted,
                format!("{} step started", role_label(&step.role)),
                [
                    ("orchestration_id".to_string(), orchestration_id.clone()),
                    ("request_id".to_string(), request_id.clone()),
                    ("policy".to_string(), policy.label().to_string()),
                    ("step_index".to_string(), step_index.to_string()),
                    ("role".to_string(), role_label(&step.role).to_string()),
                    ("model".to_string(), model.clone()),
                ]
                .into_iter()
                .collect(),
            )
            .map_err(|error| error.to_string())?;
        }

        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            model: model.clone(),
            embedding_model: config.model_for_role(&ModelRole::Embedder),
            timeout_seconds: 180,
        });
        let request = ModelRequest {
            role: step.role.clone(),
            messages: vec![Message {
                role: MessageRole::User,
                content: step_prompt,
                metadata: Metadata::new(),
            }],
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: Metadata::new(),
        };

        match provider.complete_streaming(request, |_| {}) {
            Ok(response) => {
                let latency_ms = current_time_millis().saturating_sub(started_at_ms);
                previous_outputs.push(response.message.content.clone());
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                append_event(
                    &mut store,
                    &task_id,
                    EventKind::ModelRequestFinished,
                    format!("{} step finished", role_label(&step.role)),
                    [
                        ("orchestration_id".to_string(), orchestration_id.clone()),
                        ("request_id".to_string(), request_id),
                        ("policy".to_string(), policy.label().to_string()),
                        ("step_index".to_string(), step_index.to_string()),
                        ("role".to_string(), role_label(&step.role).to_string()),
                        ("model".to_string(), model),
                        ("latency_ms".to_string(), latency_ms.to_string()),
                        ("output".to_string(), response.message.content),
                    ]
                    .into_iter()
                    .collect(),
                )
                .map_err(|error| error.to_string())?;
            }
            Err(error) => {
                let message = error.to_string();
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                append_event(
                    &mut store,
                    &task_id,
                    EventKind::Error,
                    "Orchestration failed",
                    [
                        ("orchestration_id".to_string(), orchestration_id),
                        ("policy".to_string(), policy.label().to_string()),
                        ("step_index".to_string(), step_index.to_string()),
                        ("role".to_string(), role_label(&step.role).to_string()),
                        ("error".to_string(), message.clone()),
                    ]
                    .into_iter()
                    .collect(),
                )
                .map_err(|error| error.to_string())?;
                return phase6_state(&store, Some(message)).map_err(|error| error.to_string());
            }
        }
    }

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &task_id,
        EventKind::TaskStatusChanged,
        format!("Orchestration finished: {}", policy.label()),
        [
            ("orchestration_id".to_string(), orchestration_id),
            (
                "requested_policy".to_string(),
                requested_policy.label().to_string(),
            ),
            ("policy".to_string(), policy.label().to_string()),
            ("steps".to_string(), plan.steps.len().to_string()),
            ("router_model".to_string(), routing_decision.model.clone()),
            (
                "router_retrieval_mode".to_string(),
                routing_decision.retrieval_mode.clone(),
            ),
            (
                "router_explanation".to_string(),
                routing_decision.explanation.clone(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .map_err(|error| error.to_string())?;

    phase6_state(&store, None).map_err(|error| error.to_string())
}
