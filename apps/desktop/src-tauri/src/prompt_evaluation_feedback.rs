use super::*;

pub(crate) fn evaluate_prompt_candidate_pair(
    config: &ProviderConfig,
    reviewer_model: &str,
    objective: &str,
    candidate_a: &PromptExecutionCandidate,
    candidate_b: &PromptExecutionCandidate,
    evaluation_id: &str,
    control: &Arc<AgentRunControl>,
) -> Result<PromptPairwiseEvaluationPayload, String> {
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
                    step.succeeded(),
                    step.latency_ms,
                    step.total_tokens,
                    step.tool_calls.len(),
                    truncate_for_collaboration(&step.output, 6_000)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        format!(
            "plan:\n{plan}\n\nexecution_succeeded={} quality_gate_met={} execution_latency_ms={} execution_tokens={}\n\nsteps:\n{}\n\nfinal_output:\n{}",
            candidate.execution.succeeded,
            candidate.execution.quality_gate_met,
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
        reviewer_model.to_string(),
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
    provenance: PromptEvaluationProvenance,
) -> PromptEvolutionObservation {
    let score = score.clamp(0.0, 1.0);
    let succeeded = candidate.execution.succeeded
        && candidate.execution.quality_gate_met
        && safety_violations == 0
        && score >= 0.5;
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
                        .is_some_and(PromptExecutionStep::succeeded),
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
    for step in &candidate.execution.steps {
        let fail_soft_cancellation = candidate.execution.succeeded
            && !step.succeeded()
            && step.errors.iter().any(|error| {
                let error = error.to_ascii_lowercase();
                error.contains("cancel") || error.contains("quorum")
            });
        if fail_soft_cancellation {
            merge_unique_feedback_entries(
                &mut actionable_feedback.passed_constraints,
                vec![format!(
                    "step {} ({}) was safely cancelled after the workflow quality gate was met",
                    step.id, step.role
                )],
            );
            continue;
        }
        if !step.succeeded() {
            merge_unique_feedback_entries(
                &mut actionable_feedback.failed_constraints,
                vec![format!(
                    "step {} ({}) failed after {} attempt(s)",
                    step.id,
                    step.role,
                    step.attempts.max(1)
                )],
            );
            merge_unique_feedback_entries(
                &mut actionable_feedback.errors,
                step.errors
                    .iter()
                    .map(|error| format!("{}: {error}", step.id))
                    .collect(),
            );
        }
        if step.attempts > 1 {
            merge_unique_feedback_entries(
                &mut actionable_feedback.suggested_changes,
                vec![format!(
                    "make step {} independently verifiable and choose an alternate strategy after its first failed attempt",
                    step.id
                )],
            );
        }
    }
    if candidate.execution.succeeded && candidate.execution.final_output.trim().is_empty() {
        merge_unique_feedback_entries(
            &mut actionable_feedback.failed_constraints,
            vec![
                "the workflow reached a terminal state without a deliverable synthesis".to_string(),
            ],
        );
    }
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
        provenance,
    }
}
