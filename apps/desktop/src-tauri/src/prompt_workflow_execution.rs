use super::*;

pub(super) fn execute_prompt_workflow_candidate_impl(
    config: &ProviderConfig,
    workspace_root: &Path,
    objective: &str,
    candidate: PromptPlanCandidate,
    control: &Arc<AgentRunControl>,
) -> PromptExecutionCandidate {
    let config = config.clone();
    let workspace_root = workspace_root.to_path_buf();
    let control = Arc::clone(control);
    execute_prompt_workflow_candidate_with_runner_impl(
        objective,
        candidate,
        Arc::new({
            let control = Arc::clone(&control);
            move |request, branch_cancellation| {
                complete_prompt_evaluation_worker(
                    &config,
                    &workspace_root,
                    request,
                    &control,
                    &branch_cancellation,
                )
            }
        }),
        Some(control),
    )
}

pub(super) fn execute_prompt_workflow_candidate_with_runner_impl(
    objective: &str,
    candidate: PromptPlanCandidate,
    runner: PromptEvaluationRunner,
    control: Option<Arc<AgentRunControl>>,
) -> PromptExecutionCandidate {
    let started_at = Instant::now();
    let Some(plan) = candidate.plan.clone() else {
        return failed_execution(candidate);
    };
    let routing = RoutingContext::from_prompt(objective, Vec::new());
    let policy = parse_policy(&plan.policy).unwrap_or(OrchestrationPolicy::Single);
    let execution_contract =
        ConductorExecutionContract::from_routing(&routing, &plan.effort, policy)
            .with_prompt_commit_strategy(candidate.genome.commit_strategy);
    let mut outputs = BTreeMap::<String, PromptDependencyOutput>::new();
    let mut execution_steps = Vec::new();
    let retry_policy = candidate.genome.retry_policy;
    let max_attempts = candidate.genome.max_step_attempts.max(1);
    let mut checkpoint = WorkflowExecutionCheckpoint::new(
        format!("prompt-evaluation-{}", candidate.genome.id),
        plan.clone(),
        current_time_millis(),
    );
    let alternate_models = plan
        .steps
        .iter()
        .map(|step| step.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    loop {
        let Ok(frontier) = checkpoint.execution_frontier_with_partial_recovery(max_attempts) else {
            return failed_execution(candidate);
        };
        if frontier.is_complete(plan.steps.len()) {
            break;
        }
        let runnable = frontier
            .runnable_steps()
            .into_iter()
            .collect::<BTreeSet<_>>();
        if runnable.is_empty() {
            for step in &plan.steps {
                if execution_steps
                    .iter()
                    .any(|executed: &PromptExecutionStep| executed.id == step.id)
                {
                    continue;
                }
                let unresolved = required_step_inputs(step)
                    .into_iter()
                    .filter(|dependency| !outputs.contains_key(dependency))
                    .collect::<Vec<_>>();
                execution_steps.push(unresolved_step(step.clone(), &unresolved));
            }
            break;
        }

        let scheduled = plan
            .steps
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, step)| runnable.contains(&step.id))
            .map(|(index, step)| {
                let required_inputs = required_step_inputs(&step);
                let unresolved = required_inputs
                    .iter()
                    .filter(|dependency| !outputs.contains_key(*dependency))
                    .cloned()
                    .collect::<Vec<_>>();
                let completed_input_count = required_inputs
                    .iter()
                    .filter(|dependency| {
                        outputs
                            .get(*dependency)
                            .is_some_and(PromptDependencyOutput::completed)
                    })
                    .count();
                let input_degraded = !unresolved.is_empty()
                    || required_inputs.iter().any(|dependency| {
                        outputs
                            .get(dependency)
                            .is_some_and(PromptDependencyOutput::degraded)
                    });
                let initial_prompt =
                    prompt_evaluation_step_prompt(objective, &step, &outputs, &unresolved);
                ScheduledPromptStep {
                    index,
                    step,
                    initial_prompt,
                    completed_input_count,
                    input_degraded,
                }
            })
            .collect::<Vec<_>>();

        let claim_ids = scheduled
            .iter()
            .map(|scheduled| scheduled.step.id.clone())
            .collect::<Vec<_>>();
        if checkpoint
            .claim_steps_with_partial_recovery(&claim_ids, max_attempts, current_time_millis())
            .is_err()
        {
            return failed_execution(candidate);
        }

        let jobs = scheduled
            .iter()
            .cloned()
            .map(|scheduled_step| {
                let runner = Arc::clone(&runner);
                let alternate_models = alternate_models.clone();
                let control = control.clone();
                let max_model_turns = scheduled_step
                    .step
                    .tool_policy
                    .effective_model_turn_budget(plan.budget.max_model_turns_per_step);
                let max_tool_calls = scheduled_step
                    .step
                    .tool_policy
                    .effective_tool_call_budget(plan.budget.max_tool_calls_per_step);
                let max_output_tokens = plan.budget.max_output_tokens_per_step as u64;
                Box::new(move |branch_cancellation| {
                    execute_step(ExecuteStepRequest {
                        step: scheduled_step.step,
                        initial_prompt: scheduled_step.initial_prompt,
                        runner,
                        control,
                        branch_cancellation,
                        alternate_models,
                        retry_policy,
                        max_attempts,
                        max_model_turns,
                        max_tool_calls,
                        max_output_tokens,
                        completed_input_count: scheduled_step.completed_input_count,
                        input_degraded: scheduled_step.input_degraded,
                    })
                }) as CancellableParallelJob<PromptExecutionStep>
            })
            .collect::<Vec<_>>();

        let layer_execution = run_model_jobs_until_anytime_quorum_interruptible(
            "prompt-evaluation",
            jobs,
            prompt_frontier_quorum_policy(&execution_contract, scheduled.len()),
            PromptExecutionStep::usable,
            || {
                control.as_ref().is_some_and(|control| {
                    control.should_stop() || control.stage_should_stop(RunStageClass::Worker)
                })
            },
        );
        let mut completed = scheduled
            .into_iter()
            .zip(layer_execution.results)
            .map(|(scheduled_step, result)| {
                let index = scheduled_step.index;
                (
                    index,
                    result.unwrap_or_else(|error| parallel_error_step(scheduled_step.step, error)),
                )
            })
            .collect::<Vec<_>>();
        completed.sort_by_key(|(index, _)| *index);
        for (_, step) in completed {
            if record_prompt_step_in_checkpoint(&mut checkpoint, &step, max_attempts).is_err() {
                return failed_execution(candidate);
            }
            if step.usable() {
                outputs.insert(
                    step.id.clone(),
                    PromptDependencyOutput {
                        status: step.status.clone(),
                        content: step.output.clone(),
                    },
                );
            }
            execution_steps.push(step);
        }
    }

    let final_output = select_final_output(&plan, &execution_steps, &outputs);
    let quality_gate_met =
        prompt_execution_quality_gate(&plan, &execution_steps, &execution_contract);
    PromptExecutionCandidate {
        plan: candidate,
        execution: PromptWorkflowExecution {
            succeeded: !final_output.trim().is_empty(),
            quality_gate_met,
            final_output,
            total_tokens: execution_steps.iter().map(|step| step.total_tokens).sum(),
            steps: execution_steps,
            latency_ms: elapsed_millis(started_at),
        },
    }
}

#[derive(Clone)]
struct ScheduledPromptStep {
    index: usize,
    step: orchestrator::WorkflowPlanStep,
    initial_prompt: String,
    completed_input_count: usize,
    input_degraded: bool,
}

struct ExecuteStepRequest {
    step: orchestrator::WorkflowPlanStep,
    initial_prompt: String,
    runner: PromptEvaluationRunner,
    control: Option<Arc<AgentRunControl>>,
    branch_cancellation: Arc<AtomicBool>,
    alternate_models: Vec<String>,
    retry_policy: PromptRetryPolicy,
    max_attempts: usize,
    max_model_turns: usize,
    max_tool_calls: usize,
    max_output_tokens: u64,
    completed_input_count: usize,
    input_degraded: bool,
}

fn execute_step(request: ExecuteStepRequest) -> PromptExecutionStep {
    let ExecuteStepRequest {
        step,
        initial_prompt,
        runner,
        control,
        branch_cancellation,
        alternate_models,
        retry_policy,
        max_attempts,
        max_model_turns,
        max_tool_calls,
        max_output_tokens,
        completed_input_count,
        input_degraded,
    } = request;
    let role = prompt_evaluation_role(&step.role);
    let stage = format!("prompt_evaluation_{}", step.id);
    let stage_class = prompt_evaluation_stage_class(&step);
    let mut model = step.model.clone();
    let mut prompt = initial_prompt.clone();
    let mut prompts = Vec::new();
    let mut errors = Vec::new();
    let mut evidence = Vec::new();
    let mut latency_ms = 0u64;
    let mut total_tokens = 0u64;
    let mut attempts = 0usize;
    let mut final_output = None;
    let mut last_partial_output = None;

    while attempts < max_attempts {
        if branch_cancellation.load(Ordering::SeqCst) {
            errors.push("parallel branch cancelled after anytime quorum".to_string());
            break;
        }
        if attempts > 0 {
            if let Some(control) = control.as_ref() {
                if let Err(reason) = control.begin_repair_attempt("prompt_evaluation_retry") {
                    errors.push(format!("retry budget exhausted: {}", reason.code()));
                    break;
                }
            }
        }
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
        last_partial_output = completion.partial_content;

        let content = completion
            .content
            .filter(|content| !content.trim().is_empty());
        let successful_evidence = evidence
            .iter()
            .filter(|entry| entry.status == "succeeded")
            .count();
        let evidence_count = completed_input_count.saturating_add(successful_evidence);
        if let Some(content) = content {
            last_partial_output = Some(content.clone());
            match validate_step_contract(&step, &content, evidence_count) {
                Ok(()) => {
                    final_output = Some(content);
                    break;
                }
                Err(failure) => {
                    errors.push(failure.message);
                    break;
                }
            }
        }

        let failure = completion.failure.unwrap_or_else(|| {
            AgentFailure::model_output(
                "empty_worker_output",
                completion
                    .error
                    .unwrap_or_else(|| "empty worker output".to_string()),
            )
        });
        errors.push(failure.message.clone());
        if attempts >= max_attempts
            || retry_policy == PromptRetryPolicy::FailFast
            || !prompt_evaluation_retry_allowed(
                &failure,
                branch_cancellation.load(Ordering::SeqCst),
            )
        {
            break;
        }

        let alternate_available = retry_policy == PromptRetryPolicy::AlternateModel
            && alternate_models.iter().any(|candidate| *candidate != model);
        match failure.recovery_action(alternate_available) {
            AgentRecoveryAction::RetrySame => {}
            AgentRecoveryAction::RetryAlternate
                if retry_policy == PromptRetryPolicy::AlternateModel =>
            {
                let Some(alternate) = alternate_models
                    .iter()
                    .find(|candidate| **candidate != model)
                else {
                    break;
                };
                model.clone_from(alternate);
            }
            AgentRecoveryAction::RetryAlternate | AgentRecoveryAction::Stop => break,
        }
        prompt = format!(
            "Retry the same authorized evaluation node after a recoverable failure. Correct the failure without widening scope or claiming unavailable actions.\n\nFailure:\n{}\n\nOriginal node contract:\n{}",
            truncate_for_collaboration(&failure.message, 2_000),
            initial_prompt
        );
    }

    let successful_evidence = evidence
        .iter()
        .filter(|entry| entry.status == "succeeded")
        .count();
    let evidence_count = completed_input_count.saturating_add(successful_evidence);
    let partial_validation = final_output.is_none().then(|| {
        last_partial_output
            .as_ref()
            .map(|partial| validate_step_contract(&step, partial, evidence_count))
    });
    if let Some(Some(Err(failure))) = partial_validation.as_ref() {
        errors.push(failure.message.clone());
    }
    let status = if final_output.is_some() {
        if input_degraded {
            WorkflowStepStatus::Degraded
        } else {
            WorkflowStepStatus::Completed
        }
    } else if matches!(partial_validation, Some(Some(Ok(())))) {
        WorkflowStepStatus::Degraded
    } else {
        WorkflowStepStatus::Failed
    };
    let output = final_output.unwrap_or_else(|| last_partial_output.unwrap_or_default());
    let tool_calls = prompt_evaluation_tool_traces(&evidence);
    PromptExecutionStep {
        id: step.id,
        role: step.role,
        model,
        prompt: prompts.join("\n\n---\n\n"),
        attempts,
        status,
        output,
        tool_calls,
        errors,
        latency_ms,
        total_tokens,
        evidence_count,
    }
}

fn validate_step_contract(
    step: &orchestrator::WorkflowPlanStep,
    output: &str,
    evidence_count: usize,
) -> Result<(), AgentFailure> {
    if step.contract.completion.require_non_empty_output && output.trim().is_empty() {
        return Err(AgentFailure::contract(
            "workflow_empty_output",
            format!("workflow step {} produced no usable output", step.id),
        ));
    }
    let minimum_evidence = step.contract.completion.minimum_evidence_items;
    if evidence_count < minimum_evidence {
        return Err(AgentFailure::contract(
            "workflow_missing_evidence",
            format!(
                "workflow step {} requires {minimum_evidence} evidence items but produced {evidence_count}",
                step.id
            ),
        ));
    }
    Ok(())
}

fn required_step_inputs(step: &orchestrator::WorkflowPlanStep) -> Vec<String> {
    step.access
        .iter()
        .chain(step.contract.input_steps.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn prompt_frontier_quorum_policy(
    contract: &ConductorExecutionContract,
    total: usize,
) -> AnytimeQuorumPolicy {
    let required_successes = match contract.stop_policy {
        ConductorStopPolicy::Exhaustive => total.max(1),
        _ => contract.required_successes_for_layer(total),
    };
    let preferred_successes = match contract.stop_policy {
        ConductorStopPolicy::FirstVerified => 1,
        _ => total.max(1),
    };
    AnytimeQuorumPolicy::new(
        required_successes,
        preferred_successes,
        Duration::from_millis(contract.quorum_grace_ms()),
        Duration::from_millis(25),
    )
}

fn record_prompt_step_in_checkpoint(
    checkpoint: &mut WorkflowExecutionCheckpoint,
    step: &PromptExecutionStep,
    max_attempts: usize,
) -> Result<(), String> {
    let now_ms = current_time_millis();
    if let Some(checkpoint_step) = checkpoint.steps.get_mut(&step.id) {
        checkpoint_step.attempts =
            if step.status == WorkflowStepStatus::Failed || step.attempts == 0 {
                max_attempts
            } else {
                checkpoint_step.attempts.max(step.attempts)
            };
    }
    match step.status {
        WorkflowStepStatus::Completed => {
            let evidence = vec![serde_json::Value::Null; step.evidence_count];
            checkpoint.complete_step(
                &step.id,
                &step.model,
                step.output.clone(),
                serde_json::to_string(&evidence).map_err(|error| error.to_string())?,
                now_ms,
            )?;
        }
        WorkflowStepStatus::Degraded => {
            checkpoint.degrade_step(
                &step.id,
                step.output.clone(),
                step.errors.join("; "),
                now_ms,
            )?;
        }
        WorkflowStepStatus::Failed => {
            checkpoint.fail_step(&step.id, step.errors.join("; "), now_ms)?;
        }
        WorkflowStepStatus::Pending | WorkflowStepStatus::Running => {
            return Err(format!(
                "prompt evaluation step {} returned a non-terminal status",
                step.id
            ));
        }
    }
    checkpoint.record_step_metrics(&step.id, step.latency_ms, step.total_tokens)
}

fn prompt_execution_quality_gate(
    plan: &WorkflowPlanIr,
    steps: &[PromptExecutionStep],
    contract: &ConductorExecutionContract,
) -> bool {
    let Some(final_step) = plan.steps.last() else {
        return false;
    };
    let final_step_completed = steps
        .iter()
        .find(|step| step.id == final_step.id)
        .is_some_and(PromptExecutionStep::succeeded);
    let root_steps = plan
        .steps
        .iter()
        .filter(|step| step.access.is_empty())
        .collect::<Vec<_>>();
    let successful_roots = root_steps
        .iter()
        .filter(|planned| {
            steps
                .iter()
                .find(|executed| executed.id == planned.id)
                .is_some_and(PromptExecutionStep::succeeded)
        })
        .count();
    let root_gate_met = root_steps.is_empty()
        || successful_roots >= contract.required_successes_for_layer(root_steps.len());
    let verification_gate_met = !contract.verification_required
        || steps.iter().any(|step| {
            step.succeeded() && matches!(step.role.as_str(), "verifier" | "synthesizer")
        });
    final_step_completed && root_gate_met && verification_gate_met
}

fn unresolved_step(
    step: orchestrator::WorkflowPlanStep,
    unresolved: &[String],
) -> PromptExecutionStep {
    let error = format!("unresolved dependencies: {}", unresolved.join(", "));
    PromptExecutionStep {
        id: step.id,
        role: step.role,
        model: step.model,
        prompt: String::new(),
        attempts: 0,
        status: WorkflowStepStatus::Failed,
        output: String::new(),
        tool_calls: Vec::new(),
        errors: vec![error],
        latency_ms: 0,
        total_tokens: 0,
        evidence_count: 0,
    }
}

fn parallel_error_step(
    step: orchestrator::WorkflowPlanStep,
    error: agent_runtime::ParallelTaskError,
) -> PromptExecutionStep {
    PromptExecutionStep {
        id: step.id,
        role: step.role,
        model: step.model,
        prompt: String::new(),
        attempts: usize::from(error != agent_runtime::ParallelTaskError::Cancelled),
        status: WorkflowStepStatus::Failed,
        output: String::new(),
        tool_calls: Vec::new(),
        errors: vec![error.to_string()],
        latency_ms: 0,
        total_tokens: 0,
        evidence_count: 0,
    }
}

fn select_final_output(
    plan: &WorkflowPlanIr,
    steps: &[PromptExecutionStep],
    outputs: &BTreeMap<String, PromptDependencyOutput>,
) -> String {
    if let Some(output) = plan
        .steps
        .last()
        .and_then(|step| outputs.get(&step.id))
        .filter(|output| !output.content.trim().is_empty())
    {
        return output.content.clone();
    }

    steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.usable())
        .max_by_key(|(execution_index, step)| {
            let output_rank = plan
                .steps
                .iter()
                .find(|planned| planned.id == step.id)
                .map(|planned| output_kind_rank(&planned.contract.output_kind))
                .unwrap_or_default();
            let completion_rank = usize::from(step.succeeded());
            (
                output_rank,
                completion_rank,
                step.evidence_count,
                *execution_index,
            )
        })
        .map(|(_, step)| step.output.clone())
        .unwrap_or_default()
}

fn output_kind_rank(kind: &orchestrator::WorkflowOutputKind) -> usize {
    match kind {
        orchestrator::WorkflowOutputKind::Analysis => 0,
        orchestrator::WorkflowOutputKind::Evidence => 1,
        orchestrator::WorkflowOutputKind::Verification => 2,
        orchestrator::WorkflowOutputKind::Synthesis => 3,
    }
}

fn failed_execution(candidate: PromptPlanCandidate) -> PromptExecutionCandidate {
    PromptExecutionCandidate {
        plan: candidate,
        execution: PromptWorkflowExecution {
            succeeded: false,
            quality_gate_met: false,
            final_output: String::new(),
            steps: Vec::new(),
            latency_ms: 0,
            total_tokens: 0,
        },
    }
}

fn elapsed_millis(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}
