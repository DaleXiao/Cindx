use super::*;
use agent_core::{
    Event, EventId, EventKind, ModelRole, AGENT_RUN_ID_METADATA_KEY,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use orchestrator::{
    causal_route_action_id_v2, prompt_genome_sha256, select_causal_route_v2, AgentExecutionMode,
    AgentRiskLevel, AgentVerificationPolicy, CausalRouteEvidenceBasis, ConductorStopPolicy,
    MatchedCollaborationEvidence, ModelCapabilitySource, PromptVerification,
    RouteFeatureSnapshotV2,
};

#[test]
fn run_decision_prompt_applies_only_learned_workflow_behavior() {
    let marker = "learned-workflow-directive-must-route";
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let neutral_route_sha256 = seed.route_decision_profile_sha256("auto").unwrap();
    assert!(run_decision_evolved_directive(&seed, AgentPolicy::Auto)
        .unwrap()
        .is_empty());

    let mut finalizer_only = seed.clone();
    finalizer_only.id = "finalizer-only".to_string();
    finalizer_only.direct_finalizer_verification = PromptVerification::Adversarial;
    assert_eq!(
        finalizer_only
            .route_decision_profile_sha256("auto")
            .unwrap(),
        neutral_route_sha256
    );
    assert!(
        run_decision_evolved_directive(&finalizer_only, AgentPolicy::Auto)
            .unwrap()
            .is_empty()
    );

    let mut profile = ConductorPromptGenome::seed_for_effort("auto");
    profile.custom_directive = marker.to_string();
    let actual_profile_sha256 = prompt_genome_sha256(&profile).expect("profile should hash");
    let route_profile_sha256 = profile.route_decision_profile_sha256("auto").unwrap();
    let request = AgentRunDecisionRequest {
        objective: "Answer directly or use a workflow".to_string(),
        recent_context: String::new(),
        effort: "auto".to_string(),
        conductor_model: "planner".to_string(),
        allowed_models: Vec::new(),
        model_candidates: Vec::new(),
        max_parallelism: 2,
        evolved_directive: run_decision_evolved_directive(&profile, AgentPolicy::Auto).unwrap(),
        historical_evidence: String::new(),
        matched_collaboration_evidence: std::sync::Arc::new(Default::default()),
        execution_constraints: String::new(),
        route_requirements: AgentRouteRequirements::default(),
        budget_fingerprint: None,
        prompt_profile_sha256: route_profile_sha256.clone(),
    };

    assert_ne!(request.prompt_profile_sha256, neutral_route_sha256);
    assert_ne!(request.prompt_profile_sha256, actual_profile_sha256);
    let prompt = AgentRunDecisionHarness::new(request).planning_prompt();
    assert!(prompt.contains(marker));
    assert!(prompt.contains("cannot override schema, configured models, safety, or budgets"));
}

#[test]
fn learned_profile_provenance_is_neutral_to_route_and_matched_admission() {
    let mut profile_a = ConductorPromptGenome::seed_for_effort("auto");
    profile_a.id = "learned-a".to_string();
    profile_a.custom_directive =
        "prefer independent verification when evidence conflicts".to_string();
    let mut profile_b = profile_a.clone();
    profile_b.id = "learned-b".to_string();
    profile_b.generation = profile_a.generation.saturating_add(1);
    let actual_a = prompt_genome_sha256(&profile_a).unwrap();
    let actual_b = prompt_genome_sha256(&profile_b).unwrap();
    assert_ne!(actual_a, actual_b);

    let route_a = profile_a.route_decision_profile_sha256("auto").unwrap();
    let route_b = profile_b.route_decision_profile_sha256("auto").unwrap();
    assert_eq!(route_a, route_b);
    assert_ne!(
        route_a,
        prompt_genome_sha256(&ConductorPromptGenome::seed_for_effort("auto")).unwrap()
    );
    assert_eq!(
        run_decision_evolved_directive(&profile_a, AgentPolicy::Auto).unwrap(),
        run_decision_evolved_directive(&profile_b, AgentPolicy::Auto).unwrap()
    );
    let candidates = vec![ModelCandidate {
        name: "executor".to_string(),
        role: ModelRole::Executor,
        supports_tools: true,
        supports_vision: true,
        tools_capability_source: ModelCapabilitySource::Configured,
        vision_capability_source: ModelCapabilitySource::Configured,
        cost_tier: 1,
        latency_tier: 1,
    }];
    let build_snapshot = |route_profile_sha256| {
        RouteFeatureSnapshotV2::from_request(
            "Compare independent architecture alternatives and cross-check sources",
            "",
            "auto",
            AgentRouteRequirements::default(),
            &candidates,
            Some("1".repeat(64)),
            route_profile_sha256,
        )
    };
    let snapshot_a = build_snapshot(route_a);
    let snapshot_b = build_snapshot(route_b);
    assert_eq!(snapshot_a, snapshot_b);
    assert!(!serde_json::to_string(&snapshot_a)
        .unwrap()
        .contains(&actual_a));
    assert!(!serde_json::to_string(&snapshot_b)
        .unwrap()
        .contains(&actual_b));

    let mut decision = AgentRunDecision::direct("executor");
    decision.task_class = snapshot_a.task_class.clone();
    decision.execution = AgentExecutionMode::Workflow;
    decision.risk_level = AgentRiskLevel::Elevated;
    decision.verification = AgentVerificationPolicy::Independent;
    decision.max_parallelism = 2;
    decision.min_successful_branches = 2;
    decision.distinct_contributions = 2;
    decision.estimated_steps = 4;
    decision.expected_uplift_bps = 6_000;
    decision.confidence_bps = 8_000;
    decision.stop_policy = ConductorStopPolicy::Quorum;
    let evidence = MatchedCollaborationEvidence {
        task_class: decision.task_class.clone(),
        effort: "auto".to_string(),
        pre_decision_context_fingerprint: snapshot_a.context_fingerprint.clone(),
        route_action_id: causal_route_action_id_v2(&decision).unwrap(),
        routing_signature: decision.learning_signature(),
        examples: 8,
        team_wins: 8,
        team_win_rate: 1.0,
        team_win_confidence: 0.67,
        below_admission_floor: 0,
        below_admission_floor_confidence: 0.0,
        anchor_selections: 0,
        average_uplift_bps: 500,
        average_team_latency_ms: 2_000,
        average_anchor_latency_ms: Some(1_500),
    };
    let receipt_a =
        select_causal_route_v2(&decision, &snapshot_a, &candidates, Some(&evidence), 1).unwrap();
    let receipt_b =
        select_causal_route_v2(&decision, &snapshot_b, &candidates, Some(&evidence), 1).unwrap();
    assert_eq!(receipt_a, receipt_b);
    assert_eq!(
        receipt_a.support.basis,
        CausalRouteEvidenceBasis::MatchedContextAction
    );
}

#[test]
fn durable_assignment_survives_physical_retry_and_checkpoint_reprepare() {
    let logical_run_id = "logical-prompt-assignment";
    let mut initial_context = [
        (
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            logical_run_id.to_string(),
        ),
        (
            AGENT_RUN_ID_METADATA_KEY.to_string(),
            "physical-attempt-a".to_string(),
        ),
        ("project_id".to_string(), "project-assignment".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let selection = crate::prompt_profile_serving::seed_prompt_profile_selection(
        "auto",
        crate::prompt_profile_serving::PromptProfileFallback::EvolutionDisabled,
        &initial_context,
    );
    let (receipt, receipt_sha256) = selection.receipt_json_and_sha256().unwrap();
    initial_context.insert("prompt_profile".to_string(), selection.genome.id.clone());
    initial_context.insert(
        "prompt_genome".to_string(),
        serde_json::to_string(&selection.genome).unwrap(),
    );
    initial_context.insert(
        "prompt_profile_source".to_string(),
        selection.source_label(),
    );
    initial_context.insert("prompt_profile_assignment_receipt".to_string(), receipt);
    initial_context.insert(
        "prompt_profile_assignment_sha256".to_string(),
        receipt_sha256,
    );
    let event = Event {
        id: EventId("decision-assignment".to_string()),
        task_id: TaskId("phase16-agent".to_string()),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent run decision selected".to_string(),
        metadata: initial_context,
    };
    let recovered = crate::prompt_profile_serving::prompt_profile_assignment_from_events(
        &[event],
        logical_run_id,
    )
    .unwrap();
    let mut retry_context = [
        (
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            logical_run_id.to_string(),
        ),
        (
            AGENT_RUN_ID_METADATA_KEY.to_string(),
            "physical-attempt-b".to_string(),
        ),
        ("project_id".to_string(), "project-assignment".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    retry_context.extend(recovered);
    crate::agent_preparation_runtime::reset_preparation_run_context(&mut retry_context);
    let restored = crate::prompt_profile_serving::restore_prompt_profile_selection(
        "auto",
        "project-assignment",
        &retry_context,
    )
    .unwrap()
    .expect("durable assignment should be restored");
    assert_eq!(restored, selection);
}

#[test]
fn assignment_context_copy_is_atomic_and_cleanup_is_complete() {
    let keys = [
        "prompt_profile",
        "prompt_genome",
        "prompt_profile_source",
        "prompt_profile_assignment_receipt",
        "prompt_profile_assignment_sha256",
        "prompt_rollout_status",
    ];
    let mut target = keys
        .into_iter()
        .map(|key| (key.to_string(), format!("original-{key}")))
        .collect::<Metadata>();
    let original = target.clone();
    let partial = [("prompt_profile".to_string(), "partial".to_string())]
        .into_iter()
        .collect::<Metadata>();
    assert!(
        crate::prompt_profile_serving::copy_prompt_profile_assignment(&partial, &mut target)
            .is_err()
    );
    assert_eq!(target, original);

    crate::prompt_profile_serving::copy_prompt_profile_assignment(&Metadata::new(), &mut target)
        .unwrap();
    assert!(keys.iter().all(|key| !target.contains_key(*key)));
}
