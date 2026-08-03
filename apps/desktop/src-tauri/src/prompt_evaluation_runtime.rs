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
    let effective_genome = genome.clone().with_effort_delivery_contract(effort);
    let conductor_model = config.model_for_conductor();
    let routing = RoutingContext::from_prompt(objective, Vec::new());
    let contract_policy = parse_policy(policy).unwrap_or(OrchestrationPolicy::Single);
    let harness = ConductorHarness::new(ConductorRequest {
        workflow_id: format!("{evaluation_id}-{}", effective_genome.id),
        objective: objective.to_string(),
        recent_context: String::new(),
        effort: effort.to_string(),
        policy: policy.to_string(),
        conductor_model: conductor_model.clone(),
        primary_model: worker_models
            .iter()
            .find(|model| **model == config.model_for_role(&ModelRole::Executor))
            .cloned()
            .or_else(|| worker_models.first().cloned())
            .unwrap_or_default(),
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
        prompt_genome: effective_genome.clone(),
    });
    let system_prompt =
        collaboration_system_prompt_for_run(&config.agent_system_prompt, &Metadata::new());
    evaluate_conductor_prompt_profile_with_runner(&harness, &effective_genome, |prompt| {
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
    match step.contract.output_kind {
        orchestrator::WorkflowOutputKind::Synthesis => RunStageClass::Finalizer,
        orchestrator::WorkflowOutputKind::Verification => RunStageClass::Reviewer,
        _ => match step.role.as_str() {
            "verifier" | "reviewer" => RunStageClass::Reviewer,
            "synthesizer" => RunStageClass::Synthesizer,
            _ if step.access.is_empty() => RunStageClass::Candidate,
            _ => RunStageClass::Worker,
        },
    }
}

pub(crate) fn prompt_evaluation_retry_allowed(
    failure: &AgentFailure,
    branch_cancelled: bool,
) -> bool {
    !branch_cancelled
        && failure.class != AgentFailureClass::Cancelled
        && failure.code != MODEL_REQUEST_CANCELLED
        && failure.code != RunStopReason::StageBudgetExhausted.code()
        && !failure.message.contains("stopped before model call")
}

fn prompt_evaluation_elapsed_ms(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn prompt_evaluation_step_prompt(
    objective: &str,
    step: &orchestrator::WorkflowPlanStep,
    outputs: &BTreeMap<String, PromptDependencyOutput>,
    missing_dependencies: &[String],
) -> String {
    let required_dependencies = step
        .access
        .iter()
        .chain(step.contract.input_steps.iter())
        .collect::<BTreeSet<_>>();
    let dependencies = if required_dependencies.is_empty() {
        "(none; solve this branch independently)".to_string()
    } else {
        required_dependencies
            .iter()
            .filter_map(|dependency| {
                outputs.get(*dependency).map(|output| {
                    let status = if output.completed() {
                        "completed"
                    } else {
                        "degraded_partial"
                    };
                    format!("[{dependency} status={status}]\n{}", output.content)
                })
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let missing = if missing_dependencies.is_empty() {
        "(none)".to_string()
    } else {
        missing_dependencies.join(", ")
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
        "You are executing one node in an isolated Cindx Conductor evaluation. The workspace boundary is read-only and no external side effects are allowed. {tool_contract} Never claim an action that is absent from your tool results. Produce the strongest work product possible from the objective, authorized dependency outputs, and verified read-only evidence. Inputs marked degraded_partial are real but incomplete; preserve useful claims while explicitly correcting or qualifying them. Missing dependency ids contain no usable content and must never be treated as evidence. State uncertainty rather than inventing evidence. Put the decisive conclusion or requested output first, then add only the bounded support needed by downstream nodes; never postpone the deliverable until the end.\n\nObjective:\n{objective}\n\nYour role: {}\nYour subtask:\n{}\n\nAuthorized dependency outputs:\n{dependencies}\n\nMissing dependency ids:\n{missing}",
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
    branch_cancellation: &Arc<AtomicBool>,
    objective_epoch: u64,
) -> CollaborationCompletion {
    let started_at = Instant::now();
    let stage_class = request.stage_class;
    let registry = ToolRegistry::with_workspace_tools(workspace_root.to_path_buf());
    let tools = prompt_evaluation_tool_specs(
        &registry,
        &request.prompt,
        config.context_window_tokens,
        &request.tool_policy,
    );
    let evidence_turn_limit = request.max_model_turns.max(1);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: request.model.clone(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: control.stage_model_call_timeout_seconds(stage_class),
    });
    let mut worker = IsolatedWorkerRuntime::new(
        TaskId(unique_id("prompt-evaluation-worker")),
        request.prompt.clone(),
        tools,
        evidence_turn_limit,
        request.max_tool_calls,
    );
    if request.tool_policy == WorkflowToolPolicy::ReadOnlyEvidence {
        worker.require_substantive_evidence();
    }
    let trusted_context = format!(
        "This is a GEPA evaluation sandbox backed by the active workspace through read-only tools. Never request or imply writes, process execution, browser control, computer control, or network access. Treat tool outputs as the only external evidence. You have {evidence_turn_limit} tool-capable model rounds and {} total tool calls; batch decisive reads, avoid repeated directory listings, and stop gathering evidence once the assigned node is answerable. Return only the assigned node work product.",
        request.max_tool_calls,
    );
    let mut evidence = Vec::new();
    loop {
        if branch_cancellation.load(Ordering::SeqCst) {
            return CollaborationCompletion::failed_worker(
                AgentFailure::cancelled(
                    "parallel_quorum_cancelled",
                    "evaluation branch cancelled after the layer reached an anytime quorum",
                ),
                None,
                prompt_evaluation_elapsed_ms(started_at),
                worker.completion_usage("read_only_evaluation_v3"),
                evidence,
            );
        }
        if let Some(reason) = control.stop_reason() {
            let failure = AgentFailure::from_stop_reason(
                reason,
                format!("evaluation worker stopped: {}", reason.code()),
            );
            return CollaborationCompletion::failed_worker(
                failure,
                None,
                prompt_evaluation_elapsed_ms(started_at),
                worker.completion_usage("read_only_evaluation_v3"),
                evidence,
            );
        }
        if !control.preparation_epoch_is_current(objective_epoch) {
            return CollaborationCompletion::failed_worker(
                AgentFailure::cancelled(
                    "user_steer",
                    "evaluation worker superseded by applied user steering",
                ),
                None,
                prompt_evaluation_elapsed_ms(started_at),
                worker.completion_usage("read_only_evaluation_v3"),
                evidence,
            );
        }
        if control.stage_should_stop(stage_class) {
            let failure = AgentFailure::from_stop_reason(
                RunStopReason::StageBudgetExhausted,
                "evaluation worker exhausted its stage time budget",
            );
            return CollaborationCompletion::failed_worker(
                failure,
                None,
                prompt_evaluation_elapsed_ms(started_at),
                worker.completion_usage("read_only_evaluation_v3"),
                evidence,
            );
        }
        let max_output_tokens = bounded_max_output_tokens(
            config.context_window_tokens,
            request.max_output_tokens.max(1),
        );
        let prepared_turn = match worker.prepare_model_turn(
            Some(&config.agent_system_prompt),
            Some(&trusted_context),
            config.context_window_tokens,
            max_output_tokens,
        ) {
            Ok(prepared_turn) => prepared_turn,
            Err(failed) => {
                if let Some(partial_answer) = failed.partial_content.as_ref() {
                    control.record_partial_output_at(objective_epoch, partial_answer);
                }
                return CollaborationCompletion::failed_worker(
                    failed.failure,
                    failed.partial_content,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                );
            }
        };
        match control.begin_stage_model_call_at(objective_epoch, &request.stage, stage_class) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return CollaborationCompletion::failed_worker(
                    AgentFailure::cancelled(
                        "user_steer",
                        "evaluation model call superseded by user steering",
                    ),
                    None,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                )
            }
            Err(reason) => {
                let failure = AgentFailure::from_stop_reason(
                    reason,
                    format!(
                        "evaluation worker stopped before model call: {}",
                        reason.code()
                    ),
                );
                return CollaborationCompletion::failed_worker(
                    failure,
                    None,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                );
            }
        }
        let mut model_request = prepared_turn.turn.request;
        model_request.role = request.role.clone();
        model_request.metadata.insert(
            "max_output_tokens".to_string(),
            max_output_tokens.to_string(),
        );
        model_request
            .metadata
            .insert("evaluation_sandbox".to_string(), "read_only_v2".to_string());
        let model_attempt = match crate::model_resource_runtime::ControlledModelAttempt::reserve_at(
            control,
            objective_epoch,
            &request.model,
            &model_request,
            stage_class,
        ) {
            Ok(Some(attempt)) => attempt,
            Ok(None) => {
                control.finish_model_call_at(objective_epoch);
                return CollaborationCompletion::failed_worker(
                    AgentFailure::cancelled(
                        "user_steer",
                        "evaluation provider dispatch superseded by user steering",
                    ),
                    None,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                );
            }
            Err(reason) => {
                control.finish_model_call_at(objective_epoch);
                return CollaborationCompletion::failed_worker(
                    AgentFailure::from_stop_reason(
                        reason,
                        format!(
                            "evaluation worker stopped before provider dispatch: {}",
                            reason.code()
                        ),
                    ),
                    None,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                );
            }
        };
        let mut streamed = String::new();
        let response = provider.complete_streaming_cancellable(
            model_request,
            |delta| {
                if !delta.is_empty() {
                    streamed.push_str(delta);
                }
            },
            || {
                branch_cancellation.load(Ordering::SeqCst)
                    || control.should_stop()
                    || !control.preparation_epoch_is_current(objective_epoch)
                    || control.stage_should_stop(stage_class)
            },
        );
        match &response {
            Ok(response) => {
                let _ = model_attempt.settle_response(response);
            }
            Err(_) => {
                let _ = model_attempt.settle_unknown();
            }
        }
        control.finish_model_call_at(objective_epoch);
        let mut response = match response {
            Ok(response) => response,
            Err(error) => {
                let failure = if branch_cancellation.load(Ordering::SeqCst) {
                    AgentFailure::cancelled(
                        "parallel_quorum_cancelled",
                        "evaluation branch cancelled after the layer reached an anytime quorum",
                    )
                } else if let Some(reason) = control.stop_reason() {
                    AgentFailure::from_stop_reason(
                        reason,
                        format!("evaluation worker stopped: {}", reason.code()),
                    )
                } else if !control.preparation_epoch_is_current(objective_epoch) {
                    AgentFailure::cancelled(
                        "user_steer",
                        "evaluation worker superseded by applied user steering",
                    )
                } else if control.stage_should_stop(stage_class) {
                    AgentFailure::from_stop_reason(
                        RunStopReason::StageBudgetExhausted,
                        "evaluation worker exhausted its stage time budget",
                    )
                } else {
                    AgentFailure::from_model_error(&error)
                };
                let partial_content = (!streamed.trim().is_empty()).then_some(streamed);
                if let Some(partial_answer) = partial_content.as_ref() {
                    control.record_partial_output_at(objective_epoch, partial_answer);
                }
                return CollaborationCompletion::failed_worker(
                    failure,
                    partial_content,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                );
            }
        };
        if !streamed.trim().is_empty() {
            response.message.content = streamed;
        }
        if let Some(evidence) = model_response_checkpoint_evidence(&response) {
            control.record_checkpoint_at(
                objective_epoch,
                "model_result",
                "prompt_evaluation_worker",
                &evidence,
            );
        }
        match worker.advance_model_response(response) {
            WorkerAdvance::Completed { answer } => {
                return CollaborationCompletion::completed_worker(
                    answer,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                );
            }
            WorkerAdvance::Failed(failed) => {
                if let Some(partial_answer) = failed.partial_content.as_ref() {
                    control.record_partial_output_at(objective_epoch, partial_answer);
                }
                return CollaborationCompletion::failed_worker(
                    failed.failure,
                    failed.partial_content,
                    prompt_evaluation_elapsed_ms(started_at),
                    worker.completion_usage("read_only_evaluation_v3"),
                    evidence,
                );
            }
            WorkerAdvance::Retry => continue,
            WorkerAdvance::ToolCalls { calls } => {
                for call in calls {
                    let admission = worker.admit_tool_call(&call);
                    let (status, output) = if let WorkerToolAdmission::Denied { reason, .. } =
                        admission
                    {
                        (ToolOutcomeStatus::Denied, reason.to_string())
                    } else {
                        let mut invocation = worker.tool_invocation(&call);
                        invocation.proposed_by_model = "prompt-evaluation-worker".to_string();
                        invocation
                            .metadata
                            .insert("evaluation_sandbox".to_string(), "read_only_v2".to_string());
                        let tool_control = ToolExecutionControl::new({
                            let control = Arc::clone(control);
                            let branch_cancellation = Arc::clone(branch_cancellation);
                            move || {
                                branch_cancellation.load(Ordering::SeqCst)
                                    || control.should_stop()
                                    || control.stage_should_stop(stage_class)
                            }
                        });
                        match registry.permissionless_read_tool(&invocation) {
                            Ok(tool) => match tool.execute_with_control(invocation, &tool_control) {
                                Ok(result) => (result.status, result.output),
                                Err(error) => (ToolOutcomeStatus::Failed, error.message),
                            },
                            Err(error) => (ToolOutcomeStatus::Denied, error.message),
                        }
                    };
                    evidence.push(CollaborationEvidence {
                        evidence_schema: String::new(),
                        steer_epoch: None,
                        collaboration_id: String::new(),
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
                    worker.apply_tool_observation(&call, &status, &observation);
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
    prompt_workflow_execution::execute_prompt_workflow_candidate_impl(
        config,
        workspace_root,
        objective,
        candidate,
        control,
    )
}

#[cfg(test)]
pub(crate) fn execute_prompt_workflow_candidate_with_runner(
    objective: &str,
    candidate: PromptPlanCandidate,
    runner: PromptEvaluationRunner,
) -> PromptExecutionCandidate {
    prompt_workflow_execution::execute_prompt_workflow_candidate_with_runner_impl(
        objective, candidate, runner, None, false,
    )
}

#[cfg(test)]
pub(crate) fn execute_prompt_workflow_candidate_with_direct_anchor_runner(
    objective: &str,
    candidate: PromptPlanCandidate,
    runner: PromptEvaluationRunner,
) -> PromptExecutionCandidate {
    prompt_workflow_execution::execute_prompt_workflow_candidate_with_runner_impl(
        objective, candidate, runner, None, true,
    )
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
            truncate_for_collaboration(&redact_sensitive_text(error), 2_000),
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
    if !observations
        .iter()
        .all(|observation| observation.is_strict_matched_transfer_evidence())
        || observations[0].provenance.matched_evaluation
            != observations[1].provenance.matched_evaluation
        || observations[0].profile_id
            != observations[1]
                .opponent_profile_id
                .as_deref()
                .unwrap_or_default()
        || observations[1].profile_id
            != observations[0]
                .opponent_profile_id
                .as_deref()
                .unwrap_or_default()
    {
        return Err("prompt transfer pair is not strict matched evidence".to_string());
    }
    let encoded_observations = serde_json::to_string(&observations)
        .map_err(|error| format!("prompt observations serialization failed: {error}"))?;
    let encoded_genomes = serde_json::to_string(&genomes)
        .map_err(|error| format!("prompt genomes serialization failed: {error}"))?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let model = load_prompt_evolution_read_model(&mut store).map_err(|error| error.to_string())?;
    let identity = observations[0]
        .provenance
        .matched_evaluation
        .as_ref()
        .ok_or_else(|| "prompt transfer pair is missing matched identity".to_string())?;
    let cohort = model
        .cohorts
        .get(&identity.cohort_sha256)
        .ok_or_else(|| "prompt transfer cohort is not persisted".to_string())?;
    let attempt = model
        .attempts
        .get(&identity.evaluation_id)
        .ok_or_else(|| "prompt transfer attempt is not persisted".to_string())?;
    if !crate::prompt_attempt_runtime::prompt_matched_identity_belongs_to_cohort(identity, cohort)
        || !observations.iter().all(|observation| {
            crate::prompt_attempt_runtime::prompt_observation_matches_attempt(
                observation,
                &attempt.started,
            )
        })
    {
        return Err("prompt transfer pair does not match its persisted attempt".to_string());
    }
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

pub(crate) fn append_prompt_transfer_observations(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    mode: PromptEvaluationMode,
    observations: [&PromptEvolutionObservation; 2],
) -> Result<(), String> {
    let encoded_observations = serde_json::to_string(&observations)
        .map_err(|error| format!("prompt transfer serialization failed: {error}"))?;
    let transfer = observations[0]
        .provenance
        .transfer
        .as_ref()
        .ok_or_else(|| "prompt transfer observation is missing provenance".to_string())?;
    if observations[1].provenance.transfer.as_ref() != Some(transfer) {
        return Err("prompt transfer pair has mismatched provenance".to_string());
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Conductor Auto transfer evaluation",
        metadata_with_context(
            [
                ("background_evaluation".to_string(), "true".to_string()),
                (
                    "evaluation_id".to_string(),
                    observations[0].evaluation_id.clone(),
                ),
                ("prompt_effort".to_string(), effort.to_string()),
                (
                    "prompt_profile".to_string(),
                    observations[0].profile_id.clone(),
                ),
                (
                    "auto_teacher_profile".to_string(),
                    transfer.source_profile_id.clone(),
                ),
                (
                    "auto_teacher_run_id".to_string(),
                    transfer.source_run_id.clone(),
                ),
                (
                    "evaluation_mode".to_string(),
                    match mode {
                        PromptEvaluationMode::PairedExecution => "paired_execution",
                        PromptEvaluationMode::ReplayExecution => "replay_execution",
                        PromptEvaluationMode::PairedShadow => "paired_shadow",
                        PromptEvaluationMode::ReplayHoldout => "replay_holdout",
                        PromptEvaluationMode::Live => "live",
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
