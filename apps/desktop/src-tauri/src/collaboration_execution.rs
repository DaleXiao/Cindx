use super::*;

pub(crate) fn collaboration_run_should_interrupt(control: &Arc<AgentRunControl>) -> bool {
    agent_run_should_stop(control) || control.has_pending_steer()
}

fn collaboration_stage_should_interrupt(
    control: &Arc<AgentRunControl>,
    stage_class: RunStageClass,
) -> bool {
    collaboration_run_should_interrupt(control) || control.stage_should_stop(stage_class)
}

#[derive(Debug)]
pub(crate) struct CollaborationCandidateSpec {
    pub(crate) stage: String,
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) request_id: String,
}

pub(crate) fn collaboration_candidate_models(
    config: &ProviderConfig,
    candidates: usize,
) -> Vec<String> {
    let limit = candidates.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS);
    let mut models = Vec::new();
    for role in [
        ModelRole::Planner,
        ModelRole::Executor,
        ModelRole::Reviewer,
        ModelRole::Summarizer,
    ] {
        let model = config.model_for_role(&role);
        if !model.trim().is_empty() && !models.iter().any(|existing| existing == &model) {
            models.push(model);
        }
        if models.len() >= limit {
            break;
        }
    }
    models
}

pub(crate) fn collaboration_role_hints(
    config: &ProviderConfig,
    worker_models: &[String],
) -> ConductorRoleHints {
    let fallback = |preferred: String, index: usize| {
        if worker_models.iter().any(|model| model == &preferred) {
            preferred
        } else {
            worker_models
                .get(index)
                .or_else(|| worker_models.first())
                .cloned()
                .unwrap_or(preferred)
        }
    };
    let planner = fallback(config.model_for_role(&ModelRole::Planner), 0);
    let mut executor = fallback(config.model_for_role(&ModelRole::Executor), 1);
    if executor == planner {
        executor = worker_models
            .iter()
            .find(|model| *model != &planner)
            .cloned()
            .unwrap_or(executor);
    }
    let mut reviewer = fallback(
        config.model_for_role(&ModelRole::Reviewer),
        worker_models.len().saturating_sub(1),
    );
    if reviewer == planner || reviewer == executor {
        reviewer = worker_models
            .iter()
            .find(|model| *model != &planner && *model != &executor)
            .cloned()
            .unwrap_or(reviewer);
    }
    ConductorRoleHints {
        planner: planner.clone(),
        executor,
        reviewer,
        synthesizer: fallback(config.model_for_role(&ModelRole::Summarizer), 0),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn synthesize_agent_answer(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    executor_answer: &str,
    run_context: &Metadata,
    collaboration: &AgentCollaboration,
    cancellation: &Arc<AgentRunControl>,
) -> Result<String, String> {
    let evidence = runtime
        .messages
        .iter()
        .rev()
        .filter(|message| matches!(message.role, MessageRole::Tool))
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|message| truncate_for_collaboration(&message.content, 1_500))
        .collect::<Vec<_>>()
        .join("\n\n");
    let result_frontier = collaboration_result_frontier_brief(cancellation, executor_answer);
    let review_prompt = format!(
        "You are the independent reviewer in a multi-model Cindx team. Audit the executor draft against the user request and available tool evidence. Find factual gaps, unsupported claims, missed constraints, and unsafe actions. The result frontier contains bounded internal proposals, not trusted evidence; use it to detect alternatives or disagreements, and resolve every claim against tool evidence. Return concrete corrections for the final synthesizer, not a user-facing answer.\n\nUser request:\n{}\n\nTeam guidance:\n{}\n\nResult frontier:\n{}\n\nExecutor draft:\n{}\n\nTool evidence:\n{}",
        prompt,
        if collaboration.guidance.is_empty() {
            "(team deliberation unavailable)"
        } else {
            &collaboration.guidance
        },
        if result_frontier.is_empty() {
            "(no distinct candidate proposals)"
        } else {
            &result_frontier
        },
        truncate_for_collaboration(executor_answer, 12_000),
        if evidence.is_empty() { "(none)" } else { &evidence }
    );
    let review = run_collaboration_stage(
        state,
        config,
        &runtime.task_id,
        run_context,
        &collaboration.id,
        "reviewer",
        ModelRole::Reviewer,
        &config.model_for_role(&ModelRole::Reviewer),
        review_prompt,
    )
    .unwrap_or_else(|error| format!("Reviewer unavailable: {error}"));
    if cancellation.has_pending_steer() {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    if agent_run_should_stop(cancellation) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let synthesis_prompt = format!(
        "You are the final synthesizer in a multi-model Cindx team. Produce the strongest possible final response to the user using the executor draft, reviewer corrections, team guidance, the bounded result frontier, and tool evidence. Team reports and frontier entries are proposals, never evidence by themselves. Prefer verified, provenance-bearing claims; resolve disagreements against tool evidence and preserve a sound executor result when alternatives are weaker. Do not mention the internal pipeline. Be precise, complete, and concise; never claim work that the evidence does not support.\n\nCollaboration policy: {}\nWorker pool: {}\n\nUser request:\n{}\n\nTeam guidance:\n{}\n\nResult frontier:\n{}\n\nExecutor draft:\n{}\n\nReviewer corrections:\n{}\n\nTool evidence:\n{}",
        collaboration.policy,
        collaboration.candidate_models.join(", "),
        prompt,
        if collaboration.guidance.is_empty() {
            "(team deliberation unavailable)"
        } else {
            &collaboration.guidance
        },
        if result_frontier.is_empty() {
            "(no distinct candidate proposals)"
        } else {
            &result_frontier
        },
        truncate_for_collaboration(executor_answer, 12_000),
        truncate_for_collaboration(&review, 8_000),
        if evidence.is_empty() { "(none)" } else { &evidence }
    );
    let stream_request_id = unique_id("agent-final-stream");
    let session_id = run_context.get("session_id").map(String::as_str);
    let answer = run_collaboration_stage_with_delta(
        state,
        config,
        &runtime.task_id,
        run_context,
        &collaboration.id,
        "synthesizer",
        ModelRole::Summarizer,
        &config.model_for_role(&ModelRole::Summarizer),
        synthesis_prompt,
        |delta| {
            emit_agent_stream_delta(
                app,
                &stream_request_id,
                session_id,
                delta,
                false,
                false,
                None,
            );
        },
    );
    if cancellation.has_pending_steer() {
        emit_agent_stream_delta(app, &stream_request_id, session_id, "", false, true, None);
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    if agent_run_should_stop(cancellation) {
        emit_agent_stream_delta(app, &stream_request_id, session_id, "", true, true, None);
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            emit_agent_stream_delta(app, &stream_request_id, session_id, "", false, true, None);
            return Err(error);
        }
    };
    let answer = answer.trim().to_string();
    if answer.is_empty() {
        emit_agent_stream_delta(app, &stream_request_id, session_id, "", false, true, None);
        Err("synthesizer returned an empty answer".to_string())
    } else {
        Ok(answer)
    }
}

pub(crate) fn collaboration_result_frontier_brief(
    cancellation: &AgentRunControl,
    executor_answer: &str,
) -> String {
    cancellation
        .result_frontier()
        .into_iter()
        .filter(|result| result.content.trim() != executor_answer.trim())
        .take(3)
        .enumerate()
        .map(|(index, result)| {
            format!(
                "Candidate {} [stage={}, quality={}, verified={}, evidence={}]:\n{}",
                index + 1,
                result.stage,
                result.quality.as_str(),
                result.verified,
                result.evidence_count,
                truncate_for_collaboration(&result.content, 1_800)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_collaboration_stage_started(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: &ModelRole,
    model: &str,
    request_id: &str,
    stage_metadata: &Metadata,
) -> Result<(), String> {
    let mut metadata = [
        ("collaboration_id".to_string(), collaboration_id.to_string()),
        ("request_id".to_string(), request_id.to_string()),
        ("stage".to_string(), stage.to_string()),
        ("role".to_string(), role_label(role).to_string()),
        ("model".to_string(), model.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    for (key, value) in stage_metadata {
        metadata.insert(key.clone(), value.clone());
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::ModelRequestStarted,
        format!("Collaboration {stage} started"),
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn complete_collaboration_model_with_control(
    config: ProviderConfig,
    role: ModelRole,
    model: String,
    system_prompt: String,
    prompt: String,
    cancellation: Option<Arc<AgentRunControl>>,
    on_delta: impl FnMut(&str),
) -> CollaborationCompletion {
    let stage = role_label(&role).to_string();
    complete_collaboration_model_for_stage_with_control(
        config,
        stage,
        role,
        model,
        system_prompt,
        prompt,
        cancellation,
        on_delta,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_collaboration_model_for_stage_with_control(
    config: ProviderConfig,
    stage: String,
    role: ModelRole,
    model: String,
    system_prompt: String,
    prompt: String,
    cancellation: Option<Arc<AgentRunControl>>,
    mut on_delta: impl FnMut(&str),
) -> CollaborationCompletion {
    let started_at_ms = current_time_millis();
    let role_name = role_label(&role).to_string();
    let inferred_stage_class = RunStageClass::from_label(&stage);
    let stage_class = if inferred_stage_class == RunStageClass::Other {
        RunStageClass::from_label(&role_name)
    } else {
        inferred_stage_class
    };
    if let Some(control) = cancellation.as_ref() {
        if let Err(reason) = control.begin_stage_model_call(&stage, stage_class) {
            return CollaborationCompletion::failed(format!(
                "Run stopped before model call: {}",
                reason.code()
            ));
        }
    }
    let timeout_seconds = cancellation
        .as_ref()
        .map(|control| control.model_call_timeout_seconds())
        .unwrap_or(180);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model,
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds,
    });
    let mut partial_output = String::new();
    let mut stream_progress = ModelStreamProgress::new();
    let mut first_delta_at_ms = None;
    let response = provider.complete_streaming_cancellable(
        ModelRequest {
            role,
            messages: vec![
                Message {
                    role: MessageRole::System,
                    content: system_prompt,
                    metadata: Metadata::new(),
                },
                Message {
                    role: MessageRole::User,
                    content: prompt,
                    metadata: Metadata::new(),
                },
            ],
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: [(
                "max_output_tokens".to_string(),
                COLLABORATION_MAX_OUTPUT_TOKENS.to_string(),
            )]
            .into_iter()
            .collect(),
        },
        |delta| {
            if !delta.is_empty() {
                first_delta_at_ms.get_or_insert_with(current_time_millis);
                partial_output.push_str(delta);
            }
            if let Some(control) = cancellation.as_ref() {
                stream_progress.observe(control, "model_stream", &stage, &partial_output);
            }
            on_delta(delta);
        },
        || {
            cancellation
                .as_ref()
                .is_some_and(|control| collaboration_stage_should_interrupt(control, stage_class))
        },
    );
    if let Some(control) = cancellation.as_ref() {
        control.finish_model_call();
    }
    let latency_ms = current_time_millis().saturating_sub(started_at_ms);
    match response {
        Ok(response) => {
            let mut usage = Metadata::new();
            for key in [
                "prompt_tokens",
                "completion_tokens",
                "total_tokens",
                "usage_source",
                "usage_estimated",
            ] {
                if let Some(value) = response.metadata.get(key) {
                    usage.insert(key.to_string(), value.clone());
                }
            }
            if let Some(first_delta_at_ms) = first_delta_at_ms {
                usage.insert(
                    "first_token_latency_ms".to_string(),
                    first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
                );
            }
            let content = match no_tool_collaboration_content(response, &role_name) {
                Ok(content) => content,
                Err(error) => {
                    if let Some(control) = cancellation.as_ref() {
                        control.record_observation("model_protocol_error", &role_name, &error);
                    }
                    return CollaborationCompletion {
                        content: None,
                        error: Some(error),
                        latency_ms,
                        usage,
                        evidence: Vec::new(),
                    };
                }
            };
            if let Some(control) = cancellation.as_ref() {
                control.record_partial_output(&content);
                control.record_observation("model_result", &stage, &content);
                let quality = match stage_class {
                    RunStageClass::Reviewer => ResultQuality::Verified,
                    RunStageClass::Synthesizer => ResultQuality::Synthesized,
                    RunStageClass::Candidate | RunStageClass::Worker => ResultQuality::Substantive,
                    _ => ResultQuality::Draft,
                };
                control.record_best_known_result(
                    &stage,
                    &content,
                    quality,
                    0,
                    false,
                    stage_class == RunStageClass::Synthesizer,
                );
            }
            CollaborationCompletion {
                content: Some(content),
                error: None,
                latency_ms,
                usage,
                evidence: Vec::new(),
            }
        }
        Err(error) => {
            if let Some(control) = cancellation.as_ref() {
                control.record_observation("provider_failure", error.class.label(), &error.message);
            }
            let mut usage = Metadata::new();
            usage.insert(
                "provider_failure_class".to_string(),
                error.class.label().to_string(),
            );
            usage.insert(
                "provider_failure_retryable".to_string(),
                error.retryable.to_string(),
            );
            if let Some(status_code) = error.status_code {
                usage.insert("provider_status_code".to_string(), status_code.to_string());
            }
            CollaborationCompletion {
                content: None,
                error: Some(error.to_string()),
                latency_ms,
                usage,
                evidence: Vec::new(),
            }
        }
    }
}

fn no_tool_collaboration_content(
    response: model_provider::ModelResponse,
    role_name: &str,
) -> Result<String, String> {
    if !response.tool_calls.is_empty() {
        return Err(format!(
            "collaboration {role_name} attempted {} tool call(s) in a no-tool stage",
            response.tool_calls.len()
        ));
    }
    Ok(response.message.content)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_collaboration_worker_with_tools(
    app: tauri::AppHandle,
    config: ProviderConfig,
    task_id: TaskId,
    workspace_root: PathBuf,
    run_context: Metadata,
    collaboration_id: String,
    stage: String,
    role: ModelRole,
    model: String,
    prompt: String,
    allow_tools: bool,
    max_model_turns: usize,
    max_tool_calls: usize,
    cancellation: Option<Arc<AgentRunControl>>,
    branch_cancellation: Option<Arc<AtomicBool>>,
) -> CollaborationCompletion {
    let started_at_ms = current_time_millis();
    let state = app.state::<AppState>();
    let registry = if allow_tools {
        match tool_registry_for_state(&state, &workspace_root) {
            Ok(registry) => Some(registry),
            Err(error) => return CollaborationCompletion::failed(error),
        }
    } else {
        None
    };
    let tools = if let Some(registry) = registry.as_ref() {
        evidence_worker_tools(
            &registry
                .exposure_plan(&prompt, config.context_window_tokens)
                .inline,
        )
    } else {
        Vec::new()
    };
    let has_tools = !tools.is_empty();
    let evidence_turn_limit = max_model_turns.max(1);
    let timeout_seconds = cancellation
        .as_ref()
        .map(|control| control.model_call_timeout_seconds())
        .unwrap_or(180);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model,
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds,
    });
    let mut runtime = start_agent_loop(
        task_id,
        prompt.clone(),
        AgentRuntimeConfig {
            max_turns: collaboration_worker_runtime_turn_limit(evidence_turn_limit, has_tools),
        },
    );
    let mut trusted_context = agent_runtime_context_for_run(&run_context).unwrap_or_default();
    if !trusted_context.is_empty() {
        trusted_context.push('\n');
    }
    trusted_context.push_str(&format!(
        "Collaboration: {collaboration_id}\nWorker stage: {stage}\nWorker role: {}\n{} Evidence budget: {evidence_turn_limit} tool-capable model rounds and {max_tool_calls} total tool calls. Batch independent reads, prefer decisive file reads or searches over repeated directory listings, and stop gathering evidence as soon as the assigned question is answerable. Return concise conclusions for downstream workers; do not claim workspace changes.",
        role_label(&role),
        if allow_tools {
            "This worker has an isolated transcript and may use only exposed read-only evidence tools."
        } else {
            "This is a bounded worker with no tools. Reason only from the supplied request and recent context, then finish in one response."
        }
    ));
    let mut worker_context = run_context.clone();
    let evidence_source = stage.clone();
    worker_context.insert("collaboration_id".to_string(), collaboration_id);
    worker_context.insert("stage".to_string(), stage.clone());
    worker_context.insert("role".to_string(), role_label(&role).to_string());
    worker_context.insert(
        "worker_runtime".to_string(),
        "isolated_evidence_v1".to_string(),
    );
    let mut usage = Metadata::new();
    let mut evidence = Vec::new();
    let mut tool_call_count = 0usize;
    let mut first_delta_at_ms = None;
    let stage_class = RunStageClass::Worker;

    loop {
        if branch_cancellation
            .as_ref()
            .is_some_and(|cancelled| cancelled.load(Ordering::SeqCst))
        {
            return CollaborationCompletion {
                content: None,
                error: Some("collaboration branch cancelled after quorum".to_string()),
                latency_ms: current_time_millis().saturating_sub(started_at_ms),
                usage,
                evidence,
            };
        }
        if cancellation
            .as_ref()
            .is_some_and(collaboration_run_should_interrupt)
        {
            return CollaborationCompletion {
                content: None,
                error: Some(
                    if cancellation
                        .as_ref()
                        .is_some_and(|control| control.has_pending_steer())
                    {
                        COLLABORATION_STEER_INTERRUPTED
                    } else {
                        MODEL_REQUEST_CANCELLED
                    }
                    .to_string(),
                ),
                latency_ms: current_time_millis().saturating_sub(started_at_ms),
                usage,
                evidence,
            };
        }

        if let Some(control) = cancellation.as_ref() {
            if let Err(reason) = control.begin_stage_model_call(&stage, stage_class) {
                return CollaborationCompletion {
                    content: None,
                    error: Some(format!(
                        "Run stopped before worker model call: {}",
                        reason.code()
                    )),
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                };
            }
        }

        let finalizing =
            prepare_collaboration_worker_turn(&mut runtime, has_tools, evidence_turn_limit);
        let request_tools: &[ToolSpec] = if finalizing { &[] } else { &tools };

        let max_output_tokens = bounded_max_output_tokens(
            config.context_window_tokens,
            COLLABORATION_MAX_OUTPUT_TOKENS,
        );
        let prepared_turn = AgentKernel::new(&mut runtime, request_tools).prepare_model_turn(
            Some(&config.agent_system_prompt),
            Some(&trusted_context),
            config.context_window_tokens,
            max_output_tokens,
        );
        let mut request = prepared_turn.request;
        let governor = prepared_turn.context;
        request.role = role.clone();
        request
            .metadata
            .insert("collaboration_worker".to_string(), "true".to_string());
        request.metadata.insert(
            "max_output_tokens".to_string(),
            max_output_tokens.to_string(),
        );
        usage.insert(
            "context_governor_applied".to_string(),
            governor.applied.to_string(),
        );
        usage.insert(
            "context_projected_tokens".to_string(),
            governor.estimated_projected_tokens.to_string(),
        );
        let mut partial_output = String::new();
        let mut stream_progress = ModelStreamProgress::new();
        let response = provider.complete_streaming_cancellable(
            request,
            |delta| {
                if !delta.is_empty() {
                    first_delta_at_ms.get_or_insert_with(current_time_millis);
                    partial_output.push_str(delta);
                }
                if let Some(control) = cancellation.as_ref() {
                    stream_progress.observe(control, "model_stream", &stage, &partial_output);
                }
            },
            || {
                cancellation.as_ref().is_some_and(|control| {
                    collaboration_stage_should_interrupt(control, stage_class)
                }) || branch_cancellation
                    .as_ref()
                    .is_some_and(|cancelled| cancelled.load(Ordering::SeqCst))
            },
        );
        if let Some(control) = cancellation.as_ref() {
            control.finish_model_call();
        }
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if let Some(control) = cancellation.as_ref() {
                    control.record_observation(
                        "provider_failure",
                        error.class.label(),
                        &error.message,
                    );
                }
                usage.insert(
                    "provider_failure_class".to_string(),
                    error.class.label().to_string(),
                );
                usage.insert(
                    "provider_failure_retryable".to_string(),
                    error.retryable.to_string(),
                );
                if let Some(status_code) = error.status_code {
                    usage.insert("provider_status_code".to_string(), status_code.to_string());
                }
                return CollaborationCompletion {
                    content: None,
                    error: Some(error.to_string()),
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                };
            }
        };
        if let Some(control) = cancellation.as_ref() {
            if let Err(reason) = control.record_agent_turn(&stage) {
                return CollaborationCompletion {
                    content: None,
                    error: Some(format!(
                        "Run stopped after worker model turn: {}",
                        reason.code()
                    )),
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                };
            }
        }
        for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
            let previous = usage
                .get(key)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default();
            let additional = response
                .metadata
                .get(key)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default();
            usage.insert(
                key.to_string(),
                previous.saturating_add(additional).to_string(),
            );
        }
        let previous_source = usage.get("usage_source").map(String::as_str);
        let additional_source = response.metadata.get("usage_source").map(String::as_str);
        let combined_source = if [previous_source, additional_source]
            .into_iter()
            .flatten()
            .any(|source| source == "estimated")
        {
            "estimated"
        } else if [previous_source, additional_source]
            .into_iter()
            .flatten()
            .any(|source| source == "provider_partial")
        {
            "provider_partial"
        } else {
            "provider"
        };
        usage.insert("usage_source".to_string(), combined_source.to_string());
        usage.insert(
            "usage_estimated".to_string(),
            (combined_source != "provider").to_string(),
        );
        let finalization_content = finalizing
            .then(|| response.message.content.trim().to_string())
            .filter(|content| !content.is_empty());
        if let Some(control) = cancellation.as_ref() {
            if let Some(evidence) = model_response_checkpoint_evidence(&response) {
                control.record_observation("model_result", &stage, &evidence);
            }
        }

        match AgentKernel::new(&mut runtime, request_tools).advance_model_response(response) {
            AgentAdvance::Completed { answer } => {
                if let Some(control) = cancellation.as_ref() {
                    control.record_partial_output(&answer);
                    control.record_best_known_result(
                        &stage,
                        &answer,
                        if evidence.is_empty() {
                            ResultQuality::Substantive
                        } else {
                            ResultQuality::Grounded
                        },
                        evidence.len(),
                        false,
                        false,
                    );
                }
                usage.insert("worker_turns".to_string(), runtime.turn.to_string());
                usage.insert("worker_tool_calls".to_string(), tool_call_count.to_string());
                usage.insert("worker_tool_count".to_string(), tools.len().to_string());
                if let Some(first_delta_at_ms) = first_delta_at_ms {
                    usage.insert(
                        "first_token_latency_ms".to_string(),
                        first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
                    );
                }
                usage.insert(
                    "worker_runtime".to_string(),
                    "isolated_evidence_v1".to_string(),
                );
                return CollaborationCompletion {
                    content: Some(answer),
                    error: None,
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                };
            }
            AgentAdvance::TurnBudgetExhausted {
                completed_turns,
                max_turns,
                ..
            } => {
                return CollaborationCompletion {
                    content: None,
                    error: Some(format!(
                        "worker_turn_budget_exhausted: completed {completed_turns} turns with a {max_turns}-turn budget"
                    )),
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                }
            }
            AgentAdvance::Failed { message } => {
                return CollaborationCompletion {
                    content: None,
                    error: Some(message),
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                }
            }
            AgentAdvance::Retry { instruction } => {
                AgentKernel::new(&mut runtime, request_tools)
                    .apply_empty_response_retry(instruction);
                continue;
            }
            AgentAdvance::ToolCalls { calls } => {
                if finalizing {
                    if let Some(answer) = finalization_content {
                        if let Some(control) = cancellation.as_ref() {
                            control.record_best_known_result(
                                &stage,
                                &answer,
                                if evidence.is_empty() {
                                    ResultQuality::Substantive
                                } else {
                                    ResultQuality::Grounded
                                },
                                evidence.len(),
                                false,
                                false,
                            );
                        }
                        usage.insert("worker_turns".to_string(), runtime.turn.to_string());
                        usage.insert(
                            "worker_tool_calls".to_string(),
                            tool_call_count.to_string(),
                        );
                        usage.insert("worker_tool_count".to_string(), tools.len().to_string());
                        usage.insert(
                            "worker_runtime".to_string(),
                            "isolated_evidence_v1".to_string(),
                        );
                        return CollaborationCompletion {
                            content: Some(answer),
                            error: None,
                            latency_ms: current_time_millis().saturating_sub(started_at_ms),
                            usage,
                            evidence,
                        };
                    }
                    return CollaborationCompletion {
                        content: None,
                        error: Some(
                            "collaboration worker requested another tool after its evidence phase; final answer was empty"
                                .to_string(),
                        ),
                        latency_ms: current_time_millis().saturating_sub(started_at_ms),
                        usage,
                        evidence,
                    };
                }
                for call in calls {
                    tool_call_count += 1;
                    let tool_call_id = call.call_id.0.clone();
                    let mut invocation =
                        AgentKernel::new(&mut runtime, request_tools).tool_invocation(&call);
                    invocation.proposed_by_model = "collaboration-worker".to_string();
                    invocation
                        .metadata
                        .insert("collaboration_worker".to_string(), "true".to_string());
                    {
                        let mut store = match state.store.lock() {
                            Ok(store) => store,
                            Err(error) => {
                                return CollaborationCompletion::failed(format!(
                                    "store lock poisoned: {error}"
                                ))
                            }
                        };
                        if let Err(error) =
                            append_tool_proposed_event(&mut store, &invocation, Some(&worker_context))
                        {
                            return CollaborationCompletion::failed(error.to_string());
                        }
                    }

                    let budget_exhausted = tool_call_count > max_tool_calls;
                    let rejection = if budget_exhausted {
                        Some("This collaboration worker exhausted its evidence-tool budget. Stop searching and return the best concise brief from existing evidence.")
                    } else if AgentKernel::new(&mut runtime, request_tools)
                        .repeated_tool_failure_count(&call)
                        >= MAX_IDENTICAL_TOOL_FAILURES
                    {
                        Some("Cindx blocked this identical worker tool call after repeated failures. Change the arguments or use a different approach.")
                    } else if !tools.iter().any(|tool| tool.name == call.tool_name) {
                        Some("This tool is not exposed to the collaboration worker. Return the proposed action to the main executor instead.")
                    } else {
                        None
                    };
                    let (status, observation) = if let Some(reason) = rejection {
                        let observation =
                            observation_from_tool_result(&call.tool_name, "failed", reason);
                        evidence.push(CollaborationEvidence {
                            source_step: evidence_source.clone(),
                            tool_call_id: tool_call_id.clone(),
                            tool_name: call.tool_name.clone(),
                            request: call.input.clone(),
                            status: "failed".to_string(),
                            output: truncate_for_collaboration(reason, 2_000),
                        });
                        let mut store = match state.store.lock() {
                            Ok(store) => store,
                            Err(error) => {
                                return CollaborationCompletion::failed(format!(
                                    "store lock poisoned: {error}"
                                ))
                            }
                        };
                        let failure_code = if budget_exhausted {
                            "worker_tool_budget_exhausted"
                        } else {
                            "worker_tool_not_allowed"
                        };
                        if let Err(error) = append_tool_finished_event(
                            &mut store,
                            &runtime.task_id,
                            &call.call_id.0,
                            &call.tool_name,
                            "failed",
                            &observation,
                            [("failure_code".to_string(), failure_code.to_string())]
                                .into_iter()
                                .collect(),
                            Some(&worker_context),
                        ) {
                            return CollaborationCompletion::failed(error.to_string());
                        }
                        (ToolOutcomeStatus::Failed, observation)
                    } else {
                        let result = registry.as_ref().ok_or_else(|| {
                            "collaboration worker tool registry is unavailable".to_string()
                        });
                        match result.and_then(|registry| {
                            execute_agent_tool_invocation(
                                &state,
                                registry,
                                invocation,
                                &workspace_root,
                                &worker_context,
                            )
                        }) {
                            Ok(result) => {
                                let status = result.status.clone();
                                evidence.push(CollaborationEvidence {
                                    source_step: evidence_source.clone(),
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: call.tool_name.clone(),
                                    request: call.input.clone(),
                                    status: tool_outcome_label(&result.status).to_string(),
                                    output: truncate_for_collaboration(&result.output, 2_000),
                                });
                                (
                                    status,
                                    observation_from_agent_tool_result(&call.tool_name, &result),
                                )
                            }
                            Err(error) => {
                                evidence.push(CollaborationEvidence {
                                    source_step: evidence_source.clone(),
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: call.tool_name.clone(),
                                    request: call.input.clone(),
                                    status: "failed".to_string(),
                                    output: truncate_for_collaboration(&error, 2_000),
                                });
                                (
                                    ToolOutcomeStatus::Failed,
                                    observation_from_tool_result(
                                        &call.tool_name,
                                        "failed",
                                        &error,
                                    ),
                                )
                            }
                        }
                    };
                    AgentKernel::new(&mut runtime, request_tools).apply_tool_observation(
                        &call,
                        &status,
                        None,
                        &observation,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_collaboration_stage_finished(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: &ModelRole,
    model: &str,
    request_id: &str,
    completion: &CollaborationCompletion,
    stage_metadata: &Metadata,
) -> Result<(), String> {
    let mut metadata = [
        ("collaboration_id".to_string(), collaboration_id.to_string()),
        ("request_id".to_string(), request_id.to_string()),
        ("stage".to_string(), stage.to_string()),
        ("role".to_string(), role_label(role).to_string()),
        ("model".to_string(), model.to_string()),
        ("latency_ms".to_string(), completion.latency_ms.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    metadata.insert(
        "evidence_count".to_string(),
        completion.evidence.len().to_string(),
    );
    metadata.insert(
        "evidence_tools".to_string(),
        completion
            .evidence
            .iter()
            .map(|entry| entry.tool_name.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(","),
    );
    for (key, value) in stage_metadata {
        metadata.insert(key.clone(), value.clone());
    }
    let summary = if let Some(content) = completion.content.as_ref() {
        metadata.insert("output".to_string(), content.clone());
        metadata.insert("status".to_string(), "completed".to_string());
        for (key, value) in &completion.usage {
            metadata.insert(key.clone(), value.clone());
        }
        format!("Collaboration {stage} finished")
    } else {
        metadata.insert("status".to_string(), "degraded".to_string());
        metadata.insert(
            "error".to_string(),
            completion
                .error
                .clone()
                .unwrap_or_else(|| "unknown collaboration failure".to_string()),
        );
        format!("Collaboration {stage} unavailable")
    };
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::ModelRequestFinished,
        summary,
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_collaboration_stage(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: ModelRole,
    model: &str,
    prompt: String,
) -> Result<String, String> {
    run_collaboration_stage_with_delta(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        stage,
        role,
        model,
        prompt,
        |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_collaboration_stage_with_delta(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    role: ModelRole,
    model: &str,
    prompt: String,
    on_delta: impl FnMut(&str),
) -> Result<String, String> {
    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
    if let Some(control) = cancellation.as_ref() {
        if control.has_pending_steer() {
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        if agent_run_should_stop(control) {
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
    }
    let request_id = unique_id("collaboration-model");
    record_collaboration_stage_started(
        state,
        task_id,
        run_context,
        collaboration_id,
        stage,
        &role,
        model,
        &request_id,
        &Metadata::new(),
    )?;
    let completion = complete_collaboration_model_for_stage_with_control(
        config.clone(),
        stage.to_string(),
        role.clone(),
        model.to_string(),
        collaboration_system_prompt_for_run(&config.agent_system_prompt, run_context),
        prompt,
        cancellation.clone(),
        on_delta,
    );
    record_collaboration_stage_finished(
        state,
        task_id,
        run_context,
        collaboration_id,
        stage,
        &role,
        model,
        &request_id,
        &completion,
        &Metadata::new(),
    )?;
    if cancellation
        .as_ref()
        .is_some_and(|control| control.has_pending_steer())
    {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    completion.content.ok_or_else(|| {
        completion
            .error
            .unwrap_or_else(|| "collaboration model returned no content".to_string())
    })
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    fn response_with_tool_call() -> model_provider::ModelResponse {
        model_provider::ModelResponse {
            message: agent_core::Message {
                role: agent_core::MessageRole::Assistant,
                content: String::new(),
                metadata: agent_core::Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: vec![model_provider::ModelToolCall {
                id: "call-dsml-0".to_string(),
                name: "shell_run".to_string(),
                arguments_json: r#"{"command":"pwd"}"#.to_string(),
            }],
            metadata: agent_core::Metadata::new(),
        }
    }

    #[test]
    fn no_tool_collaboration_stage_rejects_tool_calls() {
        let error = no_tool_collaboration_content(response_with_tool_call(), "synthesizer")
            .expect_err("tool calls must not be accepted as final content");

        assert_eq!(
            error,
            "collaboration synthesizer attempted 1 tool call(s) in a no-tool stage"
        );
    }
}
