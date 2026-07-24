use super::*;

pub(crate) fn append_single_model_policy_guidance(
    history: &mut Vec<Message>,
    policy: &OrchestrationPolicy,
) {
    if *policy != OrchestrationPolicy::PlanExecuteReview {
        return;
    }
    history.push(Message {
        role: MessageRole::System,
        content: "Use a single-model plan-execute-review loop for this request: form a concise plan, execute only the required tools, verify the result against evidence, then answer. Do not expose private chain-of-thought; report only decisions, actions, and verified results.".to_string(),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            (
                "kind".to_string(),
                "single_model_policy_guidance".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pause_agent_loop_for_control_stop(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    if cancellation
        .stop_reason()
        .is_some_and(|reason| !reason.is_user_cancelled())
    {
        remember_suspended_agent_run(
            state,
            SuspendedAgentRun {
                runtime: runtime.clone(),
                prompt: prompt.to_string(),
                run_context: run_context.clone(),
                workspace_root: workspace_root.to_path_buf(),
                collaboration: collaboration.cloned(),
                run_control: cancellation.snapshot(),
            },
        )?;
    }
    let task_state = AgentTaskStateSnapshot::capture(runtime);
    finish_agent_run_for_control_stop_with_task_state(
        app,
        state,
        run_context,
        cancellation,
        Some(&task_state),
    )
}

pub(crate) fn apply_pending_agent_steers(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
    cancellation: &AgentRunControl,
) -> Result<Option<String>, String> {
    let pending_ids = cancellation.take_pending_steers();
    if pending_ids.is_empty() {
        return Ok(None);
    }
    let Some(session_id) = run_context.get("session_id") else {
        return Ok(None);
    };
    let pending = {
        let mut store = open_app_read_store()?;
        let model = load_agent_session_read_model(&mut store, session_id)
            .map_err(|error| error.to_string())?;
        pending_ids
            .into_iter()
            .filter_map(|pending| {
                let view = model
                    .state
                    .queued_messages
                    .iter()
                    .find(|message| message.id == pending.queue_id)?
                    .clone();
                let payload = model.queued_payloads.get(&pending.queue_id)?.clone();
                Some((view, payload))
            })
            .collect::<Vec<_>>()
    };

    let mut latest_prompt = None;
    for (view, payload) in pending {
        let attachments = validate_agent_attachments(workspace_root, payload.attachments)?;
        let model_prompt = prompt_with_attachments(&payload.prompt, &attachments);
        let previous_message_count = runtime.messages.len();
        let mut metadata = [
            ("queue_id".to_string(), view.id.clone()),
            ("queue_mode".to_string(), "steer".to_string()),
            ("display_content".to_string(), payload.prompt.clone()),
        ]
        .into_iter()
        .collect::<Metadata>();
        add_attachment_metadata(&mut metadata, &attachments);
        AgentKernel::new(runtime, &[]).apply_steer(model_prompt.clone(), metadata);

        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_agent_queue_event(
            &mut store,
            run_context,
            "start",
            &view.id,
            "steer",
            view.created_at_ms,
            None,
        )?;
        persist_new_runtime_messages(
            &mut store,
            &runtime.task_id,
            &runtime.messages,
            previous_message_count,
            run_context,
        )
        .map_err(|error| error.to_string())?;
        latest_prompt = Some(model_prompt);
    }
    if latest_prompt.is_some() {
        cancellation.record_checkpoint(
            "steering",
            "User guidance applied",
            latest_prompt.as_deref().unwrap_or_default(),
        );
    }
    Ok(latest_prompt)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn continue_agent_loop(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    workspace_root: &Path,
    mut runtime: agent_runtime::AgentLoopState,
    mut prompt: String,
    mut run_context: Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    let agent_model = agent_model_for_run(config, &run_context);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: agent_model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: cancellation.model_call_timeout_seconds(),
    });
    let registry = tool_registry_for_state(state, workspace_root)?;
    let mut tools = registry
        .exposure_plan(&prompt, config.context_window_tokens)
        .inline;
    let mut runtime_context = agent_runtime_context_for_run(&run_context);
    let mut active_collaboration = collaboration;

    'agent_loop: loop {
        if let Some(steer_prompt) = apply_pending_agent_steers(
            state,
            workspace_root,
            &mut runtime,
            &run_context,
            cancellation,
        )? {
            active_collaboration = None;
            prompt = steer_prompt;
            add_image_generation_run_context(&mut run_context, config, &prompt);
            tools = registry
                .exposure_plan(&prompt, config.context_window_tokens)
                .inline;
            runtime_context = agent_runtime_context_for_run(&run_context);
            cancellation.mark_progress("steering", "User guidance applied");
            append_agent_progress_event(
                state,
                &runtime.task_id,
                &run_context,
                "Applying user steering",
            )?;
        }
        if cancellation.begin_model_call("executor").is_err() {
            return pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                &runtime,
                &prompt,
                &run_context,
                active_collaboration,
                cancellation,
            );
        }
        let max_output_tokens =
            bounded_max_output_tokens(config.context_window_tokens, AGENT_MAX_OUTPUT_TOKENS);
        let prepared_turn = AgentKernel::new(&mut runtime, &tools).prepare_model_turn(
            Some(&config.agent_system_prompt),
            runtime_context.as_deref(),
            config.context_window_tokens,
            max_output_tokens,
        );
        let mut request = prepared_turn.request;
        let context_governor = prepared_turn.context;
        request.metadata.insert(
            "max_output_tokens".to_string(),
            max_output_tokens.to_string(),
        );
        let request_id = unique_id("agent-model");
        let started_at_ms = current_time_millis();
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            if agent_task_is_cancelled(&mut store, session_id).map_err(|error| error.to_string())? {
                return agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string());
            }
            let mut metadata = [
                ("request_id".to_string(), request_id.clone()),
                ("turn".to_string(), runtime.turn.to_string()),
                ("model".to_string(), agent_model.clone()),
                ("tool_count".to_string(), tools.len().to_string()),
                ("prompt".to_string(), prompt.clone()),
                (
                    "context_governor_applied".to_string(),
                    context_governor.applied.to_string(),
                ),
                (
                    "context_original_tokens".to_string(),
                    context_governor.estimated_original_tokens.to_string(),
                ),
                (
                    "context_projected_tokens".to_string(),
                    context_governor.estimated_projected_tokens.to_string(),
                ),
                (
                    "context_input_budget_tokens".to_string(),
                    context_governor.input_budget_tokens.to_string(),
                ),
                (
                    "context_omitted_messages".to_string(),
                    context_governor.omitted_messages.to_string(),
                ),
            ]
            .into_iter()
            .collect::<Metadata>();
            if let Some(collaboration) = active_collaboration {
                metadata.insert("collaboration_id".to_string(), collaboration.id.clone());
                metadata.insert("stage".to_string(), "executor".to_string());
                metadata.insert("role".to_string(), "executor".to_string());
                metadata.insert("policy".to_string(), collaboration.policy.clone());
            }
            append_event(
                &mut store,
                &runtime.task_id,
                EventKind::ModelRequestStarted,
                "Agent model turn started",
                metadata_with_context(metadata, &run_context),
            )
            .map_err(|error| error.to_string())?;
        }

        let visible_stream = active_collaboration.is_none();
        let mut streamed_output = false;
        let mut partial_stream = String::new();
        let mut stream_progress = ModelStreamProgress::new();
        let mut first_delta_at_ms = None;
        let mut transport_attempt = 0usize;
        let mut response = loop {
            transport_attempt += 1;
            partial_stream.clear();
            stream_progress.reset();
            let result = provider.complete_streaming_cancellable(
                request.clone(),
                |delta| {
                    if !delta.is_empty() {
                        first_delta_at_ms.get_or_insert_with(current_time_millis);
                        partial_stream.push_str(delta);
                        stream_progress.observe(
                            cancellation,
                            "model_stream",
                            "executor",
                            &partial_stream,
                        );
                    }
                    if visible_stream && !delta.is_empty() {
                        streamed_output = true;
                        emit_agent_stream_delta(
                            app,
                            &request_id,
                            session_id,
                            delta,
                            false,
                            false,
                            None,
                        );
                    }
                },
                || agent_run_should_stop(cancellation) || cancellation.has_pending_steer(),
            );
            match result {
                Ok(response) => break response,
                Err(error) => {
                    if error.is_cancelled()
                        && cancellation.has_pending_steer()
                        && !agent_run_should_stop(cancellation)
                    {
                        if visible_stream && streamed_output {
                            emit_agent_stream_delta(
                                app,
                                &request_id,
                                session_id,
                                "",
                                false,
                                true,
                                None,
                            );
                        }
                        cancellation.finish_model_call();
                        continue 'agent_loop;
                    }
                    if error.is_cancelled() || agent_run_should_stop(cancellation) {
                        emit_agent_stream_delta(app, &request_id, session_id, "", true, true, None);
                        cancellation.finish_model_call();
                        return pause_agent_loop_for_control_stop(
                            app,
                            state,
                            workspace_root,
                            &runtime,
                            &prompt,
                            &run_context,
                            active_collaboration,
                            cancellation,
                        );
                    }
                    if transport_attempt < MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS
                        && error.is_retryable()
                        && cancellation
                            .begin_repair_attempt("provider_transport_retry")
                            .is_ok()
                    {
                        if visible_stream && streamed_output {
                            emit_agent_stream_delta(
                                app,
                                &request_id,
                                session_id,
                                "",
                                false,
                                true,
                                None,
                            );
                            streamed_output = false;
                        }
                        cancellation.mark_progress("model_retry", "Transient provider failure");
                        append_agent_progress_event(
                            state,
                            &runtime.task_id,
                            &run_context,
                            &format!(
                                "Retrying model request ({}/{})",
                                transport_attempt + 1,
                                MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS
                            ),
                        )?;
                        std::thread::sleep(model_transport_retry_delay(transport_attempt));
                        continue;
                    }
                    if !partial_stream.trim().is_empty() {
                        cancellation.record_partial_output(&partial_stream);
                    }
                    cancellation.finish_model_call();
                    cancellation.record_observation(
                        "provider_failure",
                        error.class.label(),
                        &error.message,
                    );
                    if let Some(reason) = exhausted_model_transport_error_stop_reason(&error) {
                        cancellation.request_stop(reason);
                        return pause_agent_loop_for_control_stop(
                            app,
                            state,
                            workspace_root,
                            &runtime,
                            &prompt,
                            &run_context,
                            active_collaboration,
                            cancellation,
                        );
                    }
                    return agent_state_with_error_in_context(
                        state,
                        &run_context,
                        format!("Agent model call failed: {error}"),
                    );
                }
            }
        };
        cancellation.finish_model_call();
        if cancellation.record_agent_turn("executor").is_err() {
            if !partial_stream.trim().is_empty() {
                cancellation.record_partial_output(&partial_stream);
            }
            return pause_agent_loop_for_control_stop(
                app,
                state,
                workspace_root,
                &runtime,
                &prompt,
                &run_context,
                active_collaboration,
                cancellation,
            );
        }
        let raw_response_content = response.message.content.clone();
        response.message.content = sanitize_assistant_content(&raw_response_content);
        let reasoning_markup_removed = response.message.content != raw_response_content.trim();
        if visible_stream && streamed_output && reasoning_markup_removed {
            emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
            streamed_output = false;
        }
        if !response.message.content.trim().is_empty() {
            cancellation.record_partial_output(&response.message.content);
        }
        if let Some(evidence) = model_response_checkpoint_evidence(&response) {
            cancellation.record_observation("model_result", "executor", &evidence);
        }
        let should_synthesize = active_collaboration.is_some()
            && response.tool_calls.is_empty()
            && !response.message.content.trim().is_empty();
        if should_synthesize {
            response
                .message
                .metadata
                .insert("internal".to_string(), "true".to_string());
            response.message.metadata.insert(
                "collaboration_stage".to_string(),
                "executor_draft".to_string(),
            );
        }
        let latency_ms = current_time_millis().saturating_sub(started_at_ms);
        let output_length = response.message.content.len();
        let tool_call_count = response.tool_calls.len();
        if visible_stream && streamed_output && tool_call_count > 0 {
            emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
            streamed_output = false;
        }
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            if agent_task_is_cancelled(&mut store, session_id).map_err(|error| error.to_string())? {
                return agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string());
            }
            let mut metadata = Metadata::new();
            metadata.insert("request_id".to_string(), request_id.clone());
            metadata.insert("latency_ms".to_string(), latency_ms.to_string());
            if let Some(first_delta_at_ms) = first_delta_at_ms {
                metadata.insert(
                    "first_token_latency_ms".to_string(),
                    first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
                );
            }
            metadata.insert("output_length".to_string(), output_length.to_string());
            metadata.insert("tool_calls".to_string(), tool_call_count.to_string());
            metadata.insert(
                "transport_attempts".to_string(),
                transport_attempt.to_string(),
            );
            metadata.insert(
                "context_projected_tokens".to_string(),
                context_governor.estimated_projected_tokens.to_string(),
            );
            let progress = cancellation.progress();
            metadata.insert(
                "run_checkpoints".to_string(),
                progress.checkpoints.to_string(),
            );
            metadata.insert(
                "run_observations".to_string(),
                progress.observations.to_string(),
            );
            metadata.insert(
                "run_budget_extensions".to_string(),
                progress.budget_extensions.to_string(),
            );
            metadata.insert(
                "run_model_call_limit".to_string(),
                progress.model_call_limit.to_string(),
            );
            metadata.insert(
                "run_tool_call_limit".to_string(),
                progress.tool_call_limit.to_string(),
            );
            metadata.insert(
                "run_agent_turns".to_string(),
                progress.agent_turns.to_string(),
            );
            metadata.insert(
                "run_agent_turn_limit".to_string(),
                progress.agent_turn_limit.to_string(),
            );
            metadata.insert(
                "run_repair_attempts".to_string(),
                progress.repair_attempts.to_string(),
            );
            for key in [
                "prompt_tokens",
                "completion_tokens",
                "total_tokens",
                "usage_source",
                "usage_estimated",
            ] {
                if let Some(value) = response.metadata.get(key) {
                    metadata.insert(key.to_string(), value.clone());
                }
            }
            if let Some(raw_tool_calls_json) = response.raw_tool_calls_json.clone() {
                metadata.insert("raw_tool_calls_json".to_string(), raw_tool_calls_json);
            }
            if let Some(collaboration) = active_collaboration {
                metadata.insert("collaboration_id".to_string(), collaboration.id.clone());
                metadata.insert("stage".to_string(), "executor".to_string());
                metadata.insert("role".to_string(), "executor".to_string());
                metadata.insert("policy".to_string(), collaboration.policy.clone());
            }
            append_event(
                &mut store,
                &runtime.task_id,
                EventKind::ModelRequestFinished,
                "Agent model turn finished",
                metadata_with_context(metadata, &run_context),
            )
            .map_err(|error| error.to_string())?;
        }

        let previous_message_count = runtime.messages.len();
        let advance = AgentKernel::new(&mut runtime, &tools).advance_model_response(response);
        if matches!(&advance, AgentAdvance::Completed { .. })
            && !required_image_generation_satisfied(&runtime, &run_context)
        {
            runtime.messages.truncate(previous_message_count);
            runtime.messages.push(Message {
                role: MessageRole::System,
                content: "The task cannot complete yet: the authoritative image-generation policy requires a successful `image.generate` call using the user's configured backend. Call that tool now; do not substitute another implementation.".to_string(),
                metadata: [
                    ("internal".to_string(), "true".to_string()),
                    ("kind".to_string(), "image_generation_policy".to_string()),
                ]
                .into_iter()
                .collect(),
            });
            continue;
        }
        if matches!(&advance, AgentAdvance::Completed { .. }) {
            let verification_required = run_context
                .get("verification_required")
                .is_some_and(|value| value == "true");
            let verification_instruction =
                AgentKernel::new(&mut runtime, &tools).completion_gate(verification_required);
            if let Some(instruction) = verification_instruction {
                runtime.messages.truncate(previous_message_count);
                AgentKernel::new(&mut runtime, &tools).apply_instruction(&instruction);
                cancellation.mark_progress(
                    "verification",
                    "Waiting for post-change verification evidence",
                );
                continue;
            }
        }
        {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            persist_new_runtime_messages(
                &mut store,
                &runtime.task_id,
                &runtime.messages,
                previous_message_count,
                &run_context,
            )
            .map_err(|error| error.to_string())?;
        }

        match advance {
            AgentAdvance::Completed { answer } => {
                clear_suspended_agent_run_for_context(state, &run_context)?;
                let (completion_evidence, routing_learning_eligible) =
                    completion_learning_signal(&runtime);
                let completion_evidence_count = runtime
                    .messages
                    .iter()
                    .filter(|message| matches!(message.role, MessageRole::Tool))
                    .count();
                cancellation.record_best_known_result(
                    "executor",
                    &answer,
                    if completion_evidence_count > 0 {
                        ResultQuality::Grounded
                    } else {
                        ResultQuality::Substantive
                    },
                    completion_evidence_count,
                    false,
                    true,
                );
                let (final_answer, synthesized) = if let Some(collaboration) = active_collaboration
                {
                    match synthesize_agent_answer(
                        app,
                        state,
                        config,
                        &runtime,
                        &prompt,
                        &answer,
                        &run_context,
                        collaboration,
                        cancellation,
                    ) {
                        Ok(answer) => (answer, true),
                        Err(_)
                            if cancellation.has_pending_steer()
                                && !agent_run_should_stop(cancellation) =>
                        {
                            emit_agent_stream_delta(
                                app,
                                &request_id,
                                session_id,
                                "",
                                false,
                                true,
                                None,
                            );
                            active_collaboration = None;
                            continue 'agent_loop;
                        }
                        Err(_) if agent_run_should_stop(cancellation) => {
                            return pause_agent_loop_for_control_stop(
                                app,
                                state,
                                workspace_root,
                                &runtime,
                                &prompt,
                                &run_context,
                                Some(collaboration),
                                cancellation,
                            );
                        }
                        Err(_) => {
                            emit_agent_stream_delta(
                                app,
                                &request_id,
                                session_id,
                                "",
                                false,
                                true,
                                None,
                            );
                            emit_agent_stream_delta(
                                app,
                                &request_id,
                                session_id,
                                &answer,
                                false,
                                false,
                                None,
                            );
                            (answer.clone(), false)
                        }
                    }
                } else {
                    if !streamed_output && !answer.trim().is_empty() {
                        emit_agent_stream_delta(
                            app,
                            &request_id,
                            session_id,
                            &answer,
                            false,
                            false,
                            None,
                        );
                    }
                    (answer.clone(), false)
                };
                cancellation.record_best_known_result(
                    if synthesized {
                        "synthesizer"
                    } else if completion_evidence_count > 0 {
                        "verified_executor"
                    } else {
                        "executor"
                    },
                    &final_answer,
                    if synthesized {
                        ResultQuality::Synthesized
                    } else if completion_evidence_count > 0 {
                        ResultQuality::Verified
                    } else {
                        ResultQuality::Substantive
                    },
                    completion_evidence_count,
                    completion_evidence_count > 0,
                    true,
                );
                let completion_progress = cancellation.progress();
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                if synthesized {
                    append_message_event_with_metadata(
                        &mut store,
                        &runtime.task_id,
                        MessageRole::Assistant,
                        &final_answer,
                        metadata_with_context(
                            [
                                ("collaboration_final".to_string(), "true".to_string()),
                                (
                                    "model".to_string(),
                                    config.model_for_role(&ModelRole::Summarizer),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                            &run_context,
                        ),
                    )
                    .map_err(|error| error.to_string())?;
                }
                append_event(
                    &mut store,
                    &runtime.task_id,
                    EventKind::TaskStatusChanged,
                    "Agent task completed",
                    metadata_with_context(
                        [
                            ("answer_length".to_string(), final_answer.len().to_string()),
                            (
                                "collaboration".to_string(),
                                active_collaboration.is_some().to_string(),
                            ),
                            (
                                "collaboration_synthesized".to_string(),
                                synthesized.to_string(),
                            ),
                            (
                                "elapsed_ms".to_string(),
                                completion_progress.elapsed.as_millis().to_string(),
                            ),
                            (
                                "model_calls".to_string(),
                                completion_progress.model_calls.to_string(),
                            ),
                            (
                                "tool_calls".to_string(),
                                completion_progress.tool_calls.to_string(),
                            ),
                            (
                                "agent_turns".to_string(),
                                completion_progress.agent_turns.to_string(),
                            ),
                            (
                                "repair_attempts".to_string(),
                                completion_progress.repair_attempts.to_string(),
                            ),
                            (
                                "completion_evidence".to_string(),
                                completion_evidence.to_string(),
                            ),
                            (
                                "routing_learning_eligible".to_string(),
                                routing_learning_eligible.to_string(),
                            ),
                            (
                                "verification_gate_requests".to_string(),
                                runtime.verification_gate_requests.to_string(),
                            ),
                            (
                                "interaction_verification_gate_requests".to_string(),
                                runtime.interaction_verification_gate_requests.to_string(),
                            ),
                            (
                                "verified_interactions".to_string(),
                                runtime.verified_interactions.to_string(),
                            ),
                            (
                                "pending_interaction_verifications".to_string(),
                                runtime.pending_interaction_verifications.len().to_string(),
                            ),
                            (
                                "checkpoints".to_string(),
                                completion_progress.checkpoints.to_string(),
                            ),
                            (
                                "observations".to_string(),
                                completion_progress.observations.to_string(),
                            ),
                            (
                                "budget_extensions".to_string(),
                                completion_progress.budget_extensions.to_string(),
                            ),
                            ("last_stage".to_string(), completion_progress.stage),
                        ]
                        .into_iter()
                        .collect(),
                        &run_context,
                    ),
                )
                .map_err(|error| error.to_string())?;
                if let Err(error) = record_project_memory_observed_use(
                    &mut store,
                    &runtime.task_id,
                    &run_context,
                    &final_answer,
                ) {
                    eprintln!("project memory utilization unavailable: {error}");
                }
                let memory_ledger =
                    match refresh_project_memory_after_completion(&mut store, &run_context) {
                        Ok(ledger) => ledger,
                        Err(error) => {
                            eprintln!("project memory checkpoint unavailable: {error}");
                            None
                        }
                    };
                let completed_state = agent_state_for_session(&store, None, session_id)
                    .map_err(|error| error.to_string())?;
                drop(store);
                emit_agent_stream_delta(app, &request_id, session_id, "", true, false, None);
                if let Some(ledger) = memory_ledger {
                    schedule_project_memory_vector_refresh(
                        workspace_root.to_path_buf(),
                        config.clone(),
                        ledger,
                    );
                }
                return Ok(completed_state);
            }
            AgentAdvance::TurnBudgetExhausted { partial_answer, .. } => {
                if let Some(partial_answer) = partial_answer {
                    cancellation.record_partial_output(&partial_answer);
                }
                cancellation.request_stop(RunStopReason::TurnBudgetExhausted);
                return pause_agent_loop_for_control_stop(
                    app,
                    state,
                    workspace_root,
                    &runtime,
                    &prompt,
                    &run_context,
                    active_collaboration,
                    cancellation,
                );
            }
            AgentAdvance::Retry { instruction } => {
                if visible_stream && streamed_output {
                    emit_agent_stream_delta(app, &request_id, session_id, "", false, true, None);
                }
                let previous_message_count = runtime.messages.len();
                AgentKernel::new(&mut runtime, &tools).apply_model_response_retry(instruction);
                let mut store = state
                    .store
                    .lock()
                    .map_err(|error| format!("store lock poisoned: {error}"))?;
                persist_new_runtime_messages(
                    &mut store,
                    &runtime.task_id,
                    &runtime.messages,
                    previous_message_count,
                    &run_context,
                )
                .map_err(|error| error.to_string())?;
                drop(store);
                cancellation.mark_progress("model_retry", "Recovering incomplete model response");
                continue;
            }
            AgentAdvance::Failed { message } => {
                clear_suspended_agent_run_for_context(state, &run_context)?;
                return agent_state_with_error_in_context(state, &run_context, message);
            }
            AgentAdvance::ToolCalls { calls } => {
                if cancellation.has_pending_steer() && !agent_run_should_stop(cancellation) {
                    active_collaboration = None;
                    continue 'agent_loop;
                }
                let mut waiting_for_permission = false;
                for call in calls {
                    if cancellation.has_pending_steer() && !agent_run_should_stop(cancellation) {
                        active_collaboration = None;
                        continue 'agent_loop;
                    }
                    if agent_run_should_stop(cancellation) {
                        return pause_agent_loop_for_control_stop(
                            app,
                            state,
                            workspace_root,
                            &runtime,
                            &prompt,
                            &run_context,
                            active_collaboration,
                            cancellation,
                        );
                    }
                    let invocation = AgentKernel::new(&mut runtime, &tools).tool_invocation(&call);
                    let mut store = state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    append_tool_proposed_event(&mut store, &invocation, Some(&run_context))
                        .map_err(|error| error.to_string())?;

                    if AgentKernel::new(&mut runtime, &tools).repeated_tool_failure_count(&call)
                        >= MAX_IDENTICAL_TOOL_FAILURES
                    {
                        let observation = observation_from_tool_result(
                            &call.tool_name,
                            "failed",
                            "Cindx blocked this identical tool call after repeated failures. Change the arguments or use a different approach.",
                        );
                        append_tool_finished_event(
                            &mut store,
                            &runtime.task_id,
                            &call.call_id.0,
                            &call.tool_name,
                            "failed",
                            &observation,
                            [(
                                "failure_code".to_string(),
                                "repeated_call_blocked".to_string(),
                            )]
                            .into_iter()
                            .collect(),
                            Some(&run_context),
                        )
                        .map_err(|error| error.to_string())?;
                        let previous_message_count = runtime.messages.len();
                        AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
                            &call,
                            &ToolOutcomeStatus::Failed,
                            None,
                            &observation,
                        );
                        persist_new_runtime_messages(
                            &mut store,
                            &runtime.task_id,
                            &runtime.messages,
                            previous_message_count,
                            &run_context,
                        )
                        .map_err(|error| error.to_string())?;
                        continue;
                    }

                    let Some(tool) = registry.get(&call.tool_name) else {
                        let observation = observation_from_tool_result(
                            &call.tool_name,
                            "failed",
                            "Unknown tool requested by model.",
                        );
                        append_tool_finished_event(
                            &mut store,
                            &runtime.task_id,
                            &call.call_id.0,
                            &call.tool_name,
                            "failed",
                            &observation,
                            Metadata::new(),
                            Some(&run_context),
                        )
                        .map_err(|error| error.to_string())?;
                        let previous_message_count = runtime.messages.len();
                        AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
                            &call,
                            &ToolOutcomeStatus::Failed,
                            None,
                            &observation,
                        );
                        persist_new_runtime_messages(
                            &mut store,
                            &runtime.task_id,
                            &runtime.messages,
                            previous_message_count,
                            &run_context,
                        )
                        .map_err(|error| error.to_string())?;
                        continue;
                    };
                    let tool_risk = tool.spec().risk;

                    if let Some(mut request) = tool.permission_request(&invocation) {
                        request.id = PermissionRequestId(unique_id("agent-perm"));
                        request
                            .metadata
                            .insert("phase".to_string(), "16".to_string());
                        request
                            .metadata
                            .entry("tool_input".to_string())
                            .or_insert_with(|| invocation.input_json.clone());
                        request
                            .metadata
                            .insert("tool_call_id".to_string(), invocation.id.0.clone());
                        request
                            .metadata
                            .entry("tool_name".to_string())
                            .or_insert_with(|| invocation.tool_name.clone());
                        request
                            .metadata
                            .insert("agent_prompt".to_string(), prompt.clone());
                        for (key, value) in &run_context {
                            request
                                .metadata
                                .entry(key.clone())
                                .or_insert_with(|| value.clone());
                        }
                        if !agent_session_permission_granted(
                            &store,
                            &phase16_task_id(),
                            &request,
                            session_id,
                        )
                        .map_err(|error| error.to_string())?
                        {
                            store
                                .save_permission_request(request.clone(), current_time_millis())
                                .map_err(|error| error.to_string())?;
                            append_event(
                                &mut store,
                                &runtime.task_id,
                                EventKind::PermissionRequested,
                                format!("Agent permission requested for {}", request.action),
                                metadata_with_context(
                                    [
                                        ("permission_id".to_string(), request.id.0),
                                        ("tool_call_id".to_string(), invocation.id.0),
                                        ("tool".to_string(), request.action),
                                        (
                                            "risk".to_string(),
                                            permission_risk_label(&request.risk).to_string(),
                                        ),
                                        ("scope".to_string(), request.scope),
                                        ("agent_prompt".to_string(), prompt.clone()),
                                    ]
                                    .into_iter()
                                    .collect(),
                                    &run_context,
                                ),
                            )
                            .map_err(|error| error.to_string())?;
                            waiting_for_permission = true;
                            continue;
                        }
                        append_event(
                            &mut store,
                            &runtime.task_id,
                            EventKind::PermissionResolved,
                            format!("Session permission reused for {}", request.action),
                            metadata_with_context(
                                [
                                    ("decision".to_string(), "allow_for_session".to_string()),
                                    ("tool_call_id".to_string(), invocation.id.0.clone()),
                                    ("tool".to_string(), request.action),
                                    ("scope".to_string(), request.scope),
                                ]
                                .into_iter()
                                .collect(),
                                &run_context,
                            ),
                        )
                        .map_err(|error| error.to_string())?;
                    }

                    let tool_name = invocation.tool_name.clone();
                    drop(store);
                    let result = execute_agent_tool_invocation(
                        state,
                        &registry,
                        invocation,
                        workspace_root,
                        &run_context,
                    )?;
                    let observation = observation_from_agent_tool_result(&tool_name, &result);
                    let image_paths = tool_result_image_paths(&result);
                    let previous_message_count = runtime.messages.len();
                    AgentKernel::new(&mut runtime, &tools).apply_tool_observation(
                        &call,
                        &result.status,
                        Some(&tool_risk),
                        &observation,
                    );
                    append_visual_reference_message(&mut runtime, &tool_name, &image_paths);
                    let mut store = state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    persist_new_runtime_messages(
                        &mut store,
                        &runtime.task_id,
                        &runtime.messages,
                        previous_message_count,
                        &run_context,
                    )
                    .map_err(|error| error.to_string())?;
                    drop(store);
                    if cancellation.has_pending_steer() && !agent_run_should_stop(cancellation) {
                        active_collaboration = None;
                        continue 'agent_loop;
                    }
                    if agent_run_should_stop(cancellation) {
                        return pause_agent_loop_for_control_stop(
                            app,
                            state,
                            workspace_root,
                            &runtime,
                            &prompt,
                            &run_context,
                            active_collaboration,
                            cancellation,
                        );
                    }
                }
                if waiting_for_permission {
                    let mut store = state
                        .store
                        .lock()
                        .map_err(|error| format!("store lock poisoned: {error}"))?;
                    let events = agent_events_for_session(&store, &phase16_task_id(), session_id)
                        .map_err(|error| error.to_string())?;
                    let active_events = active_agent_events_for_session(&events, session_id);
                    let recovery_metadata = agent_recovery_metadata(
                        &active_events,
                        &run_context,
                        "blocked",
                        "waiting_for_permission",
                        Metadata::new(),
                    )?;
                    append_event(
                        &mut store,
                        &runtime.task_id,
                        EventKind::TaskStatusChanged,
                        "Agent task waiting for permission",
                        recovery_metadata,
                    )
                    .map_err(|error| error.to_string())?;
                    remember_suspended_agent_run(
                        state,
                        SuspendedAgentRun {
                            runtime: runtime.clone(),
                            prompt: prompt.clone(),
                            run_context: run_context.clone(),
                            workspace_root: workspace_root.to_path_buf(),
                            collaboration: active_collaboration.cloned(),
                            run_control: cancellation.snapshot(),
                        },
                    )?;
                    return agent_state_for_session(&store, None, session_id)
                        .map_err(|error| error.to_string());
                }
            }
        }
    }
}
