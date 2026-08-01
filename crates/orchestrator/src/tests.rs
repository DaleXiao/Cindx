use super::*;

fn test_verified_learning_evidence() -> LearningEvidenceV1 {
    LearningEvidenceV1::verified_postcondition(
        LearningUsageCompleteness::Complete,
        0,
        "0".repeat(64),
    )
}

fn test_quality_learning_evidence(
    score: f32,
    passed: bool,
    attribution: LearningAttribution,
) -> LearningEvidenceV1 {
    LearningEvidenceV1::independent_quality(
        LearningTermination::Completed,
        attribution,
        LearningUsageCompleteness::Complete,
        0,
        "0".repeat(64),
        IndependentQualitySource::CollaborationQualityGate,
        (score.clamp(0.0, 1.0) * 10_000.0).round() as u16,
        passed,
    )
}

#[test]
fn learning_evidence_contract_is_bounded_and_fails_closed() {
    let evidence = test_quality_learning_evidence(0.91, true, LearningAttribution::Model);
    let encoded = evidence
        .to_metadata_value()
        .expect("valid evidence should serialize");
    assert!(encoded.len() <= LEARNING_EVIDENCE_MAX_BYTES);
    let metadata = [(LEARNING_EVIDENCE_METADATA_KEY.to_string(), encoded)]
        .into_iter()
        .collect::<Metadata>();
    assert_eq!(LearningEvidenceV1::from_metadata(&metadata), Some(evidence));

    let tool_encoded = test_verified_learning_evidence()
        .to_metadata_value()
        .expect("valid tool evidence should serialize");
    for missing_field in ["independent_quality_source", "quality_bps"] {
        let mut partial = serde_json::from_str::<serde_json::Value>(&tool_encoded)
            .expect("serialized evidence should remain valid JSON");
        assert_eq!(partial.get(missing_field), Some(&serde_json::Value::Null));
        partial
            .as_object_mut()
            .expect("evidence should serialize as an object")
            .remove(missing_field);
        let metadata = [(
            LEARNING_EVIDENCE_METADATA_KEY.to_string(),
            partial.to_string(),
        )]
        .into_iter()
        .collect::<Metadata>();
        assert!(LearningEvidenceV1::from_metadata(&metadata).is_none());
    }

    let invalid = LearningEvidenceV1 {
        disposition: LearningDisposition::Positive,
        verification: LearningVerification::Passed,
        attribution: LearningAttribution::Model,
        usage_completeness: LearningUsageCompleteness::Complete,
        steer_epoch: Some(0),
        budget_fingerprint: Some("not-a-fingerprint".to_string()),
        independent_quality_source: Some(IndependentQualitySource::CollaborationQualityGate),
        quality_bps: Some(9_100),
        ..LearningEvidenceV1::default()
    };
    assert!(!invalid.is_learnable());
    assert!(invalid.to_metadata_value().is_none());
    assert!(!LearningEvidenceV1::default().is_learnable());

    let legacy = serde_json::json!({
        "task_class": "general",
        "context_signature": "general",
        "selected_policy": "single",
        "selected_model": "fast-mini",
        "latency_ms": 10,
        "outcome": "succeeded",
        "quality_score": 0.99,
        "verification_passed": true,
        "cost_proxy": 10,
        "tool_count": 0,
        "retrieval_count": 0,
        "user_override": false
    });
    let legacy: RoutingTelemetry =
        serde_json::from_value(legacy).expect("legacy telemetry should remain readable");
    assert!(!legacy.learning_evidence.is_learnable());
}

fn candidates() -> Vec<ModelCandidate> {
    vec![
        ModelCandidate {
            name: "fast-mini".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: false,
            cost_tier: 1,
            latency_tier: 1,
        },
        ModelCandidate {
            name: "strong-vision".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 4,
            latency_tier: 3,
        },
    ]
}

fn workflow_plan(workflow_id: &str, with_verifier: bool) -> WorkflowPlanIr {
    let mut steps = vec![
        AdaptiveWorkflowStep {
            id: "approach_a".to_string(),
            role: "thinker".to_string(),
            model: "planner".to_string(),
            subtask: "develop the primary approach".to_string(),
            access: Vec::new(),
        },
        AdaptiveWorkflowStep {
            id: "approach_b".to_string(),
            role: "worker".to_string(),
            model: "reviewer".to_string(),
            subtask: "develop an independent alternative".to_string(),
            access: Vec::new(),
        },
    ];
    if with_verifier {
        steps.push(AdaptiveWorkflowStep {
            id: "verify".to_string(),
            role: "verifier".to_string(),
            model: "reviewer".to_string(),
            subtask: "cross-check both approaches".to_string(),
            access: vec!["approach_a".to_string(), "approach_b".to_string()],
        });
    }
    steps.push(AdaptiveWorkflowStep {
        id: "synthesize".to_string(),
        role: "synthesizer".to_string(),
        model: "planner".to_string(),
        subtask: "produce one execution brief".to_string(),
        access: if with_verifier {
            vec![
                "approach_a".to_string(),
                "approach_b".to_string(),
                "verify".to_string(),
            ]
        } else {
            vec!["approach_a".to_string(), "approach_b".to_string()]
        },
    });
    WorkflowPlanIr::from_adaptive(
        workflow_id,
        "Compare two implementation strategies",
        "pro",
        "best_of_n",
        "planner",
        &AdaptiveWorkflow { steps },
        WorkflowBudget {
            max_steps: 5,
            max_models: 2,
            max_model_turns_per_step: 5,
            max_tool_calls_per_step: 6,
            max_output_tokens_per_step: 4_096,
        },
    )
}

#[test]
fn fallback_delivery_sink_has_a_terminal_output_contract() {
    let plan = WorkflowPlanIr::from_adaptive(
        "single-step",
        "Answer the question",
        "auto",
        "single",
        "default",
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "answer".to_string(),
                role: "worker".to_string(),
                model: "default".to_string(),
                subtask: "Return the final answer".to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: 1,
            max_tool_calls_per_step: 0,
            max_output_tokens_per_step: 1_024,
        },
    );

    assert_eq!(
        plan.steps[0].contract.output_kind,
        WorkflowOutputKind::Synthesis
    );
}

fn conductor_request() -> ConductorRequest {
    let routing = RoutingContext::from_prompt("Compare implementation strategies", Vec::new());
    ConductorRequest {
        workflow_id: "workflow-conductor".to_string(),
        objective: "Compare implementation strategies".to_string(),
        recent_context: "The workspace uses Rust.".to_string(),
        effort: "pro".to_string(),
        policy: "best_of_n".to_string(),
        conductor_model: "conductor-only".to_string(),
        primary_model: "planner".to_string(),
        worker_models: vec!["planner".to_string(), "reviewer".to_string()],
        role_hints: ConductorRoleHints {
            planner: "planner".to_string(),
            executor: "planner".to_string(),
            reviewer: "reviewer".to_string(),
            synthesizer: "planner".to_string(),
        },
        budget: WorkflowBudget {
            max_steps: 3,
            max_models: 2,
            max_model_turns_per_step: 5,
            max_tool_calls_per_step: 6,
            max_output_tokens_per_step: 4_096,
        },
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 2 },
        ),
        prior_hint: Some("Prefer two independent branches.".to_string()),
        prompt_evolution_enabled: true,
        prompt_genome: ConductorPromptGenome::seed_for_effort("pro"),
    }
}

#[test]
fn tool_steps_keep_a_discovery_and_evidence_round_without_slowing_text_only_fast() {
    assert_eq!(WorkflowToolPolicy::None.effective_model_turn_budget(1), 1);
    assert_eq!(
        WorkflowToolPolicy::ReadOnlyEvidence.effective_model_turn_budget(1),
        2
    );
    assert_eq!(
        WorkflowToolPolicy::ReadOnlyExploration.effective_model_turn_budget(1),
        3
    );
    assert_eq!(
        WorkflowToolPolicy::ReadOnlyExploration.effective_model_turn_budget(3),
        3
    );
    assert_eq!(WorkflowToolPolicy::None.effective_tool_call_budget(6), 0);
    assert_eq!(
        WorkflowToolPolicy::ReadOnlyEvidence.effective_tool_call_budget(0),
        4
    );
    assert_eq!(
        WorkflowToolPolicy::ReadOnlyExploration.effective_tool_call_budget(4),
        6
    );
}

#[test]
fn workflow_ir_round_trips_and_enforces_declared_budgets() {
    let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
    let plan = workflow_plan("workflow-1", false);
    plan.validate(&allowed_models)
        .expect("workflow should be valid");

    let json = plan.to_json().expect("workflow should serialize");
    assert_eq!(
        WorkflowPlanIr::from_json(&json, &allowed_models).unwrap(),
        plan
    );

    let mut invalid = plan;
    invalid.budget.max_steps = 2;
    assert_eq!(
        invalid.validate(&allowed_models),
        Err("workflow exceeds its declared step budget".to_string())
    );
}

#[test]
fn workflow_checkpoint_resumes_only_incomplete_dependency_ready_steps() {
    let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
    let plan = workflow_plan("workflow-resume", false);
    let layers = adaptive_workflow_layers(&plan.adaptive_workflow()).unwrap();
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume-key", plan.clone(), 1_000);

    assert_eq!(
        checkpoint.runnable_step_indices(&layers[0]).unwrap(),
        vec![0, 1]
    );
    checkpoint
        .begin_step("approach_a", "planner", 1_010)
        .unwrap();
    checkpoint
        .complete_step(
            "approach_a",
            "planner",
            "primary".to_string(),
            "[]".to_string(),
            1_020,
        )
        .unwrap();
    assert_eq!(
        checkpoint.runnable_step_indices(&layers[0]).unwrap(),
        vec![1]
    );
    assert!(checkpoint.runnable_step_indices(&layers[1]).is_err());

    let json = checkpoint.to_json().unwrap();
    let mut restored = WorkflowExecutionCheckpoint::from_json(&json, &allowed_models).unwrap();
    restored
        .begin_step("approach_b", "reviewer", 1_030)
        .unwrap();
    restored
        .complete_step(
            "approach_b",
            "reviewer",
            "alternative".to_string(),
            "[]".to_string(),
            1_040,
        )
        .unwrap();
    assert_eq!(restored.runnable_step_indices(&layers[1]).unwrap(), vec![2]);
    restored.begin_step("synthesize", "planner", 1_050).unwrap();
    restored
        .fail_step("synthesize", "transient", 1_060)
        .unwrap();
    assert_eq!(restored.runnable_step_indices(&layers[1]).unwrap(), vec![2]);
    restored
        .complete_step(
            "synthesize",
            "reviewer",
            "final".to_string(),
            "[]".to_string(),
            1_070,
        )
        .unwrap();
    restored.anytime_outputs.insert(
        "__direct_anchor".to_string(),
        "recoverable anchor".to_string(),
    );
    restored
        .record_step_metrics("synthesize", 420, 900)
        .unwrap();
    let credits = restored.assign_step_credits(0.9);
    assert_eq!(credits.len(), 3);
    assert!(credits
        .iter()
        .find(|step| step.step_id == "synthesize")
        .is_some_and(|step| step.credit > 0.7 && step.total_tokens == 900));
    assert!(!restored.is_complete());
    restored
        .finalize("quality-gated final".to_string(), 1_080)
        .unwrap();
    assert!(restored.is_complete());
    assert_eq!(
        restored
            .completed_outputs()
            .get("approach_a")
            .map(String::as_str),
        Some("primary")
    );
    assert_eq!(
        restored
            .completed_outputs()
            .get("synthesize")
            .map(String::as_str),
        Some("quality-gated final")
    );
    assert_eq!(
        restored
            .anytime_outputs
            .get("__direct_anchor")
            .map(String::as_str),
        Some("recoverable anchor")
    );
}

#[test]
fn degraded_branch_preserves_dag_progress_without_claiming_success() {
    let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
    let plan = workflow_plan("workflow-degraded", false);
    let layers = adaptive_workflow_layers(&plan.adaptive_workflow()).unwrap();
    let mut checkpoint = WorkflowExecutionCheckpoint::new("degraded-key", plan, 1_000);

    checkpoint
        .complete_step(
            "approach_a",
            "planner",
            "grounded branch".to_string(),
            "[]".to_string(),
            1_010,
        )
        .unwrap();
    checkpoint
        .fail_step("approach_b", "provider timeout", 1_020)
        .unwrap();
    checkpoint
        .degrade_step(
            "approach_b",
            "INTERNAL DEGRADED BRANCH approach_b".to_string(),
            "provider timeout",
            1_030,
        )
        .unwrap();

    assert_eq!(checkpoint.completed_step_count(), 1);
    assert_eq!(checkpoint.resolved_step_count(), 2);
    assert_eq!(
        checkpoint.runnable_step_indices(&layers[1]).unwrap(),
        vec![2]
    );
    assert_eq!(
        checkpoint
            .completed_outputs()
            .get("approach_b")
            .map(String::as_str),
        Some("INTERNAL DEGRADED BRANCH approach_b")
    );
    let restored =
        WorkflowExecutionCheckpoint::from_json(&checkpoint.to_json().unwrap(), &allowed_models)
            .unwrap();
    assert_eq!(
        restored.steps["approach_b"].status,
        WorkflowStepStatus::Degraded
    );
}

#[test]
fn workflow_checkpoint_requires_an_explicit_budget_continuation() {
    let mut plan = workflow_plan("workflow-budget", false);
    plan.budget.max_model_turns_per_step = 1;
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume-budget", plan, 2_000);

    checkpoint
        .begin_step("approach_a", "planner", 2_010)
        .unwrap();
    checkpoint
        .fail_step("approach_a", "timeout", 2_020)
        .unwrap();
    assert!(checkpoint
        .begin_step("approach_a", "planner", 2_030)
        .unwrap_err()
        .contains("exhausted"));
    checkpoint.continue_with_budget(1, 2_040);
    checkpoint
        .begin_step("approach_a", "planner", 2_050)
        .unwrap();
    assert_eq!(checkpoint.continuations, 1);
    assert_eq!(checkpoint.steps["approach_a"].attempts, 2);
}

#[test]
fn workflow_checkpoint_accepts_an_explicit_attempt_budget() {
    let mut plan = workflow_plan("workflow-attempt-budget", false);
    plan.budget.max_model_turns_per_step = 1;
    let mut checkpoint = WorkflowExecutionCheckpoint::new("attempt-budget", plan, 3_000);

    checkpoint
        .begin_step_with_attempt_limit("approach_a", "planner", 2, 3_010)
        .unwrap();
    checkpoint.fail_step("approach_a", "retry", 3_020).unwrap();
    checkpoint
        .begin_step_with_attempt_limit("approach_a", "planner", 2, 3_030)
        .unwrap();
    assert!(checkpoint
        .begin_step_with_attempt_limit("approach_a", "planner", 2, 3_040)
        .unwrap_err()
        .contains("exhausted"));
}

#[test]
fn search_teacher_prefers_reliable_efficient_topology() {
    let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
    let fast_plan = workflow_plan("fast-1", false);
    let slow_plan = workflow_plan("slow-1", true);
    let telemetry = vec![
        WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: fast_plan.clone(),
            succeeded: true,
            quality_score: Some(0.92),
            learning_evidence: test_quality_learning_evidence(
                0.92,
                true,
                LearningAttribution::Workflow,
            ),
            latency_ms: 4_000,
            total_tokens: 4_000,
            tool_calls: 2,
            successful_tools_by_step: [("approach_a".to_string(), vec!["file.search".to_string()])]
                .into_iter()
                .collect(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        },
        WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: fast_plan.clone(),
            succeeded: true,
            quality_score: Some(0.88),
            learning_evidence: test_quality_learning_evidence(
                0.88,
                true,
                LearningAttribution::Workflow,
            ),
            latency_ms: 5_000,
            total_tokens: 5_000,
            tool_calls: 2,
            successful_tools_by_step: [("approach_a".to_string(), vec!["file.search".to_string()])]
                .into_iter()
                .collect(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        },
        WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: fast_plan.clone(),
            succeeded: true,
            quality_score: Some(0.90),
            learning_evidence: test_quality_learning_evidence(
                0.90,
                true,
                LearningAttribution::Workflow,
            ),
            latency_ms: 4_500,
            total_tokens: 4_500,
            tool_calls: 2,
            successful_tools_by_step: [("approach_a".to_string(), vec!["file.search".to_string()])]
                .into_iter()
                .collect(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        },
        WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: fast_plan,
            succeeded: true,
            quality_score: Some(0.91),
            learning_evidence: test_quality_learning_evidence(
                0.91,
                true,
                LearningAttribution::Workflow,
            ),
            latency_ms: 4_250,
            total_tokens: 4_250,
            tool_calls: 2,
            successful_tools_by_step: [("approach_a".to_string(), vec!["file.search".to_string()])]
                .into_iter()
                .collect(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        },
        WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: slow_plan.clone(),
            succeeded: false,
            quality_score: Some(0.30),
            learning_evidence: test_quality_learning_evidence(
                0.30,
                false,
                LearningAttribution::Workflow,
            ),
            latency_ms: 80_000,
            total_tokens: 20_000,
            tool_calls: 8,
            successful_tools_by_step: BTreeMap::new(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        },
        WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: slow_plan,
            succeeded: true,
            quality_score: Some(0.45),
            learning_evidence: test_quality_learning_evidence(
                0.45,
                false,
                LearningAttribution::Workflow,
            ),
            latency_ms: 70_000,
            total_tokens: 18_000,
            tool_calls: 7,
            successful_tools_by_step: BTreeMap::new(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        },
    ];

    let teacher = WorkflowSearchTeacher::train(&telemetry);
    let prior = teacher
        .best_prior(&TaskClass::Research, "pro", &allowed_models, 2)
        .expect("a stable prior should be available");
    assert_eq!(prior.steps.len(), 3);
    assert_eq!(prior.examples, 4);
    assert_eq!(prior.success_rate, 1.0);
    assert!(prior.success_confidence >= LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE);
    assert!(prior.prompt_hint().contains("Treat this only as a prior"));
    assert_eq!(prior.steps[0].preferred_tools, vec!["file.search"]);
    assert!(prior
        .prompt_hint()
        .contains("observed_successful_tools=file.search"));
}

#[test]
fn search_teacher_withholds_under_evidenced_topology() {
    let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
    let plan = workflow_plan("under-evidenced", false);
    let telemetry = (0..3)
        .map(|_| WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: plan.clone(),
            succeeded: true,
            quality_score: Some(0.95),
            learning_evidence: test_quality_learning_evidence(
                0.95,
                true,
                LearningAttribution::Workflow,
            ),
            latency_ms: 4_000,
            total_tokens: 4_000,
            tool_calls: 2,
            successful_tools_by_step: BTreeMap::new(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        })
        .collect::<Vec<_>>();

    let teacher = WorkflowSearchTeacher::train(&telemetry);
    assert!(teacher
        .best_prior(&TaskClass::Research, "pro", &allowed_models, 2)
        .is_none());
}

#[test]
fn search_teacher_ignores_unmeasured_workflow_completions() {
    let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
    let plan = workflow_plan("unmeasured", false);
    let telemetry = (0..8)
        .map(|_| WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: String::new(),
            plan: plan.clone(),
            succeeded: true,
            quality_score: None,
            learning_evidence: LearningEvidenceV1::default(),
            latency_ms: 2_000,
            total_tokens: 2_000,
            tool_calls: 1,
            successful_tools_by_step: BTreeMap::new(),
            fallback_used: false,
            paired_team_score_bps: None,
            paired_anchor_score_bps: None,
            paired_uplift_bps: None,
            selected_anchor: false,
            anchor_latency_ms: None,
        })
        .collect::<Vec<_>>();

    let teacher = WorkflowSearchTeacher::train(&telemetry);
    assert!(teacher
        .best_prior(&TaskClass::Research, "pro", &allowed_models, 2)
        .is_none());
}

#[test]
fn matched_collaboration_teacher_preserves_direct_team_pairing() {
    let plan = workflow_plan("matched-evidence", false);
    let uplifts = [-1_000, -500, 500, 1_000];
    let telemetry = uplifts
        .into_iter()
        .enumerate()
        .map(|(index, uplift)| WorkflowExecutionTelemetry {
            task_class: TaskClass::Research,
            routing_signature: "research-shape".to_string(),
            plan: plan.clone(),
            succeeded: true,
            quality_score: Some(0.8),
            learning_evidence: if uplift < 0 {
                LearningEvidenceV1::censored(
                    LearningTermination::Completed,
                    LearningAttribution::Workflow,
                    LearningUsageCompleteness::Complete,
                    Some(0),
                    Some("0".repeat(64)),
                )
            } else {
                test_quality_learning_evidence(0.8, true, LearningAttribution::Workflow)
            },
            latency_ms: 4_000 + index as u64 * 100,
            total_tokens: 4_000,
            tool_calls: 0,
            successful_tools_by_step: BTreeMap::new(),
            fallback_used: uplift < 0,
            paired_team_score_bps: Some((8_000i32 + i32::from(uplift)) as u16),
            paired_anchor_score_bps: Some(8_000),
            paired_uplift_bps: Some(uplift),
            selected_anchor: uplift < 0,
            anchor_latency_ms: Some(1_000),
        })
        .collect::<Vec<_>>();

    let teacher = MatchedCollaborationEvidenceTeacher::train(&telemetry);
    let evidence = &teacher.calibrated_evidence()[0];

    assert!(!telemetry[0].learning_evidence.is_learnable());
    assert!(evidence.evidence_ready());
    assert_eq!(evidence.examples, 4);
    assert_eq!(evidence.team_wins, 2);
    assert_eq!(evidence.below_admission_floor, 2);
    assert_eq!(evidence.anchor_selections, 2);
    assert_eq!(evidence.average_uplift_bps, 0);
    assert_eq!(evidence.average_anchor_latency_ms, Some(1_000));
    assert!(evidence.prompt_hint().contains("matched_direct_team"));
    assert!(evidence.prompt_hint().contains("support=ready"));

    let mut inconsistent = telemetry[0].clone();
    inconsistent.paired_uplift_bps = Some(1);
    assert!(MatchedCollaborationEvidenceTeacher::train(&[inconsistent])
        .calibrated_evidence()
        .is_empty());
}

#[test]
fn conductor_harness_builds_context_and_parses_a_valid_plan() {
    let harness = ConductorHarness::new(conductor_request());
    let prompt = harness.planning_prompt();
    assert!(prompt.contains("Allowed worker pool:\n- planner\n- reviewer"));
    assert!(prompt.contains("Prefer two independent branches"));
    assert!(!prompt.contains("conductor-only"));
    assert!(prompt.matches(r#""model":"planner""#).count() >= 3);
    assert!(!prompt.contains(r#""model":"reviewer""#));
    assert!(prompt.contains(r#""role":"thinker""#));
    assert!(prompt.contains(r#""role":"worker""#));

    let plan = harness.parse_plan(
            r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"alternative","access":[]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b"]}]}"#,
        ).expect("plan should parse");
    assert_eq!(plan.coordinator_model, "conductor-only");
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(plan.schema, WORKFLOW_IR_SCHEMA);
}

#[test]
fn conductor_schema_keeps_required_branches_when_only_one_model_is_available() {
    let mut request = conductor_request();
    request.worker_models = vec!["planner".to_string()];
    request.role_hints = ConductorRoleHints {
        planner: "planner".to_string(),
        executor: "planner".to_string(),
        reviewer: "planner".to_string(),
        synthesizer: "planner".to_string(),
    };
    request.budget.max_models = 1;
    let prompt = ConductorHarness::new(request).planning_prompt();

    assert!(prompt.contains(r#""id":"approach_a""#));
    assert!(prompt.contains(r#""id":"approach_b""#));
    assert!(prompt.matches(r#""model":"planner""#).count() >= 3);
}

#[test]
fn conductor_harness_produces_a_bounded_repair_request() {
    let harness = ConductorHarness::new(conductor_request());
    let invalid = r#"{"steps":[{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":[]}]}"#;
    let error = harness
        .parse_plan(invalid)
        .expect_err("one branch should fail");
    let repair = harness.repair_prompt(invalid, &error);

    assert!(error.contains("at least 2 independent branches"));
    assert!(repair.contains("deterministic Cindx Harness"));
    assert!(repair.contains(&error));
}

#[test]
fn deterministic_pro_fallback_preserves_independent_review_and_synthesis() {
    let mut request = conductor_request();
    request.budget.max_steps = 5;
    request.execution_contract.verification_required = true;
    let harness = ConductorHarness::new(request);

    let plan = harness
        .fallback_plan()
        .expect("pro fallback should produce a valid collaboration graph");
    let roots = plan
        .steps
        .iter()
        .take(plan.steps.len() - 1)
        .filter(|step| step.access.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    assert!(roots.iter().all(|root| root.model == "planner"));
    let verifier = plan
        .steps
        .iter()
        .find(|step| step.role == "verifier")
        .expect("pro fallback should retain adversarial verification");
    assert!(roots.iter().all(|root| verifier.access.contains(&root.id)));
    assert_eq!(plan.steps.last().unwrap().role, "synthesizer");
    plan.validate(&harness.request().worker_models).unwrap();
    harness
        .request()
        .execution_contract
        .validate_plan(&plan)
        .unwrap();
}

#[test]
fn deterministic_auto_fallback_compares_two_independent_branches() {
    let routing = RoutingContext::from_prompt(
        "Compare two implementation strategies with evidence",
        Vec::new(),
    );
    let mut request = conductor_request();
    request.effort = "auto".to_string();
    request.prompt_genome =
        ConductorPromptGenome::seed_for_effort("auto").with_effort_delivery_contract("auto");
    request.execution_contract = ConductorExecutionContract::from_routing(
        &routing,
        "auto",
        OrchestrationPolicy::BestOfN { candidates: 2 },
    );
    let harness = ConductorHarness::new(request);

    let plan = harness
        .fallback_plan()
        .expect("auto fallback should produce a valid comparison graph");
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(
        plan.steps
            .iter()
            .take(2)
            .filter(|step| step.access.is_empty())
            .count(),
        2
    );
    assert_eq!(
        plan.steps.last().unwrap().access,
        vec!["approach_a".to_string(), "approach_b".to_string()]
    );
    plan.validate(&harness.request().worker_models).unwrap();
    harness
        .request()
        .execution_contract
        .validate_plan(&plan)
        .unwrap();
}

#[test]
fn conductor_harness_requires_distinct_work_but_not_distinct_models() {
    let mut request = conductor_request();
    request.budget.max_steps = 5;
    request.execution_contract.verification_required = true;
    let harness = ConductorHarness::new(request);
    harness
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary analysis","access":[]},{"id":"b","role":"worker","model":"planner","subtask":"independent implementation","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit both","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect("one capable model may perform genuinely different root tasks");

    let duplicate_subtask = harness
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"Inspect the design","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":" inspect   THE design ","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit both","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect_err("duplicate branch assignments should be rejected");
    assert!(duplicate_subtask.contains("repeat the same subtask"));

    let incomplete_review = harness
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary analysis","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"independent implementation","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit one branch","access":["a"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect_err("adversarial review should cover every root branch");
    assert!(
        incomplete_review.contains("directly audits every independent contribution"),
        "{incomplete_review}"
    );
}

#[test]
fn conductor_harness_enforces_the_selected_prompt_genome() {
    let mut lean_request = conductor_request();
    let lean_routing = RoutingContext::from_prompt("Compare implementation strategies", Vec::new());
    lean_request.effort = "fast".to_string();
    lean_request.policy = "single".to_string();
    lean_request.execution_contract = ConductorExecutionContract::from_routing(
        &lean_routing,
        "fast",
        OrchestrationPolicy::Single,
    );
    lean_request.prompt_genome = ConductorPromptGenome::seed_for_effort("fast");
    let lean = ConductorHarness::new(lean_request);
    let lean_plan = lean.parse_plan(
            r#"{"steps":[{"id":"final","role":"synthesizer","model":"planner","subtask":"direct answer","access":[]}]}"#,
        )
        .expect("lean profile should allow one direct branch");
    assert_eq!(lean_plan.budget.max_model_turns_per_step, 1);
    assert_eq!(lean_plan.budget.max_tool_calls_per_step, 0);
    assert_eq!(lean_plan.steps[0].tool_policy, WorkflowToolPolicy::None);

    let mut adversarial_request = conductor_request();
    adversarial_request.budget.max_steps = 5;
    adversarial_request.execution_contract.verification_required = true;
    let adversarial = ConductorHarness::new(adversarial_request);
    let error = adversarial
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"alternative","access":[]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b"]}]}"#,
            )
            .expect_err("adversarial profile should require a verifier");
    assert!(error.contains("requires a verification step"), "{error}");

    let pro_plan = adversarial
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"alternative","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"challenge","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect("pro profile should accept a verified graph");
    assert_eq!(pro_plan.budget.max_model_turns_per_step, 3);
    assert_eq!(pro_plan.budget.max_tool_calls_per_step, 6);
    assert_eq!(
        pro_plan.steps[0].tool_policy,
        WorkflowToolPolicy::ReadOnlyExploration
    );
    assert_eq!(
        pro_plan.steps.last().unwrap().tool_policy,
        WorkflowToolPolicy::None
    );
}

#[test]
fn conductor_harness_allows_a_single_step_pro_graph_for_a_simple_task() {
    let routing = RoutingContext::from_prompt("What is a Rust enum?", Vec::new());
    let mut request = conductor_request();
    request.objective = "What is a Rust enum?".to_string();
    request.execution_contract = ConductorExecutionContract::from_routing(
        &routing,
        "pro",
        OrchestrationPolicy::BestOfN { candidates: 3 },
    );
    request.prompt_genome = ConductorPromptGenome::seed_for_effort("pro");
    let harness = ConductorHarness::new(request);

    let plan = harness
        .parse_plan(
            r#"{"steps":[{"id":"final","role":"synthesizer","model":"planner","subtask":"produce the direct execution brief","access":[]}] }"#,
        )
        .expect("simple Pro should let the conductor choose the smallest useful graph");

    assert_eq!(plan.steps.len(), 1);
    assert!(harness
        .planning_prompt()
        .contains("requires 0 independent contribution(s)"));
}

#[test]
fn conductor_harness_applies_evolved_topology_and_role_strategies() {
    let mut serial_request = conductor_request();
    let serial_routing =
        RoutingContext::from_prompt("Compare implementation strategies", Vec::new());
    serial_request.effort = "fast".to_string();
    serial_request.policy = "single".to_string();
    serial_request.execution_contract = ConductorExecutionContract::from_routing(
        &serial_routing,
        "fast",
        OrchestrationPolicy::Single,
    );
    serial_request.prompt_genome = ConductorPromptGenome::seed_for_effort("fast");
    serial_request.prompt_genome.graph_depth = PromptGraphDepth::Balanced;
    let serial = ConductorHarness::new(serial_request);
    serial
            .parse_plan(
                r#"{"steps":[{"id":"work","role":"worker","model":"planner","subtask":"solve","access":[]},{"id":"final","role":"synthesizer","model":"planner","subtask":"report","access":["work"]}]}"#,
            )
            .expect("serial topology should accept one root chain");
    let parallel_error = serial
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"worker","model":"planner","subtask":"same","access":[]},{"id":"b","role":"worker","model":"planner","subtask":"same","access":[]},{"id":"final","role":"synthesizer","model":"planner","subtask":"report","access":["a","b"]}]}"#,
            )
            .expect_err("serial topology must reject parallel roots");
    assert!(
        parallel_error.contains("1-branch limit"),
        "{parallel_error}"
    );

    let mut flexible_request = conductor_request();
    flexible_request.budget.max_steps = 5;
    flexible_request.prompt_genome.role_strategy = PromptRoleStrategy::Flexible;
    let flexible = ConductorHarness::new(flexible_request);
    flexible
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"worker","model":"planner","subtask":"derive the primary design","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"test an alternative design","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"report","access":["a","b","verify"]}]}"#,
            )
            .expect("flexible roles should allow repeated worker roles across models");

    let diverse = ConductorHarness::new({
        let mut request = conductor_request();
        request.budget.max_steps = 5;
        request
    });
    let diversity_error = diverse
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"worker","model":"planner","subtask":"analysis","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"implementation","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"report","access":["a","b","verify"]}]}"#,
            )
            .expect_err("diverse specialists must include complementary root roles");
    assert!(
        diversity_error.contains("at least two complementary role labels"),
        "{diversity_error}"
    );
}

#[test]
fn labels_are_stable_for_persisted_traces() {
    assert_eq!(OrchestrationPolicy::Single.label(), "single");
    assert_eq!(
        OrchestrationPolicy::PlanExecuteReview.label(),
        "plan_execute_review"
    );
    assert_eq!(
        OrchestrationPolicy::BestOfN { candidates: 3 }.label(),
        "best_of_n"
    );
    assert_eq!(OrchestrationPolicy::AutoRouter.label(), "auto_router");
}

#[test]
fn plan_execute_review_has_expected_roles() {
    let plan = default_plan(OrchestrationPolicy::PlanExecuteReview);
    let roles: Vec<ModelRole> = plan.steps.into_iter().map(|step| step.role).collect();

    assert_eq!(
        roles,
        vec![ModelRole::Planner, ModelRole::Executor, ModelRole::Reviewer]
    );
}

#[test]
fn parses_policy_labels() {
    assert_eq!(parse_policy("single"), Some(OrchestrationPolicy::Single));
    assert_eq!(
        parse_policy("plan_execute_review"),
        Some(OrchestrationPolicy::PlanExecuteReview)
    );
    assert_eq!(
        parse_policy("best_of_n"),
        Some(OrchestrationPolicy::BestOfN { candidates: 3 })
    );
    assert_eq!(
        parse_policy("auto_router"),
        Some(OrchestrationPolicy::AutoRouter)
    );
    assert_eq!(parse_policy("unknown"), None);
}

#[test]
fn step_prompt_includes_previous_outputs() {
    let plan = default_plan(OrchestrationPolicy::PlanExecuteReview);
    let prompt = step_prompt(
        &plan,
        1,
        "Change README",
        &["Plan: inspect first".to_string()],
    )
    .expect("step should exist");

    assert!(prompt.contains("Role: executor"));
    assert!(prompt.contains("Change README"));
    assert!(prompt.contains("Plan: inspect first"));
}

#[test]
fn adaptive_workflow_builds_parallel_dependency_layers() {
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "research".to_string(),
                role: "thinker".to_string(),
                model: "strong-vision".to_string(),
                subtask: "Research the primary approach.".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "challenge".to_string(),
                role: "thinker".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Find independent failure modes.".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "verify".to_string(),
                role: "verifier".to_string(),
                model: "strong-vision".to_string(),
                subtask: "Verify the research.".to_string(),
                access: vec!["research".to_string()],
            },
            AdaptiveWorkflowStep {
                id: "synthesize".to_string(),
                role: "synthesizer".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Produce an execution brief.".to_string(),
                access: vec!["challenge".to_string(), "verify".to_string()],
            },
        ],
    };

    validate_adaptive_workflow(
        &workflow,
        &["fast-mini".to_string(), "strong-vision".to_string()],
    )
    .expect("workflow should be valid");
    assert_eq!(
        adaptive_workflow_layers(&workflow).expect("layers should build"),
        vec![vec![0, 1], vec![2], vec![3]]
    );
}

#[test]
fn fugu_step_budget_reuses_a_bounded_worker_pool() {
    assert_eq!(adaptive_workflow_step_budget(1), 1);
    assert_eq!(adaptive_workflow_step_budget(2), 3);
    assert_eq!(adaptive_workflow_step_budget(3), 5);
    assert_eq!(adaptive_workflow_step_budget(99), 5);
}

#[test]
fn adaptive_workflow_rejects_a_branch_omitted_from_synthesis() {
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "included".to_string(),
                role: "thinker".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Included branch".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "orphaned".to_string(),
                role: "worker".to_string(),
                model: "strong-vision".to_string(),
                subtask: "Contradicting branch".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Synthesize".to_string(),
                access: vec!["included".to_string()],
            },
        ],
    };

    let error = validate_adaptive_workflow(
        &workflow,
        &["fast-mini".to_string(), "strong-vision".to_string()],
    )
    .expect_err("orphaned branch should be rejected");

    assert!(error.contains("incorporate every branch"));
    assert!(error.contains("orphaned"));
}

#[test]
fn adaptive_worker_only_receives_authorized_outputs() {
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "allowed".to_string(),
                role: "thinker".to_string(),
                model: "fast-mini".to_string(),
                subtask: "First branch.".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "isolated".to_string(),
                role: "worker".to_string(),
                model: "strong-vision".to_string(),
                subtask: "Independent branch.".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "consumer".to_string(),
                role: "synthesizer".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Use one branch.".to_string(),
                access: vec!["allowed".to_string()],
            },
        ],
    };
    let outputs = [
        ("allowed".to_string(), "VISIBLE_FINDING".to_string()),
        ("isolated".to_string(), "HIDDEN_FINDING".to_string()),
    ]
    .into_iter()
    .collect();

    let prompt = adaptive_worker_prompt(&workflow, 2, "Investigate", "prior turn", &outputs)
        .expect("worker prompt should build");

    assert!(prompt.contains("VISIBLE_FINDING"));
    assert!(!prompt.contains("HIDDEN_FINDING"));
    assert!(prompt.contains("Integrated execution brief"));
    assert!(prompt.contains("Do not merely restate authorized outputs"));

    let thinker = adaptive_worker_prompt(&workflow, 0, "Investigate", "", &BTreeMap::new())
        .expect("thinker prompt should build");
    let worker = adaptive_worker_prompt(&workflow, 1, "Investigate", "", &BTreeMap::new())
        .expect("worker prompt should build");
    assert!(thinker.contains("Hypotheses; Assumptions"));
    assert!(worker.contains("Evidence; Provenance; Findings; Uncertainty; Handoff"));
    assert_ne!(thinker, worker);
}

#[test]
fn adaptive_worker_context_is_bounded_without_losing_conclusions() {
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "source".to_string(),
                role: "worker".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Investigate.".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "consumer".to_string(),
                role: "synthesizer".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Synthesize.".to_string(),
                access: vec!["source".to_string()],
            },
        ],
    };
    let long_output = format!(
        "BEGIN_EVIDENCE{}FINAL_CONCLUSION",
        "x".repeat(ADAPTIVE_WORKER_DEPENDENCY_MAX_CHARS * 2)
    );
    let outputs = BTreeMap::from([("source".to_string(), long_output)]);

    let prompt = adaptive_worker_prompt(
        &workflow,
        1,
        "Investigate",
        &"m".repeat(ADAPTIVE_WORKER_SHARED_MEMORY_MAX_CHARS * 2),
        &outputs,
    )
    .expect("bounded worker prompt should build");

    assert!(prompt.contains("BEGIN_EVIDENCE"));
    assert!(prompt.contains("FINAL_CONCLUSION"));
    assert!(prompt.contains("characters omitted by the workflow context boundary"));
    assert!(prompt.chars().count() < 60_000);
}

#[test]
fn adaptive_workflow_rejects_forward_access_and_unknown_models() {
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "first".to_string(),
                role: "thinker".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Try to read the future.".to_string(),
                access: vec!["later".to_string()],
            },
            AdaptiveWorkflowStep {
                id: "later".to_string(),
                role: "synthesizer".to_string(),
                model: "fast-mini".to_string(),
                subtask: "Later work.".to_string(),
                access: vec!["first".to_string()],
            },
        ],
    };

    let error = validate_adaptive_workflow(&workflow, &["fast-mini".to_string()])
        .expect_err("workflow should be rejected");
    assert!(error.contains("only access earlier steps"));

    let mut unknown_model_workflow = workflow;
    unknown_model_workflow.steps[0].access.clear();
    unknown_model_workflow.steps[0].model = "unknown".to_string();
    let error = validate_adaptive_workflow(&unknown_model_workflow, &["fast-mini".to_string()])
        .expect_err("unknown model should be rejected");
    assert!(error.contains("unknown model"));
}

#[test]
fn adaptive_workflow_rejects_more_than_three_models() {
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "one".to_string(),
                role: "thinker".to_string(),
                model: "model-a".to_string(),
                subtask: "First branch.".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "two".to_string(),
                role: "worker".to_string(),
                model: "model-b".to_string(),
                subtask: "Second branch.".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "three".to_string(),
                role: "verifier".to_string(),
                model: "model-c".to_string(),
                subtask: "Audit both branches.".to_string(),
                access: vec!["one".to_string(), "two".to_string()],
            },
            AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "model-d".to_string(),
                subtask: "Synthesize the result.".to_string(),
                access: vec!["one".to_string(), "two".to_string(), "three".to_string()],
            },
        ],
    };
    let models = ["model-a", "model-b", "model-c", "model-d"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();

    let error = validate_adaptive_workflow(&workflow, &models)
        .expect_err("four models should exceed the bounded agent budget");

    assert!(error.contains("3-agent budget"));
}

#[test]
fn rule_router_explains_four_way_retrieval_choice() {
    let context =
        RoutingContext::from_prompt("Search the docs with RAG and cite sources", candidates());
    let router = RuleBasedRouter;
    let decision = router.route(&context);

    assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
    assert_eq!(decision.retrieval_mode, "four_way_parallel");
    assert!(decision.explanation.contains("class=retrieval"));
}

#[test]
fn chinese_collaboration_request_routes_to_real_ensemble() {
    let context =
        RoutingContext::from_prompt("分析多个模型协同，并对标 Sakana Fugu Ultra", candidates());
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(context.task_class, TaskClass::Research);
    assert!(context.needs_multi_model);
    assert_eq!(
        decision.policy,
        OrchestrationPolicy::BestOfN { candidates: 3 }
    );
}

#[test]
fn fugu_reproduction_request_routes_to_adaptive_ensemble() {
    let context = RoutingContext::from_prompt("继续完善 Sakana Fugu Ultra 的复现", candidates());
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(context.task_class, TaskClass::Research);
    assert!(context.needs_multi_model);
    assert_eq!(
        decision.policy,
        OrchestrationPolicy::BestOfN { candidates: 3 }
    );
}

#[test]
fn ordinary_research_uses_one_planned_execution_path() {
    let context = RoutingContext::from_prompt("比较两个产品路线的优缺点", candidates());
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(context.task_class, TaskClass::Research);
    assert!(!context.needs_multi_model);
    assert!(!context.needs_retrieval);
    assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
}

#[test]
fn high_stakes_architecture_work_routes_to_three_experts() {
    let context = RoutingContext::from_prompt(
            "Compare production migration architectures, investigate root causes, and propose a safe strategy",
            candidates(),
        );
    let decision = RuleBasedRouter.route(&context);

    assert!(context.high_stakes);
    assert!(context.complexity_score >= 3);
    assert_eq!(
        decision.policy,
        OrchestrationPolicy::BestOfN { candidates: 3 }
    );
}

#[test]
fn complex_non_critical_work_routes_to_two_experts() {
    let context = RoutingContext::from_prompt(
        "Investigate the root cause in this project, edit the files, and run tests",
        candidates(),
    );
    let decision = RuleBasedRouter.route(&context);

    assert!(!context.high_stakes);
    assert!(context.complexity_score >= 3);
    assert_eq!(
        decision.policy,
        OrchestrationPolicy::BestOfN { candidates: 2 }
    );
}

#[test]
fn chinese_tool_request_is_not_misclassified_as_general_chat() {
    let context = RoutingContext::from_prompt("你能不能修复这个项目并运行测试", candidates());
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(context.task_class, TaskClass::Coding);
    assert!(context.needs_tools);
    assert!(context.needs_retrieval);
    assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
    assert_eq!(decision.retrieval_mode, "semantic_literal_parallel");
}

#[test]
fn coding_capability_question_stays_direct_without_retrieval() {
    let context = RoutingContext::from_prompt("你会不会写代码", candidates());
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(context.task_class, TaskClass::General);
    assert!(!context.needs_tools);
    assert!(!context.needs_retrieval);
    assert!(!context.needs_vision);
    assert_eq!(decision.policy, OrchestrationPolicy::Single);
    assert_eq!(decision.retrieval_mode, "none");
}

#[test]
fn long_self_contained_multiple_choice_question_stays_direct() {
    let prompt = format!(
        "What is the correct answer to this question?\n{}\n\n(A) one\n(B) two\n(C) three\n(D) four\n\nFormat your response as follows: The correct answer is (insert answer here)",
        "A self-contained scientific premise with all facts supplied in the question. ".repeat(12)
    );
    let context = RoutingContext::from_prompt(&prompt, candidates());
    let decision = RuleBasedRouter.route(&context);

    assert!(context.prompt_length > 600);
    assert_eq!(context.task_class, TaskClass::General);
    assert!(!context.needs_tools);
    assert!(!context.needs_retrieval);
    assert!(!context.verification_required);
    assert_eq!(decision.policy, OrchestrationPolicy::Single);
}

#[test]
fn short_coding_explanation_does_not_require_workspace_context() {
    let context = RoutingContext::from_prompt("解释一下 Rust 所有权代码", candidates());
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(context.task_class, TaskClass::Coding);
    assert!(!context.needs_tools);
    assert!(!context.needs_retrieval);
    assert_eq!(decision.policy, OrchestrationPolicy::Single);
}

#[test]
fn rule_router_respects_user_override() {
    let mut context =
        RoutingContext::from_prompt("Research three implementation options", candidates());
    context.user_policy_override = Some(OrchestrationPolicy::Single);
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(decision.policy, OrchestrationPolicy::Single);
    assert!(decision.explanation.contains("user override"));
}

#[test]
fn learned_router_uses_successful_trace_table() {
    let context =
        RoutingContext::from_prompt("Research and compare local agent routers", candidates());
    let context_signature = context.learning_signature();
    let mut telemetry = (0..4)
        .map(|_| RoutingTelemetry {
            task_class: TaskClass::Research,
            context_signature: context_signature.clone(),
            selected_policy: OrchestrationPolicy::PlanExecuteReview,
            selected_model: "strong-vision".to_string(),
            latency_ms: 900,
            outcome: RoutingOutcome::Succeeded,
            quality_score: None,
            verification_passed: None,
            learning_evidence: test_verified_learning_evidence(),
            cost_proxy: 120,
            tool_count: 0,
            retrieval_count: 2,
            user_override: false,
        })
        .collect::<Vec<_>>();
    telemetry.push(RoutingTelemetry {
        task_class: TaskClass::Research,
        context_signature,
        selected_policy: OrchestrationPolicy::Single,
        selected_model: "fast-mini".to_string(),
        latency_ms: 200,
        outcome: RoutingOutcome::Failed,
        quality_score: None,
        verification_passed: None,
        learning_evidence: test_quality_learning_evidence(0.0, false, LearningAttribution::Model),
        cost_proxy: 20,
        tool_count: 0,
        retrieval_count: 0,
        user_override: false,
    });
    let router = LearnedModelRouter::train(&telemetry);
    let decision = router.route(&context);

    assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
    assert_eq!(decision.model, "strong-vision");
    assert!(decision.explanation.contains("policy=plan_execute_review"));
    assert!(decision.explanation.contains("learned_model=strong-vision"));
    assert_eq!(
        decision.metadata.get("router").map(String::as_str),
        Some("learned_conductor_v1")
    );
}

#[test]
fn learned_router_can_downshift_a_matching_context_without_tools() {
    let prompt = "Compare two product strategies and explain the tradeoffs.";
    let context = RoutingContext::from_prompt(prompt, candidates());
    assert_eq!(
        RuleBasedRouter.route(&context).policy,
        OrchestrationPolicy::PlanExecuteReview
    );
    let telemetry = (0..4)
        .map(|_| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: context.learning_signature(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 250,
            outcome: RoutingOutcome::Succeeded,
            quality_score: None,
            verification_passed: None,
            learning_evidence: test_verified_learning_evidence(),
            cost_proxy: 80,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        })
        .collect::<Vec<_>>();

    let decision = LearnedModelRouter::train(&telemetry).route(&context);
    assert_eq!(decision.policy, OrchestrationPolicy::Single);
    assert_eq!(decision.model, "fast-mini");
    assert!(decision.explanation.contains("learned_policy=single"));
}

#[test]
fn learned_router_does_not_overfit_three_successful_traces() {
    let prompt = "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
    let context = RoutingContext::from_prompt(&prompt, candidates());
    let telemetry = (0..3)
        .map(|_| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: context.learning_signature(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 250,
            outcome: RoutingOutcome::Succeeded,
            quality_score: None,
            verification_passed: None,
            learning_evidence: test_verified_learning_evidence(),
            cost_proxy: 80,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        })
        .collect::<Vec<_>>();

    let router = LearnedModelRouter::train(&telemetry);
    let route = router
        .learned_route_for_context(&context)
        .expect("route evidence should remain observable");
    assert!(!route.evidence_ready());
    assert_eq!(router.route(&context), RuleBasedRouter.route(&context));
}

#[test]
fn learned_router_ignores_censored_lifecycle_outcomes() {
    let prompt = "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
    let context = RoutingContext::from_prompt(&prompt, candidates());
    let telemetry = (0..8)
        .map(|index| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: context.learning_signature(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 250,
            outcome: if index % 2 == 0 {
                RoutingOutcome::Succeeded
            } else {
                RoutingOutcome::Failed
            },
            quality_score: None,
            verification_passed: None,
            learning_evidence: LearningEvidenceV1::default(),
            cost_proxy: 80,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        })
        .collect::<Vec<_>>();

    let router = LearnedModelRouter::train(&telemetry);
    assert!(router.learned_route_for_context(&context).is_none());
    assert_eq!(router.route(&context), RuleBasedRouter.route(&context));
}

#[test]
fn learned_router_does_not_transfer_coarse_task_class_evidence() {
    let prompt = "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
    let context = RoutingContext::from_prompt(&prompt, candidates());
    let telemetry = (0..4)
        .map(|_| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: TaskClass::General.label().to_string(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 250,
            outcome: RoutingOutcome::Succeeded,
            quality_score: None,
            verification_passed: None,
            learning_evidence: test_verified_learning_evidence(),
            cost_proxy: 80,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        })
        .collect::<Vec<_>>();

    let router = LearnedModelRouter::train(&telemetry);
    assert!(router.learned_route_for_context(&context).is_none());
    assert_eq!(router.route(&context), RuleBasedRouter.route(&context));
}

#[test]
fn learned_router_prefers_reliable_route_over_more_raw_successes() {
    let prompt = "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
    let context = RoutingContext::from_prompt(&prompt, candidates());
    let signature = context.learning_signature();
    let mut telemetry = (0..4)
        .map(|_| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: signature.clone(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 250,
            outcome: RoutingOutcome::Succeeded,
            quality_score: None,
            verification_passed: None,
            learning_evidence: test_verified_learning_evidence(),
            cost_proxy: 80,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        })
        .collect::<Vec<_>>();
    telemetry.extend((0..10).map(|index| RoutingTelemetry {
        task_class: TaskClass::General,
        context_signature: signature.clone(),
        selected_policy: OrchestrationPolicy::PlanExecuteReview,
        selected_model: "strong-vision".to_string(),
        latency_ms: 900,
        outcome: if index < 5 {
            RoutingOutcome::Succeeded
        } else {
            RoutingOutcome::Failed
        },
        quality_score: None,
        verification_passed: None,
        learning_evidence: if index < 5 {
            test_verified_learning_evidence()
        } else {
            test_quality_learning_evidence(0.0, false, LearningAttribution::Model)
        },
        cost_proxy: 120,
        tool_count: 0,
        retrieval_count: 0,
        user_override: false,
    }));

    let decision = LearnedModelRouter::train(&telemetry).route(&context);
    assert_eq!(decision.policy, OrchestrationPolicy::Single);
    assert_eq!(decision.model, "fast-mini");
}

#[test]
fn learned_router_rejects_nominal_success_without_quality() {
    let prompt = "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
    let context = RoutingContext::from_prompt(&prompt, candidates());
    let signature = context.learning_signature();
    let mut telemetry = (0..6)
        .map(|_| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: signature.clone(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 120,
            outcome: RoutingOutcome::Succeeded,
            quality_score: Some(0.42),
            verification_passed: Some(true),
            learning_evidence: test_quality_learning_evidence(
                0.42,
                false,
                LearningAttribution::Model,
            ),
            cost_proxy: 20,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        })
        .collect::<Vec<_>>();
    telemetry.extend((0..6).map(|_| RoutingTelemetry {
        task_class: TaskClass::General,
        context_signature: signature.clone(),
        selected_policy: OrchestrationPolicy::PlanExecuteReview,
        selected_model: "strong-vision".to_string(),
        latency_ms: 1_200,
        outcome: RoutingOutcome::Succeeded,
        quality_score: Some(0.91),
        verification_passed: Some(true),
        learning_evidence: test_quality_learning_evidence(0.91, true, LearningAttribution::Model),
        cost_proxy: 240,
        tool_count: 0,
        retrieval_count: 0,
        user_override: false,
    }));

    let router = LearnedModelRouter::train(&telemetry);
    let decision = router.route(&context);

    assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
    assert_eq!(decision.model, "strong-vision");
    assert_eq!(
        decision
            .metadata
            .get("learned_quality_score")
            .map(String::as_str),
        Some("0.910")
    );
    assert_eq!(
        decision
            .metadata
            .get("learned_verification_rate")
            .map(String::as_str),
        Some("1.000")
    );
}

#[test]
fn latency_sensitive_complex_request_does_not_spawn_an_ensemble() {
    let context = RoutingContext::from_prompt(
        "Quickly compare implementation alternatives and check the project",
        candidates(),
    );
    let decision = RuleBasedRouter.route(&context);

    assert!(context.latency_sensitive);
    assert!(context.parallelizable);
    assert_ne!(
        decision.policy,
        OrchestrationPolicy::BestOfN { candidates: 2 }
    );
    assert_ne!(
        decision.policy,
        OrchestrationPolicy::BestOfN { candidates: 3 }
    );
}

#[test]
fn auto_router_selects_models_by_task_role() {
    let role_candidates = vec![
        ModelCandidate {
            name: "planner-model".to_string(),
            role: ModelRole::Planner,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 3,
            latency_tier: 2,
        },
        ModelCandidate {
            name: "executor-model".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 2,
            latency_tier: 1,
        },
        ModelCandidate {
            name: "reviewer-model".to_string(),
            role: ModelRole::Reviewer,
            supports_tools: true,
            supports_vision: true,
            cost_tier: 4,
            latency_tier: 3,
        },
    ];
    let research = RoutingContext::from_prompt(
        "Research and compare local agent architectures",
        role_candidates.clone(),
    );
    let high_stakes = RoutingContext::from_prompt(
        "Review a critical production migration strategy",
        role_candidates,
    );

    assert_eq!(RuleBasedRouter.route(&research).model, "planner-model");
    assert_eq!(RuleBasedRouter.route(&high_stakes).model, "reviewer-model");
}

#[test]
fn learned_router_cannot_upgrade_a_lightweight_coding_question() {
    let context = RoutingContext::from_prompt("解释一下 Rust 所有权代码", candidates());
    let router = LearnedModelRouter::train(&[RoutingTelemetry {
        task_class: TaskClass::Coding,
        context_signature: context.learning_signature(),
        selected_policy: OrchestrationPolicy::PlanExecuteReview,
        selected_model: "strong-vision".to_string(),
        latency_ms: 10_000,
        outcome: RoutingOutcome::Succeeded,
        quality_score: None,
        verification_passed: None,
        learning_evidence: test_verified_learning_evidence(),
        cost_proxy: 1_000,
        tool_count: 4,
        retrieval_count: 4,
        user_override: false,
    }]);
    let decision = router.route(&context);

    assert_eq!(decision.policy, OrchestrationPolicy::Single);
    assert!(!decision.explanation.contains("learned_policy"));
}

#[test]
fn learned_router_cannot_upgrade_ordinary_research_to_ultra() {
    let context = RoutingContext::from_prompt("比较两个产品路线的优缺点", candidates());
    let router = LearnedModelRouter::train(&[RoutingTelemetry {
        task_class: TaskClass::Research,
        context_signature: context.learning_signature(),
        selected_policy: OrchestrationPolicy::BestOfN { candidates: 3 },
        selected_model: "strong-vision".to_string(),
        latency_ms: 30_000,
        outcome: RoutingOutcome::Succeeded,
        quality_score: None,
        verification_passed: None,
        learning_evidence: test_verified_learning_evidence(),
        cost_proxy: 4_000,
        tool_count: 0,
        retrieval_count: 4,
        user_override: false,
    }]);
    let decision = router.route(&context);

    assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
    assert!(!decision.explanation.contains("learned_policy"));
}

#[test]
fn evaluation_report_compares_against_baseline_policy() {
    let router = LearnedModelRouter::train(&[]);
    let contexts = vec![
        RoutingContext::from_prompt("hello", candidates()),
        RoutingContext::from_prompt("Use computer screenshot to inspect the UI", candidates()),
    ];
    let report = evaluate_router_against_baseline(&router, &contexts, OrchestrationPolicy::Single);

    assert_eq!(report.examples, 2);
    assert_eq!(report.baseline_policy, "single");
    assert_eq!(report.router_policy_matches_baseline, 1);
    assert_eq!(report.router_policy_differs_from_baseline, 1);
    assert!(report.summary.contains("baseline single"));
}

#[test]
fn english_workspace_actions_are_classified_as_coding() {
    for prompt in [
        "Implement a parser in this project and run tests.",
        "Edit these files to add pagination and check the result.",
        "Implement this function in the existing codebase.",
    ] {
        let context = RoutingContext::from_prompt(prompt, candidates());
        assert_eq!(context.task_class, TaskClass::Coding, "{prompt}");
        assert!(context.needs_tools, "{prompt}");
        assert!(context.needs_retrieval, "{prompt}");
    }
}

#[test]
fn chinese_multi_phase_root_cause_work_routes_to_two_experts() {
    let context =
        RoutingContext::from_prompt("排查这个项目的根因，修改文件并运行测试。", candidates());
    let decision = RuleBasedRouter.route(&context);

    assert_eq!(context.task_class, TaskClass::Coding);
    assert_eq!(
        decision.policy,
        OrchestrationPolicy::BestOfN { candidates: 2 }
    );
    assert_eq!(decision.retrieval_mode, "four_way_parallel");
}

#[test]
fn evaluation_lab_detects_over_and_under_orchestration() {
    let cases = vec![
        RoutingEvalCase {
            id: "under".to_string(),
            context: RoutingContext::from_prompt("hello", candidates()),
            expected_policy: OrchestrationPolicy::BestOfN { candidates: 3 },
            expected_retrieval_mode: "none".to_string(),
            expected_model: None,
        },
        RoutingEvalCase {
            id: "over".to_string(),
            context: RoutingContext::from_prompt(
                "Use multiple models to reproduce Fugu Ultra",
                candidates(),
            ),
            expected_policy: OrchestrationPolicy::Single,
            expected_retrieval_mode: "none".to_string(),
            expected_model: None,
        },
    ];

    let report = evaluate_routing_cases(&cases);

    assert_eq!(report.cases, 2);
    assert_eq!(report.passed, 0);
    assert_eq!(report.over_orchestrated, 1);
    assert_eq!(report.under_orchestrated, 1);
    assert_eq!(report.failures.len(), 2);
}

#[test]
fn operational_evaluation_aggregates_trace_cost_and_outcomes() {
    let telemetry = vec![
        RoutingTelemetry {
            task_class: TaskClass::Coding,
            context_signature: "coding".to_string(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 100,
            outcome: RoutingOutcome::Succeeded,
            quality_score: None,
            verification_passed: None,
            learning_evidence: LearningEvidenceV1::default(),
            cost_proxy: 20,
            tool_count: 1,
            retrieval_count: 0,
            user_override: false,
        },
        RoutingTelemetry {
            task_class: TaskClass::Research,
            context_signature: "research".to_string(),
            selected_policy: OrchestrationPolicy::BestOfN { candidates: 2 },
            selected_model: "strong-vision".to_string(),
            latency_ms: 500,
            outcome: RoutingOutcome::Failed,
            quality_score: None,
            verification_passed: None,
            learning_evidence: LearningEvidenceV1::default(),
            cost_proxy: 180,
            tool_count: 3,
            retrieval_count: 4,
            user_override: false,
        },
    ];

    let report = evaluate_routing_telemetry(&telemetry);

    assert_eq!(report.runs, 2);
    assert_eq!(report.succeeded, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(report.success_rate, 0.5);
    assert_eq!(report.average_latency_ms, 300);
    assert_eq!(report.average_cost_proxy, 100);
    assert_eq!(report.average_tool_calls, 2.0);
    assert_eq!(report.average_retrievals, 2.0);
    assert_eq!(report.policy_counts.get("single"), Some(&1));
    assert_eq!(report.policy_counts.get("best_of_n"), Some(&1));
}

#[test]
fn quality_rubric_requires_balanced_evidence_and_safety() {
    let strong = QualityRubricScore {
        correctness: 4,
        evidence: 4,
        completion: 4,
        safety: 4,
    };
    let unsupported = QualityRubricScore {
        correctness: 5,
        evidence: 2,
        completion: 5,
        safety: 5,
    };

    assert!(strong.passes());
    assert!(!unsupported.passes());
    assert!(QualityRubricScore {
        correctness: 6,
        evidence: 4,
        completion: 4,
        safety: 4,
    }
    .validate()
    .is_err());
}
