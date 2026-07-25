use super::*;

pub(crate) fn collaboration_run_should_interrupt(control: &Arc<AgentRunControl>) -> bool {
    agent_run_should_stop(control) || control.has_pending_steer()
}

pub(crate) fn collaboration_stage_should_interrupt(
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
            return CollaborationCompletion::failed_with(AgentFailure::from_stop_reason(
                reason,
                format!("Run stopped before model call: {}", reason.code()),
            ));
        }
    }
    let timeout_seconds = cancellation
        .as_ref()
        .map(|control| control.stage_model_call_timeout_seconds(stage_class))
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
                Err(failure) => {
                    if let Some(control) = cancellation.as_ref() {
                        control.record_observation(
                            "model_protocol_error",
                            &role_name,
                            &failure.message,
                        );
                    }
                    return CollaborationCompletion {
                        content: None,
                        partial_content: (!partial_output.trim().is_empty())
                            .then_some(partial_output),
                        error: Some(failure.message.clone()),
                        failure: Some(failure),
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
                partial_content: None,
                error: None,
                failure: None,
                latency_ms,
                usage,
                evidence: Vec::new(),
            }
        }
        Err(error) => {
            let failure = AgentFailure::from_model_error(&error);
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
                partial_content: (!partial_output.trim().is_empty()).then_some(partial_output),
                error: Some(failure.message.clone()),
                failure: Some(failure),
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
) -> Result<String, AgentFailure> {
    match response.assessment().disposition {
        model_provider::ModelResponseDisposition::ToolCalls => {
            return Err(AgentFailure::model_output(
                "unexpected_tool_calls",
                format!(
                    "collaboration {role_name} attempted {} tool call(s) in a no-tool stage",
                    response.tool_calls.len()
                ),
            ));
        }
        model_provider::ModelResponseDisposition::IncompleteOutput => {
            return Err(AgentFailure::model_output(
                "incomplete_output",
                format!(
                    "collaboration {role_name} response reached its output limit before completion"
                ),
            ));
        }
        model_provider::ModelResponseDisposition::Filtered => {
            return Err(AgentFailure::model_output(
                "filtered_output",
                format!("collaboration {role_name} response was blocked by the provider filter"),
            ));
        }
        model_provider::ModelResponseDisposition::Empty => {
            return Err(AgentFailure::model_output(
                "empty_output",
                format!("collaboration {role_name} returned an empty response"),
            ));
        }
        model_provider::ModelResponseDisposition::Usable => {}
    }
    Ok(response.message.content)
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

        assert_eq!(error.class, agent_runtime::AgentFailureClass::ModelOutput);
        assert_eq!(error.code, "unexpected_tool_calls");
        assert_eq!(
            error.message,
            "collaboration synthesizer attempted 1 tool call(s) in a no-tool stage"
        );
    }
}
