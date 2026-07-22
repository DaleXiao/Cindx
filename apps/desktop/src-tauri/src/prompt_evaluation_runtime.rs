use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_conductor_prompt_profile(
    config: &ProviderConfig,
    objective: &str,
    effort: &str,
    policy: &str,
    worker_models: &[String],
    agent_budget: usize,
    genome: &ConductorPromptGenome,
    evaluation_id: &str,
    control: &Arc<AgentRunControl>,
) -> PromptPlanCandidate {
    let conductor_model = config.model_for_conductor();
    let harness = ConductorHarness::new(ConductorRequest {
        workflow_id: format!("{evaluation_id}-{}", genome.id),
        objective: objective.to_string(),
        recent_context: String::new(),
        effort: effort.to_string(),
        policy: policy.to_string(),
        conductor_model: conductor_model.clone(),
        worker_models: worker_models.to_vec(),
        role_hints: collaboration_role_hints(config, worker_models),
        budget: WorkflowBudget {
            max_steps: adaptive_workflow_step_budget(agent_budget),
            max_models: agent_budget.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS),
            max_model_turns_per_step: DEFAULT_COLLABORATION_WORKER_TURNS,
            max_tool_calls_per_step: MAX_COLLABORATION_WORKER_TOOL_CALLS,
            max_output_tokens_per_step: COLLABORATION_MAX_OUTPUT_TOKENS as usize,
        },
        prior_hint: None,
        prompt_evolution_enabled: true,
        prompt_genome: genome.clone(),
    });
    let system_prompt =
        collaboration_system_prompt_for_run(&config.agent_system_prompt, &Metadata::new());
    evaluate_conductor_prompt_profile_with_runner(&harness, genome, |prompt| {
        complete_collaboration_model_with_control(
            config.clone(),
            ModelRole::Planner,
            conductor_model.clone(),
            system_prompt.clone(),
            prompt,
            Some(control.clone()),
            |_| {},
        )
    })
}

pub(crate) fn evaluate_conductor_prompt_profile_with_runner<F>(
    harness: &ConductorHarness,
    genome: &ConductorPromptGenome,
    mut runner: F,
) -> PromptPlanCandidate
where
    F: FnMut(String) -> CollaborationCompletion,
{
    let mut prompt = harness.planning_prompt();
    let mut attempts = 0usize;
    let mut latency_ms = 0u64;
    let mut total_tokens = 0u64;
    loop {
        attempts += 1;
        let completion = runner(prompt);
        latency_ms = latency_ms.saturating_add(completion.latency_ms);
        total_tokens = total_tokens.saturating_add(
            completion
                .usage
                .get("total_tokens")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default(),
        );
        let raw_output = completion.content.unwrap_or_else(|| {
            completion
                .error
                .unwrap_or_else(|| "empty response".to_string())
        });
        match harness.parse_plan(&raw_output) {
            Ok(plan) => {
                return PromptPlanCandidate {
                    genome: genome.clone(),
                    plan: Some(plan),
                    raw_output,
                    latency_ms,
                    total_tokens,
                };
            }
            Err(error) if attempts < CONDUCTOR_MAX_ATTEMPTS => {
                prompt = harness.repair_prompt(&raw_output, &error);
            }
            Err(error) => {
                return PromptPlanCandidate {
                    genome: genome.clone(),
                    plan: None,
                    raw_output: format!("{raw_output}\n\nValidation error: {error}"),
                    latency_ms,
                    total_tokens,
                };
            }
        }
    }
}

pub(crate) fn prompt_evaluation_role(role: &str) -> ModelRole {
    match role {
        "verifier" | "reviewer" => ModelRole::Reviewer,
        "synthesizer" => ModelRole::Summarizer,
        "worker" => ModelRole::Executor,
        _ => ModelRole::Planner,
    }
}

pub(crate) fn prompt_evaluation_step_prompt(
    objective: &str,
    step: &orchestrator::WorkflowPlanStep,
    outputs: &BTreeMap<String, String>,
) -> String {
    let dependencies = if step.access.is_empty() {
        "(none; solve this branch independently)".to_string()
    } else {
        step.access
            .iter()
            .map(|dependency| {
                format!(
                    "[{dependency}]\n{}",
                    outputs
                        .get(dependency)
                        .map(String::as_str)
                        .unwrap_or("[dependency unavailable]")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let tool_contract = match step.tool_policy {
        WorkflowToolPolicy::None => {
            "No tools are available. Reason only from the objective and authorized dependencies."
        }
        WorkflowToolPolicy::ReadOnlyEvidence => {
            "Read-only workspace tools are available only to verify concrete evidence required by this subtask."
        }
        WorkflowToolPolicy::ReadOnlyExploration => {
            "Read-only workspace tools are available for bounded exploration. Writes, processes, browser, computer, and network actions are unavailable."
        }
    };
    format!(
        "You are executing one node in an isolated Cindx Conductor evaluation. The workspace boundary is read-only and no external side effects are allowed. {tool_contract} Never claim an action that is absent from your tool results. Produce the strongest work product possible from the objective, authorized dependency outputs, and verified read-only evidence. State uncertainty rather than inventing evidence.\n\nObjective:\n{objective}\n\nYour role: {}\nYour subtask:\n{}\n\nAuthorized dependency outputs:\n{dependencies}",
        step.role, step.subtask
    )
}

pub(crate) fn prompt_evaluation_tool_specs(
    registry: &ToolRegistry,
    prompt: &str,
    context_window_tokens: u64,
    policy: &WorkflowToolPolicy,
) -> Vec<ToolSpec> {
    if *policy == WorkflowToolPolicy::None {
        return Vec::new();
    }
    evidence_worker_tools(&registry.exposure_plan(prompt, context_window_tokens).inline)
}

pub(crate) fn prompt_evaluation_tool_traces(
    evidence: &[CollaborationEvidence],
) -> Vec<AgentEvaluationToolTrace> {
    evidence
        .iter()
        .map(|entry| AgentEvaluationToolTrace {
            tool: entry.tool_name.clone(),
            request: entry.request.clone(),
            response: entry.output.clone(),
            error: (entry.status != "succeeded").then(|| entry.output.clone()),
        })
        .collect()
}

pub(crate) fn complete_prompt_evaluation_worker(
    config: &ProviderConfig,
    workspace_root: &Path,
    request: PromptEvaluationWorkerRequest,
    control: &Arc<AgentRunControl>,
) -> CollaborationCompletion {
    let started_at_ms = current_time_millis();
    let registry = ToolRegistry::with_workspace_tools(workspace_root.to_path_buf());
    let tools = prompt_evaluation_tool_specs(
        &registry,
        &request.prompt,
        config.context_window_tokens,
        &request.tool_policy,
    );
    let has_tools = !tools.is_empty();
    let evidence_turn_limit = request.max_model_turns.max(1);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: request.model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: control.model_call_timeout_seconds(),
    });
    let mut runtime = start_agent_loop(
        TaskId(unique_id("prompt-evaluation-worker")),
        request.prompt.clone(),
        AgentRuntimeConfig {
            max_turns: collaboration_worker_runtime_turn_limit(evidence_turn_limit, has_tools),
        },
    );
    let trusted_context = format!(
        "This is a GEPA evaluation sandbox backed by the active workspace through read-only tools. Never request or imply writes, process execution, browser control, computer control, or network access. Treat tool outputs as the only external evidence. You have {evidence_turn_limit} tool-capable model rounds and {} total tool calls; batch decisive reads, avoid repeated directory listings, and stop gathering evidence once the assigned node is answerable. Return only the assigned node work product.",
        request.max_tool_calls,
    );
    let mut usage = Metadata::new();
    let mut evidence = Vec::new();
    let mut tool_call_count = 0usize;
    loop {
        if control.should_stop() {
            return CollaborationCompletion {
                content: None,
                error: Some(MODEL_REQUEST_CANCELLED.to_string()),
                latency_ms: current_time_millis().saturating_sub(started_at_ms),
                usage,
                evidence,
            };
        }
        if let Err(reason) = control.begin_model_call("prompt_evaluation_worker") {
            return CollaborationCompletion {
                content: None,
                error: Some(format!(
                    "evaluation worker stopped before model call: {}",
                    reason.code()
                )),
                latency_ms: current_time_millis().saturating_sub(started_at_ms),
                usage,
                evidence,
            };
        }
        let finalizing =
            prepare_collaboration_worker_turn(&mut runtime, has_tools, evidence_turn_limit);
        let request_tools: &[ToolSpec] = if finalizing { &[] } else { &tools };
        let max_output_tokens = bounded_max_output_tokens(
            config.context_window_tokens,
            COLLABORATION_MAX_OUTPUT_TOKENS,
        );
        let (mut model_request, governor) = model_request_for_turn_with_context_budget(
            &runtime,
            request_tools,
            Some(&config.agent_system_prompt),
            Some(&trusted_context),
            config.context_window_tokens,
            max_output_tokens,
        );
        model_request.role = request.role.clone();
        model_request.metadata.insert(
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
        model_request
            .metadata
            .insert("evaluation_sandbox".to_string(), "read_only_v2".to_string());
        let response = provider.complete_streaming_cancellable(
            model_request,
            |_| {},
            || control.should_stop(),
        );
        control.finish_model_call();
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return CollaborationCompletion {
                    content: None,
                    error: Some(error.to_string()),
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                }
            }
        };
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
        let finalization_content = finalizing
            .then(|| response.message.content.trim().to_string())
            .filter(|content| !content.is_empty());
        if let Some(evidence) = model_response_checkpoint_evidence(&response) {
            control.record_checkpoint("model_result", "prompt_evaluation_worker", &evidence);
        }
        match advance_with_model_response(&mut runtime, response, request_tools) {
            AgentAdvance::Completed { answer } => {
                usage.insert("worker_turns".to_string(), runtime.turn.to_string());
                usage.insert("worker_tool_calls".to_string(), tool_call_count.to_string());
                usage.insert(
                    "worker_runtime".to_string(),
                    "read_only_evaluation_v2".to_string(),
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
                        "evaluation_turn_budget_exhausted: completed {completed_turns} turns with a {max_turns}-turn budget"
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
                append_internal_instruction(&mut runtime, "empty_model_retry", &instruction);
                continue;
            }
            AgentAdvance::ToolCalls { calls } => {
                if finalizing {
                    if let Some(answer) = finalization_content {
                        usage.insert("worker_turns".to_string(), runtime.turn.to_string());
                        usage.insert(
                            "worker_tool_calls".to_string(),
                            tool_call_count.to_string(),
                        );
                        usage.insert(
                            "worker_runtime".to_string(),
                            "read_only_evaluation_v2".to_string(),
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
                            "evaluation worker requested another tool after its evidence phase; final answer was empty"
                                .to_string(),
                        ),
                        latency_ms: current_time_millis().saturating_sub(started_at_ms),
                        usage,
                        evidence,
                    };
                }
                for call in calls {
                    tool_call_count += 1;
                    let allowed = tool_call_count <= request.max_tool_calls
                        && tools.iter().any(|tool| {
                            tool.name == call.tool_name && tool.risk == ToolRisk::ReadOnly
                        });
                    let (status, output) = if !allowed {
                        (
                            ToolOutcomeStatus::Denied,
                            "Evaluation sandbox denied this tool: only exposed read-only tools within the per-step budget are allowed."
                                .to_string(),
                        )
                    } else {
                        let mut invocation = tool_invocation_from_request(&runtime.task_id, &call);
                        invocation.proposed_by_model = "prompt-evaluation-worker".to_string();
                        invocation.metadata.insert(
                            "evaluation_sandbox".to_string(),
                            "read_only_v2".to_string(),
                        );
                        let tool_control = ToolExecutionControl::new({
                            let control = Arc::clone(control);
                            move || control.should_stop()
                        });
                        match registry.get(&call.tool_name) {
                            Some(tool) if tool.spec().risk == ToolRisk::ReadOnly => {
                                match tool.execute_with_control(invocation, &tool_control) {
                                    Ok(result) => (result.status, result.output),
                                    Err(error) => (ToolOutcomeStatus::Failed, error.message),
                                }
                            }
                            _ => (
                                ToolOutcomeStatus::Denied,
                                "Evaluation sandbox rejected a non-read-only tool.".to_string(),
                            ),
                        }
                    };
                    record_tool_outcome(&mut runtime, &call.tool_name, &call.input, &status);
                    evidence.push(CollaborationEvidence {
                        source_step: "evaluation".to_string(),
                        tool_call_id: call.call_id.0.clone(),
                        tool_name: call.tool_name.clone(),
                        request: call.input.clone(),
                        status: tool_outcome_label(&status).to_string(),
                        output: truncate_for_collaboration(&output, 6_000),
                    });
                    let observation = observation_from_tool_result(
                        &call.tool_name,
                        tool_outcome_label(&status),
                        &output,
                    );
                    append_tool_observation(&mut runtime, call.call_id, &observation);
                }
            }
        }
    }
}

pub(crate) fn execute_prompt_workflow_candidate(
    config: &ProviderConfig,
    workspace_root: &Path,
    objective: &str,
    candidate: PromptPlanCandidate,
    control: &Arc<AgentRunControl>,
) -> PromptExecutionCandidate {
    let config = config.clone();
    let workspace_root = workspace_root.to_path_buf();
    let control = control.clone();
    execute_prompt_workflow_candidate_with_runner(
        objective,
        candidate,
        Arc::new(move |request| {
            complete_prompt_evaluation_worker(&config, &workspace_root, request, &control)
        }),
    )
}

pub(crate) fn execute_prompt_workflow_candidate_with_runner(
    objective: &str,
    candidate: PromptPlanCandidate,
    runner: PromptEvaluationRunner,
) -> PromptExecutionCandidate {
    let started_at_ms = current_time_millis();
    let Some(plan) = candidate.plan.as_ref() else {
        return PromptExecutionCandidate {
            plan: candidate,
            execution: PromptWorkflowExecution {
                succeeded: false,
                final_output: String::new(),
                steps: Vec::new(),
                latency_ms: 0,
                total_tokens: 0,
            },
        };
    };
    let Ok(layers) = adaptive_workflow_layers(&plan.adaptive_workflow()) else {
        return PromptExecutionCandidate {
            plan: candidate,
            execution: PromptWorkflowExecution {
                succeeded: false,
                final_output: String::new(),
                steps: Vec::new(),
                latency_ms: 0,
                total_tokens: 0,
            },
        };
    };

    let mut outputs = BTreeMap::<String, String>::new();
    let mut execution_steps = Vec::new();
    let retry_policy = candidate.genome.retry_policy;
    let max_attempts = candidate.genome.max_step_attempts.max(1);
    let alternate_models = plan
        .steps
        .iter()
        .map(|step| step.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for layer in layers {
        let jobs = layer
            .iter()
            .filter_map(|index| plan.steps.get(*index).cloned().map(|step| (*index, step)))
            .map(|(index, step)| {
                let initial_prompt = prompt_evaluation_step_prompt(objective, &step, &outputs);
                let runner = Arc::clone(&runner);
                let alternate_models = alternate_models.clone();
                let max_model_turns = plan.budget.max_model_turns_per_step;
                let max_tool_calls = plan.budget.max_tool_calls_per_step;
                Box::new(move || {
                    let role = prompt_evaluation_role(&step.role);
                    let mut model = step.model.clone();
                    let mut prompt = initial_prompt.clone();
                    let mut prompts = Vec::new();
                    let mut errors = Vec::new();
                    let mut evidence = Vec::new();
                    let mut latency_ms = 0u64;
                    let mut total_tokens = 0u64;
                    let mut attempts = 0usize;
                    let mut final_output = None;
                    while attempts < max_attempts {
                        attempts += 1;
                        prompts.push(format!("attempt {attempts} model={model}\n{prompt}"));
                        let completion = runner(PromptEvaluationWorkerRequest {
                            role: role.clone(),
                            model: model.clone(),
                            prompt: prompt.clone(),
                            tool_policy: step.tool_policy.clone(),
                            max_model_turns,
                            max_tool_calls,
                        });
                        latency_ms = latency_ms.saturating_add(completion.latency_ms);
                        total_tokens = total_tokens.saturating_add(
                            completion
                                .usage
                                .get("total_tokens")
                                .and_then(|value| value.parse::<u64>().ok())
                                .unwrap_or_default(),
                        );
                        evidence.extend(completion.evidence);
                        if let Some(content) = completion
                            .content
                            .filter(|content| !content.trim().is_empty())
                        {
                            final_output = Some(content);
                            break;
                        }
                        let error = completion
                            .error
                            .unwrap_or_else(|| "empty worker output".to_string());
                        errors.push(error.clone());
                        if attempts >= max_attempts
                            || retry_policy == PromptRetryPolicy::FailFast
                        {
                            break;
                        }
                        if retry_policy == PromptRetryPolicy::AlternateModel {
                            if let Some(alternate) = alternate_models
                                .iter()
                                .filter(|candidate| **candidate != model)
                                .nth((attempts - 1) % alternate_models.len().max(1))
                            {
                                model.clone_from(alternate);
                            }
                        }
                        prompt = format!(
                            "Retry the same authorized evaluation node after a failed attempt. Correct the failure without widening scope or claiming unavailable actions.\n\nFailure:\n{}\n\nOriginal node contract:\n{}",
                            truncate_for_collaboration(&error, 2_000),
                            initial_prompt
                        );
                    }
                    let succeeded = final_output.is_some();
                    let output = final_output.unwrap_or_else(|| {
                        format!(
                            "[execution failed: {}]",
                            errors
                                .last()
                                .map(String::as_str)
                                .unwrap_or("empty worker output")
                        )
                    });
                    let tool_calls = prompt_evaluation_tool_traces(&evidence);
                    (
                        index,
                        PromptExecutionStep {
                            id: step.id,
                            role: step.role,
                            model,
                            prompt: prompts.join("\n\n---\n\n"),
                            attempts,
                            succeeded,
                            output,
                            tool_calls,
                            errors,
                            latency_ms,
                            total_tokens,
                            evidence_count: step.access.len() + evidence.len(),
                        },
                    )
                }) as ParallelJob<(usize, PromptExecutionStep)>
            })
            .collect::<Vec<_>>();
        let mut completed = run_model_jobs_ordered("prompt-evaluation", jobs)
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|error| {
                    (
                        usize::MAX,
                        PromptExecutionStep {
                            id: "worker-panic".to_string(),
                            role: "worker".to_string(),
                            model: String::new(),
                            prompt: String::new(),
                            attempts: 1,
                            succeeded: false,
                            output: format!("[execution failed: {error}]"),
                            tool_calls: Vec::new(),
                            errors: vec![error.to_string()],
                            latency_ms: 0,
                            total_tokens: 0,
                            evidence_count: 0,
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        completed.sort_by_key(|(index, _)| *index);
        for (_, step) in completed {
            outputs.insert(step.id.clone(), step.output.clone());
            execution_steps.push(step);
        }
    }
    let final_output = plan
        .steps
        .last()
        .and_then(|step| outputs.get(&step.id))
        .cloned()
        .unwrap_or_default();
    let succeeded = !final_output.trim().is_empty()
        && execution_steps.len() == plan.steps.len()
        && execution_steps.iter().all(|step| step.succeeded);
    PromptExecutionCandidate {
        plan: candidate,
        execution: PromptWorkflowExecution {
            succeeded,
            final_output,
            total_tokens: execution_steps.iter().map(|step| step.total_tokens).sum(),
            steps: execution_steps,
            latency_ms: current_time_millis().saturating_sub(started_at_ms),
        },
    }
}

pub(crate) fn evaluate_prompt_candidate_pair(
    config: &ProviderConfig,
    objective: &str,
    candidate_a: &PromptExecutionCandidate,
    candidate_b: &PromptExecutionCandidate,
    evaluation_id: &str,
    control: &Arc<AgentRunControl>,
) -> Result<PromptPairwiseEvaluationPayload, String> {
    let reviewer_model = config.model_for_role(&ModelRole::Reviewer);
    let candidate_text = |candidate: &PromptExecutionCandidate| {
        let plan = candidate
            .plan
            .plan
            .as_ref()
            .and_then(|plan| plan.to_json().ok())
            .unwrap_or_else(|| truncate_for_collaboration(&candidate.plan.raw_output, 12_000));
        let steps = candidate
            .execution
            .steps
            .iter()
            .map(|step| {
                format!(
                    "step={} role={} model={} attempts={} succeeded={} latency_ms={} tokens={} tool_calls={} output:\n{}",
                    step.id,
                    step.role,
                    step.model,
                    step.attempts,
                    step.succeeded,
                    step.latency_ms,
                    step.total_tokens,
                    step.tool_calls.len(),
                    truncate_for_collaboration(&step.output, 6_000)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        format!(
            "plan:\n{plan}\n\nexecution_succeeded={} execution_latency_ms={} execution_tokens={}\n\nsteps:\n{}\n\nfinal_output:\n{}",
            candidate.execution.succeeded,
            candidate.execution.latency_ms,
            candidate.execution.total_tokens,
            steps,
            truncate_for_collaboration(&candidate.execution.final_output, 12_000)
        )
    };
    let prompt = format!(
        "Blindly compare two actually executed Cindx Conductor workflows for the same objective. Judge the final work product first, then factual grounding, dependency use, verification quality, completeness, efficiency, recoverability, and safety. Workers ran against the same read-only workspace sandbox; reward claims grounded in recorded tool evidence and penalize claims of unavailable writes, processes, browser, computer, or network actions. Do not prefer A or B by position. Give each exact step id a 0..1 credit. For each candidate, return actionable natural-language diagnostics grounded in its trajectory: what passed, what failed, concrete errors, and generalizable changes. Never include secrets or copy benchmark answers into suggested changes. Invalid plans, failed executions, unsafe outputs, or empty final outputs must receive a low score. Return only strict JSON with this schema: {{\"score_a\":0.0,\"score_b\":0.0,\"safety_violations_a\":0,\"safety_violations_b\":0,\"step_scores_a\":{{\"step-id\":0.0}},\"step_scores_b\":{{\"step-id\":0.0}},\"feedback_a\":{{\"summary\":\"\",\"passed_constraints\":[],\"failed_constraints\":[],\"errors\":[],\"suggested_changes\":[]}},\"feedback_b\":{{\"summary\":\"\",\"passed_constraints\":[],\"failed_constraints\":[],\"errors\":[],\"suggested_changes\":[]}}}}.\n\nObjective:\n{}\n\nCandidate A (format_valid={}):\n{}\n\nCandidate B (format_valid={}):\n{}",
        objective,
        candidate_a.plan.plan.is_some(),
        candidate_text(candidate_a),
        candidate_b.plan.plan.is_some(),
        candidate_text(candidate_b),
    );
    let completion = complete_collaboration_model_with_control(
        config.clone(),
        ModelRole::Reviewer,
        reviewer_model,
        collaboration_system_prompt_for_run(&config.agent_system_prompt, &Metadata::new()),
        prompt,
        Some(control.clone()),
        |_| {},
    );
    let response = completion.content.ok_or_else(|| {
        completion
            .error
            .unwrap_or_else(|| format!("pairwise reviewer {evaluation_id} returned no content"))
    })?;
    let start = response
        .find('{')
        .ok_or_else(|| "pairwise reviewer did not return JSON".to_string())?;
    let end = response
        .rfind('}')
        .filter(|end| *end >= start)
        .ok_or_else(|| "pairwise reviewer returned incomplete JSON".to_string())?;
    let payload = serde_json::from_str::<PromptPairwiseEvaluationPayload>(&response[start..=end])
        .map_err(|error| format!("pairwise reviewer JSON is invalid: {error}"))?;
    if !payload.score_a.is_finite() || !payload.score_b.is_finite() {
        return Err("pairwise reviewer returned a non-finite score".to_string());
    }
    Ok(payload)
}

pub(crate) fn redact_prompt_evaluation_trace(
    trace: &mut AgentEvaluationTrace,
    redaction_secrets: &[String],
) {
    trace.input = redact_sensitive_text(&trace.input);
    trace.final_output = redact_sensitive_text(&trace.final_output);
    trace.actionable_feedback.summary = redact_sensitive_text(&trace.actionable_feedback.summary);
    for entry in trace
        .actionable_feedback
        .passed_constraints
        .iter_mut()
        .chain(trace.actionable_feedback.failed_constraints.iter_mut())
        .chain(trace.actionable_feedback.errors.iter_mut())
        .chain(trace.actionable_feedback.suggested_changes.iter_mut())
    {
        *entry = redact_sensitive_text(entry);
    }
    for check in &mut trace.verifier.checks {
        check.detail = redact_sensitive_text(&check.detail);
    }
    for step in &mut trace.steps {
        step.prompt = redact_sensitive_text(&step.prompt);
        step.output = redact_sensitive_text(&step.output);
        for error in &mut step.errors {
            *error = redact_sensitive_text(error);
        }
        for call in &mut step.tool_calls {
            call.request = redact_sensitive_text(&call.request);
            call.response = redact_sensitive_text(&call.response);
            if let Some(error) = &mut call.error {
                *error = redact_sensitive_text(error);
            }
        }
    }
    trace.apply_redaction(redaction_secrets);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt_pairwise_observation(
    candidate: &PromptExecutionCandidate,
    opponent: &PromptExecutionCandidate,
    objective: &str,
    evaluation_id: &str,
    task_class: &str,
    split: PromptEvaluationSplit,
    mode: PromptEvaluationMode,
    score: f64,
    opponent_score: f64,
    safety_violations: u64,
    step_scores: &BTreeMap<String, f64>,
    mut actionable_feedback: ActionableSideInformation,
    redaction_secrets: &[String],
) -> PromptEvolutionObservation {
    let score = score.clamp(0.0, 1.0);
    let succeeded = candidate.execution.succeeded && safety_violations == 0 && score >= 0.5;
    let case_digest = sha256_hex(objective.as_bytes());
    let case_id = format!("runtime-{task_class}-{}", &case_digest[..16]);
    let step_credits = candidate.plan.plan.as_ref().map_or_else(
        || {
            vec![PromptStepCredit {
                step_id: "plan_format".to_string(),
                role: "conductor".to_string(),
                succeeded: false,
                attempts: 1,
                evidence_count: 0,
                latency_ms: candidate.plan.latency_ms,
                total_tokens: candidate.plan.total_tokens,
                credit: 0.0,
            }]
        },
        |plan| {
            plan.steps
                .iter()
                .map(|step| PromptStepCredit {
                    step_id: step.id.clone(),
                    role: step.role.clone(),
                    succeeded: candidate
                        .execution
                        .steps
                        .iter()
                        .find(|executed| executed.id == step.id)
                        .is_some_and(|executed| executed.succeeded),
                    attempts: candidate
                        .execution
                        .steps
                        .iter()
                        .find(|executed| executed.id == step.id)
                        .map(|executed| executed.attempts)
                        .unwrap_or(1),
                    evidence_count: candidate
                        .execution
                        .steps
                        .iter()
                        .find(|executed| executed.id == step.id)
                        .map(|executed| executed.evidence_count)
                        .unwrap_or_default(),
                    latency_ms: candidate
                        .execution
                        .steps
                        .iter()
                        .find(|executed| executed.id == step.id)
                        .map(|executed| executed.latency_ms)
                        .unwrap_or_default(),
                    total_tokens: candidate
                        .execution
                        .steps
                        .iter()
                        .find(|executed| executed.id == step.id)
                        .map(|executed| executed.total_tokens)
                        .unwrap_or_default(),
                    credit: step_scores
                        .get(&step.id)
                        .copied()
                        .unwrap_or(score)
                        .clamp(0.0, 1.0),
                })
                .collect()
        },
    );
    if actionable_feedback.summary.trim().is_empty() {
        actionable_feedback.summary = format!("pairwise reviewer score {score:.3}");
    }
    if !succeeded && actionable_feedback.failed_constraints.is_empty() {
        actionable_feedback
            .failed_constraints
            .push("the executed workflow did not meet the pairwise quality gate".to_string());
    }
    let reflection_packet = (mode == PromptEvaluationMode::PairedExecution).then(|| {
        let verifier = AgentEvaluationVerifierOutcome {
            source: AgentEvaluationEvidenceSource::Judge,
            passed: succeeded,
            score,
            checks: vec![AgentEvaluationCheck {
                id: "pairwise_quality".to_string(),
                passed: succeeded,
                detail: actionable_feedback.summary.clone(),
            }],
        };
        let model_fingerprints = candidate
            .execution
            .steps
            .iter()
            .map(|step| (step.id.clone(), step.model.clone()))
            .collect::<BTreeMap<_, _>>();
        let candidate_fingerprint = serde_json::to_vec(&candidate.plan.genome)
            .map(|genome| sha256_hex(&genome))
            .unwrap_or_else(|_| candidate.plan.genome.id.clone());
        let mut trace = AgentEvaluationTrace {
            schema: AGENT_EVALUATION_TRACE_SCHEMA.to_string(),
            suite_id: "runtime-prompt-evolution".to_string(),
            suite_version: 2,
            case_id: case_id.clone(),
            category: task_class.to_string(),
            split: AgentEvaluationSplit::Feedback,
            run_id: evaluation_id.to_string(),
            seed: 0,
            candidate_id: candidate.plan.genome.id.clone(),
            candidate_fingerprint,
            model_fingerprints,
            input: objective.to_string(),
            steps: candidate
                .execution
                .steps
                .iter()
                .map(|step| AgentEvaluationTraceStep {
                    step_id: step.id.clone(),
                    role: step.role.clone(),
                    model: step.model.clone(),
                    prompt: step.prompt.clone(),
                    output: step.output.clone(),
                    tool_calls: step.tool_calls.clone(),
                    errors: step.errors.clone(),
                    latency_ms: step.latency_ms,
                    total_tokens: step.total_tokens,
                })
                .collect(),
            final_output: candidate.execution.final_output.clone(),
            verifier,
            actionable_feedback: actionable_feedback.clone(),
            latency_ms: candidate
                .plan
                .latency_ms
                .saturating_add(candidate.execution.latency_ms),
            total_tokens: candidate
                .plan
                .total_tokens
                .saturating_add(candidate.execution.total_tokens),
            safety_violations,
            redaction_applied: false,
        };
        redact_prompt_evaluation_trace(&mut trace, redaction_secrets);
        trace
            .reflection_packet()
            .expect("fresh feedback trace satisfies the reflection boundary")
    });
    PromptEvolutionObservation {
        profile_id: candidate.plan.genome.id.clone(),
        evaluation_id: evaluation_id.to_string(),
        case_id,
        opponent_profile_id: Some(opponent.plan.genome.id.clone()),
        task_class: task_class.to_string(),
        split,
        mode,
        format_valid: candidate.plan.plan.is_some(),
        succeeded,
        quality_score: score,
        latency_ms: candidate
            .plan
            .latency_ms
            .saturating_add(candidate.execution.latency_ms),
        total_tokens: candidate
            .plan
            .total_tokens
            .saturating_add(candidate.execution.total_tokens),
        estimated_cost_microusd: 0,
        safety_violations,
        relative_reward: Some((score - opponent_score.clamp(0.0, 1.0)).clamp(-1.0, 1.0)),
        step_credits,
        reflection_packet,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_prompt_evaluation_status(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    summary: &str,
    evaluation_id: &str,
    effort: &str,
    mode: PromptEvaluationMode,
    profile_a: &str,
    profile_b: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let mut metadata = [
        ("background_evaluation".to_string(), "true".to_string()),
        ("evaluation_id".to_string(), evaluation_id.to_string()),
        ("prompt_effort".to_string(), effort.to_string()),
        (
            "evaluation_mode".to_string(),
            serde_json::to_value(mode)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "paired_shadow".to_string()),
        ),
        ("profile_a".to_string(), profile_a.to_string()),
        ("profile_b".to_string(), profile_b.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(error) = error {
        metadata.insert(
            "error".to_string(),
            truncate_for_collaboration(error, 2_000),
        );
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        summary,
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn append_prompt_pairwise_observations(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    mode: PromptEvaluationMode,
    observations: [&PromptEvolutionObservation; 2],
    genomes: [&ConductorPromptGenome; 2],
) -> Result<(), String> {
    let encoded_observations = serde_json::to_string(&observations)
        .map_err(|error| format!("prompt observations serialization failed: {error}"))?;
    let encoded_genomes = serde_json::to_string(&genomes)
        .map_err(|error| format!("prompt genomes serialization failed: {error}"))?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Conductor pairwise evaluation",
        metadata_with_context(
            [
                ("background_evaluation".to_string(), "true".to_string()),
                (
                    "evaluation_id".to_string(),
                    observations[0].evaluation_id.clone(),
                ),
                ("prompt_effort".to_string(), effort.to_string()),
                (
                    "prompt_profile_a".to_string(),
                    observations[0].profile_id.clone(),
                ),
                (
                    "prompt_profile_b".to_string(),
                    observations[1].profile_id.clone(),
                ),
                ("prompt_genomes".to_string(), encoded_genomes),
                (
                    "evaluation_mode".to_string(),
                    match mode {
                        PromptEvaluationMode::Live => "live",
                        PromptEvaluationMode::PairedShadow => "paired_shadow",
                        PromptEvaluationMode::ReplayHoldout => "replay_holdout",
                        PromptEvaluationMode::PairedExecution => "paired_execution",
                        PromptEvaluationMode::ReplayExecution => "replay_execution",
                    }
                    .to_string(),
                ),
                ("prompt_observations".to_string(), encoded_observations),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}
