use super::*;
use agent_core::{
    Event, EventId, EventKind, AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use orchestrator::{prompt_genome_sha256, PromptVerification};

#[test]
fn run_decision_prompt_applies_only_the_learned_route_directive() {
    let marker = "learned-route-directive-must-route";
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
    profile.custom_directive = "workflow execution guidance".to_string();
    profile.route_directive = Some(marker.to_string());
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
        preferred_primary_model: None,
        required_execution: None,
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
