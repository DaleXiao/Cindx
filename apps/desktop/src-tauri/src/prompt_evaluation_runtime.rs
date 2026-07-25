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
    let routing = RoutingContext::from_prompt(objective, Vec::new());
    let contract_policy = parse_policy(policy).unwrap_or(OrchestrationPolicy::Single);
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
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            effort,
            contract_policy,
        ),
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
                let fallback = harness.fallback_plan();
                return PromptPlanCandidate {
                    genome: genome.clone(),
                    plan: fallback.clone().ok(),
                    raw_output: match fallback {
                        Ok(_) => format!(
                            "{raw_output}\n\nValidation error: {error}\nDeterministic harness fallback applied."
                        ),
                        Err(fallback_error) => format!(
                            "{raw_output}\n\nValidation error: {error}\nDeterministic fallback error: {fallback_error}"
                        ),
                    },
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

pub(crate) fn prompt_evaluation_stage_class(
    step: &orchestrator::WorkflowPlanStep,
) -> RunStageClass {
    match step.role.as_str() {
        "verifier" | "reviewer" => RunStageClass::Reviewer,
        "synthesizer" => RunStageClass::Synthesizer,
        _ if step.access.is_empty() => RunStageClass::Candidate,
        _ => RunStageClass::Worker,
    }
}

fn prompt_evaluation_retry_allowed(error: &str, branch_cancelled: bool) -> bool {
    !branch_cancelled
        && error != MODEL_REQUEST_CANCELLED
        && !error.contains(RunStopReason::StageBudgetExhausted.code())
        && !error.contains("stopped before model call")
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
        "You are executing one node in an isolated Cindx Conductor evaluation. The workspace boundary is read-only and no external side effects are allowed. {tool_contract} Never claim an action that is absent from your tool results. Produce the strongest work product possible from the objective, authorized dependency outputs, and verified read-only evidence. State uncertainty rather than inventing evidence. Put the decisive conclusion or requested output first, then add only the bounded support needed by downstream nodes; never postpone the deliverable until the end.\n\nObjective:\n{objective}\n\nYour role: {}\nYour subtask:\n{}\n\nAuthorized dependency outputs:\n{dependencies}",
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

fn prompt_evaluation_partial_output(
    runtime: &AgentLoopState,
    fallback: Option<&str>,
) -> Option<String> {
    let accumulated = runtime
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let partial = if accumulated.is_empty() {
        fallback.unwrap_or_default().trim().to_string()
    } else {
        accumulated
    };
    (!partial.is_empty()).then(|| truncate_for_collaboration(&partial, 12_000))
}

pub(crate) fn complete_prompt_evaluation_worker(
    config: &ProviderConfig,
    workspace_root: &Path,
    request: PromptEvaluationWorkerRequest,
    control: &Arc<AgentRunControl>,
    branch_cancellation: &Arc<AtomicBool>,
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
        if control.should_stop()
            || control.stage_should_stop(request.stage_class)
            || branch_cancellation.load(Ordering::Acquire)
        {
            return CollaborationCompletion {
                content: None,
                error: Some(MODEL_REQUEST_CANCELLED.to_string()),
                latency_ms: current_time_millis().saturating_sub(started_at_ms),
                usage,
                evidence,
            };
        }
        if let Err(reason) = control.begin_stage_model_call(&request.stage, request.stage_class) {
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
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            model: request.model.clone(),
            embedding_model: config.model_for_role(&ModelRole::Embedder),
            timeout_seconds: control.stage_model_call_timeout_seconds(request.stage_class),
        });
        let finalizing =
            prepare_collaboration_worker_turn(&mut runtime, has_tools, evidence_turn_limit);
        let request_tools: &[ToolSpec] = if finalizing { &[] } else { &tools };
        let max_output_tokens = bounded_max_output_tokens(
            config.context_window_tokens,
            request
                .max_output_tokens
                .clamp(256, COLLABORATION_MAX_OUTPUT_TOKENS),
        );
        let prepared_turn = AgentKernel::new(&mut runtime, request_tools).prepare_model_turn(
            Some(&config.agent_system_prompt),
            Some(&trusted_context),
            config.context_window_tokens,
            max_output_tokens,
        );
        let mut model_request = prepared_turn.request;
        let governor = prepared_turn.context;
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
        let mut streamed_output = String::new();
        let response = provider.complete_streaming_cancellable(
            model_request,
            |delta| streamed_output.push_str(delta),
            || {
                control.should_stop()
                    || control.stage_should_stop(request.stage_class)
                    || branch_cancellation.load(Ordering::Acquire)
            },
        );
        control.finish_model_call();
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return CollaborationCompletion {
                    content: (!streamed_output.trim().is_empty())
                        .then(|| truncate_for_collaboration(streamed_output.trim(), 12_000)),
                    error: Some(error.to_string()),
                    latency_ms: current_time_millis().saturating_sub(started_at_ms),
                    usage,
                    evidence,
                }
            }
        };
        for key in [
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "usage_source",
            "usage_estimated",
        ] {
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
        match AgentKernel::new(&mut runtime, request_tools).advance_model_response(response) {
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
                partial_answer,
            } => {
                return CollaborationCompletion {
                    content: prompt_evaluation_partial_output(
                        &runtime,
                        partial_answer.as_deref(),
                    ),
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
                AgentKernel::new(&mut runtime, request_tools)
                    .apply_model_response_retry(instruction);
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
                        let mut invocation =
                            AgentKernel::new(&mut runtime, request_tools).tool_invocation(&call);
                        invocation.proposed_by_model = "prompt-evaluation-worker".to_string();
                        invocation.metadata.insert(
                            "evaluation_sandbox".to_string(),
                            "read_only_v2".to_string(),
                        );
                        let tool_control = ToolExecutionControl::new({
                            let control = Arc::clone(control);
                            let branch_cancellation = Arc::clone(branch_cancellation);
                            move || {
                                control.should_stop()
                                    || branch_cancellation.load(Ordering::Acquire)
                            }
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
        Arc::new(move |request, branch_cancellation| {
            complete_prompt_evaluation_worker(
                &config,
                &workspace_root,
                request,
                &control,
                &branch_cancellation,
            )
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
                quality_gate_met: false,
                final_output: String::new(),
                steps: Vec::new(),
                latency_ms: 0,
                total_tokens: 0,
            },
        };
    };
    if adaptive_workflow_layers(&plan.adaptive_workflow()).is_err() {
        return PromptExecutionCandidate {
            plan: candidate,
            execution: PromptWorkflowExecution {
                succeeded: false,
                quality_gate_met: false,
                final_output: String::new(),
                steps: Vec::new(),
                latency_ms: 0,
                total_tokens: 0,
            },
        };
    }

    let mut outputs = BTreeMap::<String, String>::new();
    let mut execution_steps = Vec::new();
    let mut quality_gate_met = true;
    let mut checkpoint = WorkflowExecutionCheckpoint::new(
        format!("evaluation-{}", plan.workflow_id),
        plan.clone(),
        started_at_ms,
    );
    let retry_policy = candidate.genome.retry_policy;
    let max_attempts = candidate.genome.max_step_attempts.max(1);
    let routing = RoutingContext::from_prompt(objective, Vec::new());
    let policy = if plan.policy == "direct" {
        OrchestrationPolicy::Single
    } else {
        parse_policy(&plan.policy).unwrap_or(OrchestrationPolicy::Single)
    };
    let execution_contract =
        ConductorExecutionContract::from_routing(&routing, &plan.effort, policy)
            .with_prompt_commit_strategy(candidate.genome.commit_strategy);
    let alternate_models = plan
        .steps
        .iter()
        .map(|step| step.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    loop {
        let frontier = match checkpoint.execution_frontier(1) {
            Ok(frontier) => frontier,
            Err(_) => break,
        };
        if frontier.is_complete(plan.steps.len()) {
            break;
        }
        let runnable = frontier.runnable_steps();
        if runnable.is_empty() {
            break;
        }
        let layer = runnable
            .iter()
            .filter_map(|step_id| plan.steps.iter().position(|step| &step.id == step_id))
            .collect::<Vec<_>>();
        if checkpoint
            .claim_steps(&runnable, 1, current_time_millis())
            .is_err()
        {
            break;
        }
        let jobs = layer
            .iter()
            .filter_map(|index| plan.steps.get(*index).cloned().map(|step| (*index, step)))
            .map(|(index, step)| {
                let initial_prompt = prompt_evaluation_step_prompt(objective, &step, &outputs);
                let stage = format!("prompt_evaluation_{}", step.id);
                let stage_class = prompt_evaluation_stage_class(&step);
                let runner = Arc::clone(&runner);
                let alternate_models = alternate_models.clone();
                let max_model_turns = step
                    .tool_policy
                    .effective_model_turn_budget(plan.budget.max_model_turns_per_step);
                let max_tool_calls = step
                    .tool_policy
                    .effective_tool_call_budget(plan.budget.max_tool_calls_per_step);
                let max_output_tokens = plan.budget.max_output_tokens_per_step as u64;
                Box::new(move |branch_cancellation| {
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
                    let mut best_partial_output = None;
                    while attempts < max_attempts {
                        attempts += 1;
                        prompts.push(format!("attempt {attempts} model={model}\n{prompt}"));
                        let completion = runner(
                            PromptEvaluationWorkerRequest {
                                stage: stage.clone(),
                                stage_class,
                                role: role.clone(),
                                model: model.clone(),
                                prompt: prompt.clone(),
                                tool_policy: step.tool_policy.clone(),
                                max_model_turns,
                                max_tool_calls,
                                max_output_tokens,
                            },
                            Arc::clone(&branch_cancellation),
                        );
                        latency_ms = latency_ms.saturating_add(completion.latency_ms);
                        total_tokens = total_tokens.saturating_add(
                            completion
                                .usage
                                .get("total_tokens")
                                .and_then(|value| value.parse::<u64>().ok())
                                .unwrap_or_default(),
                        );
                        evidence.extend(completion.evidence);
                        let content = completion
                            .content
                            .filter(|content| !content.trim().is_empty());
                        let completion_error = completion.error;
                        if completion_error.is_none() {
                            if let Some(content) = content {
                                final_output = Some(content);
                                break;
                            }
                        } else if let Some(content) = content {
                            best_partial_output = Some(content);
                        }
                        let error = completion_error
                            .unwrap_or_else(|| "empty worker output".to_string());
                        errors.push(error.clone());
                        if attempts >= max_attempts
                            || retry_policy == PromptRetryPolicy::FailFast
                            || !prompt_evaluation_retry_allowed(
                                &error,
                                branch_cancellation.load(Ordering::Acquire),
                            )
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
                            "Retry the same authorized evaluation node after a failed attempt. Correct the failure without widening scope or claiming unavailable actions. Preserve useful partial work, but independently verify it before completing the node.\n\nFailure:\n{}\n\nUseful incomplete work:\n{}\n\nOriginal node contract:\n{}",
                            truncate_for_collaboration(&error, 2_000),
                            best_partial_output
                                .as_deref()
                                .map(|partial| truncate_for_collaboration(partial, 6_000))
                                .unwrap_or_else(|| "(none)".to_string()),
                            initial_prompt
                        );
                    }
                    let succeeded = final_output.is_some();
                    let output = final_output.or(best_partial_output).unwrap_or_else(|| {
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
                }) as CancellableParallelJob<(usize, PromptExecutionStep)>
            })
            .collect::<Vec<_>>();
        let independent_layer = layer.iter().all(|index| {
            plan.steps
                .get(*index)
                .is_some_and(|step| step.access.is_empty())
        });
        let required_successes = if independent_layer
            && execution_contract.stop_policy != ConductorStopPolicy::Exhaustive
        {
            execution_contract.required_successes_for_layer(jobs.len())
        } else {
            jobs.len().max(1)
        };
        let execution = run_model_jobs_until_quorum_interruptible(
            "prompt-evaluation",
            jobs,
            required_successes,
            Duration::from_millis(execution_contract.quorum_grace_ms()),
            Duration::from_millis(20),
            |(_, step)| step.succeeded,
            || false,
        )
        .execution;
        let quorum_reached = execution.quorum_reached;
        let layer_steps = layer
            .iter()
            .filter_map(|index| plan.steps.get(*index).map(|step| (*index, step)))
            .collect::<Vec<_>>();
        let mut completed = execution
            .results
            .into_iter()
            .enumerate()
            .map(|(slot, result)| {
                result.unwrap_or_else(|error| {
                    let (index, step) = layer_steps
                        .get(slot)
                        .copied()
                        .expect("parallel result preserves its input slot");
                    (
                        index,
                        PromptExecutionStep {
                            id: step.id.clone(),
                            role: step.role.clone(),
                            model: step.model.clone(),
                            prompt: prompt_evaluation_step_prompt(objective, step, &outputs),
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
        let successful_steps = completed.iter().filter(|(_, step)| step.succeeded).count();
        let substantive_steps = completed
            .iter()
            .filter(|(_, step)| {
                !step.output.trim().is_empty() && !step.output.starts_with("[execution failed:")
            })
            .count();
        let layer_gate_met = successful_steps >= required_successes;
        quality_gate_met &= layer_gate_met;
        let can_continue_degraded = substantive_steps > 0 || !outputs.is_empty();
        for (_, step) in completed {
            let now_ms = current_time_millis();
            let _ = checkpoint.record_step_metrics(&step.id, step.latency_ms, step.total_tokens);
            if step.succeeded {
                let _ = checkpoint.complete_step(
                    &step.id,
                    &step.model,
                    step.output.clone(),
                    "[]".to_string(),
                    now_ms,
                );
            } else {
                let error = step
                    .errors
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "evaluation worker failed".to_string());
                let _ = checkpoint.fail_step(&step.id, &error, now_ms);
                if step.role != "synthesizer"
                    && can_continue_degraded
                    && (quorum_reached || !layer_gate_met)
                {
                    let _ = checkpoint.degrade_step(&step.id, step.output.clone(), error, now_ms);
                }
            }
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
    let final_step_succeeded = plan.steps.last().is_some_and(|final_step| {
        execution_steps
            .iter()
            .find(|step| step.id == final_step.id)
            .is_some_and(|step| step.succeeded)
    });
    let final_deliverable =
        !final_output.trim().is_empty() && !final_output.starts_with("[execution failed:");
    let succeeded = final_deliverable;
    quality_gate_met &= final_step_succeeded;
    PromptExecutionCandidate {
        plan: candidate,
        execution: PromptWorkflowExecution {
            succeeded,
            quality_gate_met,
            final_output,
            total_tokens: execution_steps.iter().map(|step| step.total_tokens).sum(),
            steps: execution_steps,
            latency_ms: current_time_millis().saturating_sub(started_at_ms),
        },
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
