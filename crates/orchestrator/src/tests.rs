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
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
            cost_tier: 1,
            latency_tier: 1,
        },
        ModelCandidate {
            name: "strong-vision".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
            cost_tier: 4,
            latency_tier: 3,
        },
    ]
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
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
            cost_tier: 3,
            latency_tier: 2,
        },
        ModelCandidate {
            name: "executor-model".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
            cost_tier: 2,
            latency_tier: 1,
        },
        ModelCandidate {
            name: "reviewer-model".to_string(),
            role: ModelRole::Reviewer,
            supports_tools: true,
            supports_vision: true,
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
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
    assert_eq!(RuleBasedRouter.route(&high_stakes).model, "planner-model");
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
