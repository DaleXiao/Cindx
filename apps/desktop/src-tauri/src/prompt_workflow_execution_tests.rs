use super::*;
use crate::prompt_workflow_execution::{parallel_error_step, record_prompt_step_in_checkpoint};
use orchestrator::{AdaptiveWorkflow, AdaptiveWorkflowStep};

fn workflow_candidate(
    mut genome: ConductorPromptGenome,
    steps: Vec<AdaptiveWorkflowStep>,
) -> PromptPlanCandidate {
    genome.max_step_attempts = genome.max_step_attempts.max(1);
    let plan = WorkflowPlanIr::from_adaptive_with_profile(
        "prompt-workflow-test",
        "Solve the assigned problem",
        "pro",
        "best_of_n",
        "conductor",
        genome.id.clone(),
        &AdaptiveWorkflow { steps },
        WorkflowBudget {
            max_steps: 8,
            max_models: 4,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
    );
    PromptPlanCandidate {
        genome,
        plan: Some(plan),
        raw_output: String::new(),
        latency_ms: 0,
        total_tokens: 0,
    }
}

fn workflow_step(id: &str, role: &str, model: &str, access: &[&str]) -> AdaptiveWorkflowStep {
    AdaptiveWorkflowStep {
        id: id.to_string(),
        role: role.to_string(),
        model: model.to_string(),
        subtask: format!("Complete {id}"),
        access: access.iter().map(|value| (*value).to_string()).collect(),
    }
}

fn completed(content: &str) -> CollaborationCompletion {
    CollaborationCompletion {
        content: Some(content.to_string()),
        partial_content: None,
        error: None,
        failure: None,
        latency_ms: 10,
        usage: [("total_tokens".to_string(), "20".to_string())]
            .into_iter()
            .collect(),
        evidence: Vec::new(),
    }
}

fn failed(failure: AgentFailure) -> CollaborationCompletion {
    CollaborationCompletion {
        content: None,
        partial_content: None,
        error: Some(failure.message.clone()),
        failure: Some(failure),
        latency_ms: 10,
        usage: BTreeMap::new(),
        evidence: Vec::new(),
    }
}

fn partial(content: &str, failure: AgentFailure) -> CollaborationCompletion {
    CollaborationCompletion {
        content: None,
        partial_content: Some(content.to_string()),
        error: Some(failure.message.clone()),
        failure: Some(failure),
        latency_ms: 10,
        usage: BTreeMap::new(),
        evidence: Vec::new(),
    }
}

#[test]
fn output_contract_makes_a_mislabeled_final_step_terminal() {
    let step = orchestrator::WorkflowPlanStep {
        id: "final".to_string(),
        role: "worker".to_string(),
        model: "model".to_string(),
        subtask: "deliver the final answer".to_string(),
        access: vec!["candidate".to_string()],
        tool_policy: WorkflowToolPolicy::None,
        contract: orchestrator::WorkflowStepContract {
            input_steps: vec!["candidate".to_string()],
            output_kind: orchestrator::WorkflowOutputKind::Synthesis,
            completion: orchestrator::WorkflowCompletionCriteria::default(),
        },
    };

    assert_eq!(
        prompt_evaluation_stage_class(&step),
        RunStageClass::Finalizer
    );
}

#[test]
fn terminal_delivery_layer_survives_the_reserved_time_boundary() {
    let mut budget = RunBudget::for_effort("pro");
    budget.max_duration = Duration::from_secs(60);
    budget.terminal_time_reserve = budget.max_duration;
    let control = AgentRunControl::with_budget(budget);

    assert!(
        prompt_workflow_execution::prompt_evaluation_layer_should_stop(
            &control,
            [RunStageClass::Candidate, RunStageClass::Worker],
        )
    );
    assert!(
        !prompt_workflow_execution::prompt_evaluation_layer_should_stop(
            &control,
            [RunStageClass::Finalizer],
        )
    );
    assert!(
        prompt_workflow_execution::prompt_evaluation_layer_should_stop(
            &control,
            [RunStageClass::Worker, RunStageClass::Reviewer],
        )
    );
    assert!(
        prompt_workflow_execution::prompt_evaluation_layer_should_stop(
            &control,
            [RunStageClass::Synthesizer],
        )
    );
}

#[test]
fn execution_arena_preserves_a_valid_branch_when_a_sibling_fails() {
    let mut genome = ConductorPromptGenome::seed_for_effort("pro");
    genome.max_step_attempts = 1;
    genome.retry_policy = PromptRetryPolicy::FailFast;
    let candidate = workflow_candidate(
        genome,
        vec![
            workflow_step("strong", "worker", "worker-a", &[]),
            workflow_step("weak", "worker", "worker-b", &[]),
            workflow_step("final", "synthesizer", "worker-c", &["strong", "weak"]),
        ],
    );
    let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let captured = Arc::clone(&calls);
    let runner: PromptEvaluationRunner = Arc::new(move |request, _| {
        captured
            .lock()
            .expect("call capture lock")
            .push((request.model.clone(), request.prompt.clone()));
        match request.model.as_str() {
            "worker-a" => completed("The verified answer is (A)."),
            "worker-b" => failed(AgentFailure::new(
                "provider_invalid_request",
                "worker-b rejected the request",
                AgentFailureClass::ProviderPermanent,
                false,
            )),
            _ => completed("The surviving evidence supports answer (A)."),
        }
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(result.execution.succeeded);
    assert!(!result.execution.quality_gate_met);
    assert_eq!(
        result.execution.final_output,
        "The surviving evidence supports answer (A)."
    );
    assert_eq!(result.execution.steps.len(), 3);
    let calls = calls.lock().expect("call capture lock");
    let (_, synthesis_prompt) = calls
        .iter()
        .find(|(model, _)| model == "worker-c")
        .expect("the synthesis step should run from the surviving branch");
    assert!(synthesis_prompt.contains("[strong status=completed]"));
    assert!(synthesis_prompt.contains("Missing dependency ids:\nweak"));
    assert!(!synthesis_prompt.contains("worker-b rejected the request"));
    let degraded_final = result
        .execution
        .steps
        .iter()
        .find(|step| step.id == "final")
        .expect("final step should remain visible in the trajectory");
    assert_eq!(degraded_final.status, WorkflowStepStatus::Degraded);
    assert_eq!(degraded_final.attempts, 1);
}

#[test]
fn product_semantics_select_the_direct_anchor_when_the_team_has_no_delivery() {
    let mut genome = ConductorPromptGenome::seed_for_effort("auto");
    genome.max_step_attempts = 1;
    genome.retry_policy = PromptRetryPolicy::FailFast;
    let candidate = workflow_candidate(
        genome,
        vec![workflow_step("answer", "worker", "worker-a", &[])],
    );
    let runner: PromptEvaluationRunner = Arc::new(|request, _| {
        if request.stage == "prompt_evaluation_direct_anchor" {
            completed("Use the independent derivation and preserve the requested answer format.")
        } else {
            failed(AgentFailure::new(
                "provider_invalid_request",
                "team branch failed permanently",
                AgentFailureClass::ProviderPermanent,
                false,
            ))
        }
    });

    let result = execute_prompt_workflow_candidate_with_direct_anchor_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(result.execution.succeeded);
    assert!(!result.execution.quality_gate_met);
    assert_eq!(
        result.execution.final_output,
        "Use the independent derivation and preserve the requested answer format."
    );
    assert_eq!(result.execution.steps[0].status, WorkflowStepStatus::Failed);
}

#[test]
fn product_semantics_select_the_anchor_when_bidirectional_review_prefers_it() {
    let candidate = workflow_candidate(
        ConductorPromptGenome::seed_for_effort("pro"),
        vec![workflow_step("answer", "worker", "worker-a", &[])],
    );
    let runner: PromptEvaluationRunner = Arc::new(|request, _| match request.stage.as_str() {
        "prompt_evaluation_direct_anchor" => completed("anchor guidance"),
        "prompt_evaluation_team_anchor_forward" => completed(
            r#"{"score_a":0.3,"score_b":0.9,"safety_violations_a":0,"safety_violations_b":0}"#,
        ),
        "prompt_evaluation_team_anchor_reverse" => completed(
            r#"{"score_a":0.9,"score_b":0.3,"safety_violations_a":0,"safety_violations_b":0}"#,
        ),
        _ => completed("team guidance"),
    });

    let result = execute_prompt_workflow_candidate_with_direct_anchor_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert_eq!(result.execution.final_output, "anchor guidance");
    assert!(!result.execution.quality_gate_met);
}

#[test]
fn product_semantics_keep_the_team_only_when_bidirectional_review_prefers_it() {
    let candidate = workflow_candidate(
        ConductorPromptGenome::seed_for_effort("pro"),
        vec![workflow_step("answer", "worker", "worker-a", &[])],
    );
    let runner: PromptEvaluationRunner = Arc::new(|request, _| match request.stage.as_str() {
        "prompt_evaluation_direct_anchor" => completed("anchor guidance"),
        "prompt_evaluation_team_anchor_forward" => completed(
            r#"{"score_a":0.9,"score_b":0.3,"safety_violations_a":0,"safety_violations_b":0}"#,
        ),
        "prompt_evaluation_team_anchor_reverse" => completed(
            r#"{"score_a":0.3,"score_b":0.9,"safety_violations_a":0,"safety_violations_b":0}"#,
        ),
        _ => completed("team guidance"),
    });

    let result = execute_prompt_workflow_candidate_with_direct_anchor_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert_eq!(result.execution.final_output, "team guidance");
}

#[test]
fn product_semantics_fail_closed_when_pairwise_order_changes_the_winner() {
    let candidate = workflow_candidate(
        ConductorPromptGenome::seed_for_effort("pro"),
        vec![workflow_step("answer", "worker", "worker-a", &[])],
    );
    let runner: PromptEvaluationRunner = Arc::new(|request, _| match request.stage.as_str() {
        "prompt_evaluation_direct_anchor" => completed("anchor guidance"),
        "prompt_evaluation_team_anchor_forward" => completed(
            r#"{"score_a":0.9,"score_b":0.3,"safety_violations_a":0,"safety_violations_b":0}"#,
        ),
        "prompt_evaluation_team_anchor_reverse" => completed(
            r#"{"score_a":0.9,"score_b":0.3,"safety_violations_a":0,"safety_violations_b":0}"#,
        ),
        _ => completed("team guidance"),
    });

    let result = execute_prompt_workflow_candidate_with_direct_anchor_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert_eq!(result.execution.final_output, "anchor guidance");
}

#[test]
fn execution_arena_delivers_a_degraded_direct_synthesis_after_all_branches_fail() {
    let mut genome = ConductorPromptGenome::seed_for_effort("pro");
    genome.max_step_attempts = 1;
    genome.retry_policy = PromptRetryPolicy::FailFast;
    let candidate = workflow_candidate(
        genome,
        vec![
            workflow_step("approach-a", "worker", "worker-a", &[]),
            workflow_step("approach-b", "worker", "worker-b", &[]),
            workflow_step(
                "verify",
                "verifier",
                "reviewer",
                &["approach-a", "approach-b"],
            ),
            workflow_step(
                "final",
                "synthesizer",
                "synthesizer",
                &["approach-a", "approach-b", "verify"],
            ),
        ],
    );
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&calls);
    let runner: PromptEvaluationRunner = Arc::new(move |request, _| {
        captured
            .lock()
            .expect("call capture lock")
            .push(request.model.clone());
        match request.model.as_str() {
            "worker-a" | "worker-b" => failed(AgentFailure::new(
                "provider_deadline",
                "analysis branch exhausted its bounded attempt",
                AgentFailureClass::ProviderTransient,
                true,
            )),
            "synthesizer" => completed("The best direct answer is (A)."),
            unexpected => panic!("blocked intermediate node ran unexpectedly: {unexpected}"),
        }
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(result.execution.succeeded);
    assert!(!result.execution.quality_gate_met);
    assert_eq!(
        result.execution.final_output,
        "The best direct answer is (A)."
    );
    let final_step = result
        .execution
        .steps
        .iter()
        .find(|step| step.id == "final")
        .expect("the direct delivery attempt should remain in the trace");
    assert_eq!(final_step.status, WorkflowStepStatus::Degraded);
    let calls = calls.lock().expect("call capture lock");
    assert!(calls.contains(&"synthesizer".to_string()));
    assert!(!calls.contains(&"reviewer".to_string()));
}

#[test]
fn execution_stops_after_the_delivery_target_instead_of_running_orphan_speculation() {
    let mut genome = ConductorPromptGenome::seed_for_effort("pro");
    genome.max_step_attempts = 1;
    let candidate = workflow_candidate(
        genome,
        vec![
            workflow_step("candidate", "worker", "worker-a", &[]),
            workflow_step("final", "synthesizer", "worker-b", &["candidate"]),
            workflow_step("orphan", "worker", "worker-c", &[]),
        ],
    );
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&calls);
    let runner: PromptEvaluationRunner = Arc::new(move |request, _| {
        captured
            .lock()
            .expect("call capture lock")
            .push(request.model.clone());
        completed(if request.model == "worker-b" {
            "The final answer is (A)."
        } else {
            "Candidate answer (A)."
        })
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(result.execution.succeeded);
    assert_eq!(result.execution.final_output, "The final answer is (A).");
    assert_eq!(result.execution.steps.len(), 2);
    assert_eq!(
        calls.lock().expect("call capture lock").as_slice(),
        ["worker-a", "worker-b"]
    );
}

#[test]
fn execution_arena_requires_the_declared_pro_quorum_before_promotion() {
    let mut genome = ConductorPromptGenome::seed_for_effort("pro");
    genome.max_step_attempts = 1;
    let candidate = workflow_candidate(
        genome,
        vec![
            workflow_step("left", "worker", "worker-a", &[]),
            workflow_step("right", "worker", "worker-b", &[]),
            workflow_step("final", "synthesizer", "worker-c", &["left", "right"]),
        ],
    );
    let runner: PromptEvaluationRunner = Arc::new(|request, _| {
        assert!(!request.stage.is_empty());
        assert!(request.max_model_turns >= 1);
        assert!(request.max_output_tokens >= 1);
        completed(match request.model.as_str() {
            "worker-a" => "Independent result A",
            "worker-b" => "Independent result B",
            _ => "Synthesis of A and B",
        })
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(result.execution.succeeded);
    assert!(result.execution.quality_gate_met);
}

#[test]
fn execution_arena_propagates_a_real_partial_with_an_explicit_degraded_status() {
    let mut genome = ConductorPromptGenome::seed_for_effort("auto");
    genome.max_step_attempts = 1;
    genome.retry_policy = PromptRetryPolicy::FailFast;
    let candidate = workflow_candidate(
        genome,
        vec![
            workflow_step("source", "worker", "worker-a", &[]),
            workflow_step("final", "synthesizer", "worker-b", &["source"]),
        ],
    );
    let synthesis_prompt = Arc::new(Mutex::new(String::new()));
    let captured = Arc::clone(&synthesis_prompt);
    let runner: PromptEvaluationRunner = Arc::new(move |request, _| {
        if request.model == "worker-a" {
            partial(
                "Partial result: answer (A) is supported by the available derivation.",
                AgentFailure::new(
                    "provider_deadline",
                    "worker timed out after useful output",
                    AgentFailureClass::ProviderTransient,
                    true,
                ),
            )
        } else {
            *captured.lock().expect("synthesis prompt lock") = request.prompt;
            completed("Answer (A), with the partial-input limitation disclosed.")
        }
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(result.execution.succeeded);
    assert_eq!(
        result.execution.steps[0].status,
        WorkflowStepStatus::Degraded
    );
    assert_eq!(
        result.execution.steps[1].status,
        WorkflowStepStatus::Degraded
    );
    let prompt = synthesis_prompt.lock().expect("synthesis prompt lock");
    assert!(prompt.contains("[source status=degraded_partial]"));
    assert!(prompt.contains("Partial result: answer (A)"));
}

#[test]
fn execution_arena_never_resolves_a_dependency_with_a_failure_placeholder() {
    let mut genome = ConductorPromptGenome::seed_for_effort("auto");
    genome.max_step_attempts = 1;
    genome.retry_policy = PromptRetryPolicy::FailFast;
    let candidate = workflow_candidate(
        genome,
        vec![
            workflow_step("source", "worker", "worker-a", &[]),
            workflow_step("final", "synthesizer", "worker-b", &["source"]),
        ],
    );
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&prompts);
    let runner: PromptEvaluationRunner = Arc::new(move |request, _| {
        captured
            .lock()
            .expect("prompt capture lock")
            .push(request.prompt);
        failed(AgentFailure::new(
            "provider_invalid_request",
            "source failed",
            AgentFailureClass::ProviderPermanent,
            false,
        ))
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(!result.execution.succeeded);
    assert!(result.execution.final_output.is_empty());
    let prompts = prompts.lock().expect("prompt capture lock");
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("Missing dependency ids:\nsource"));
    assert!(!prompts[1].contains("source failed"));
    assert!(!prompts
        .iter()
        .any(|prompt| prompt.contains("[execution failed:")));
}

#[test]
fn execution_arena_enforces_an_explicit_evidence_contract() {
    let mut genome = ConductorPromptGenome::seed_for_effort("auto");
    genome.max_step_attempts = 1;
    genome.retry_policy = PromptRetryPolicy::FailFast;
    let mut candidate = workflow_candidate(
        genome,
        vec![workflow_step("verify", "worker", "worker-a", &[])],
    );
    let plan = candidate.plan.as_mut().expect("test plan");
    plan.steps[0].tool_policy = WorkflowToolPolicy::ReadOnlyEvidence;
    plan.steps[0].contract.completion.minimum_evidence_items = 1;
    let runner: PromptEvaluationRunner = Arc::new(|_, _| {
        partial(
            "An unsupported partial assertion.",
            AgentFailure::new(
                "provider_deadline",
                "worker timed out",
                AgentFailureClass::ProviderTransient,
                true,
            ),
        )
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(!result.execution.succeeded);
    assert!(result.execution.final_output.is_empty());
    assert_eq!(result.execution.steps[0].status, WorkflowStepStatus::Failed);
    assert!(result.execution.steps[0]
        .errors
        .iter()
        .any(|error| error.contains("evidence")));
}

#[test]
fn quorum_cancellation_is_not_recorded_as_a_worker_failure() {
    let genome = ConductorPromptGenome::seed_for_effort("pro");
    let candidate = workflow_candidate(
        genome,
        vec![workflow_step("worker", "worker", "worker-a", &[])],
    );
    let plan = candidate.plan.expect("test plan");
    let step = parallel_error_step(
        plan.steps[0].clone(),
        agent_runtime::ParallelTaskError::Cancelled,
    );

    assert_eq!(step.status, WorkflowStepStatus::Cancelled);
    assert_eq!(step.attempts, 0);

    let mut checkpoint = WorkflowExecutionCheckpoint::new("cancelled", plan, 1);
    record_prompt_step_in_checkpoint(&mut checkpoint, &step, 2).unwrap();
    assert_eq!(
        checkpoint.steps["worker"].status,
        WorkflowStepStatus::Cancelled
    );
    assert_eq!(checkpoint.steps["worker"].attempts, 2);
}

#[test]
fn execution_arena_does_not_retry_a_permanent_provider_failure() {
    let mut genome = ConductorPromptGenome::seed_for_effort("auto");
    genome.max_step_attempts = 2;
    genome.retry_policy = PromptRetryPolicy::SameModel;
    let candidate = workflow_candidate(
        genome,
        vec![workflow_step("answer", "worker", "worker-a", &[])],
    );
    let calls = Arc::new(Mutex::new(0usize));
    let captured = Arc::clone(&calls);
    let runner: PromptEvaluationRunner = Arc::new(move |_, _| {
        let mut calls = captured.lock().expect("call count lock");
        *calls += 1;
        if *calls == 1 {
            failed(AgentFailure::new(
                "provider_invalid_request",
                "request cannot succeed unchanged",
                AgentFailureClass::ProviderPermanent,
                false,
            ))
        } else {
            completed("A retry would hide the permanent failure.")
        }
    });

    let result = execute_prompt_workflow_candidate_with_runner(
        "Solve the assigned problem",
        candidate,
        runner,
    );

    assert!(!result.execution.succeeded);
    assert_eq!(*calls.lock().expect("call count lock"), 1);
    assert_eq!(result.execution.steps[0].attempts, 1);
}
