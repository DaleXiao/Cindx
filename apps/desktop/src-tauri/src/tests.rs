use super::*;
use crate::agent_collaboration_runtime::{
    collaboration_candidate_handoff, collaboration_candidate_quorum,
    collaboration_candidate_quorum_grace, collaboration_error_blocks_executor,
};
use crate::agent_conductor_runtime::{conductor_call_limits, conductor_repair_recovery_window};
use crate::agent_conductor_scheduler::{
    schedule_conductor_decision, ConductorDecisionOutcome, ConductorDecisionSchedule,
};
use crate::agent_preparation_runtime::{
    image_generation_objective_for_preparation, preparation_prompt_parts,
    remove_stale_preparation_context, reset_preparation_run_context,
    route_requirements_for_preparation,
};
use crate::agent_resource_snapshot::persist_agent_resource_snapshot;
use crate::agent_strategy_runtime::{
    causal_route_event_metadata, effective_prompt_objective_for_messages,
    preferred_compatible_route_model, should_evaluate_strategy_profile, AgentPlanningSource,
    PlannedAgentRun,
};
use crate::collaboration_execution::collaboration_model_failure;
use crate::collaboration_stage_runtime::{
    collaboration_stage_result, collaboration_stage_terminal_presentation, CollaborationStageError,
};
use crate::conductor_health_runtime::{
    conductor_health_outcome, ConductorHealthLedger, ConductorHealthOutcome,
};
use crate::prompt_learning_runtime::prompt_dataset_identity;
use crate::semantic_memory_runtime::contains_completed_agent_run;
use agent_core::{EventTypeV1, EVENT_TYPE_METADATA_KEY};
use orchestrator::{
    select_causal_route_v2, AdaptiveWorkflow, AdaptiveWorkflowStep, AgentDecisionCalibration,
    AgentEffectAuthority, AgentExecutionMode, AgentRouteRequirements, AgentRunDecisionHarness,
    AgentRunDecisionRequest, AgentToolRequirement, AgentVerificationPolicy, CausalRouteReason,
    CausalRouteSelectionV2, ExecutionPlan, ExecutionPlanAuthority, ModelCapabilitySource,
    PromptDatasetCaseIdentityV1, PromptExecutionContextV1, PromptLiveAssignmentProvenanceV1,
    PromptTransferProvenance, RouteFeatureRequest, RouteFeatureSnapshotV2,
    CAUSAL_ROUTE_MAX_RECEIPT_BYTES, PROMPT_EXECUTION_CONTEXT_SCHEMA_V1,
    PROMPT_LIVE_ASSIGNMENT_PROVENANCE_SCHEMA_V1,
};
use tools::encode_input;

mod agent_collaboration_contract_tests {
    include!("agent_collaboration_contract_tests.rs");
}

fn test_prompt_evaluation_provenance(
    candidate_id: &str,
    opponent_id: &str,
) -> PromptEvaluationProvenance {
    PromptEvaluationProvenance::blind_pairwise_swap(
        vec!["independent-judge".to_string()],
        vec!["candidate-worker".to_string()],
        "d".repeat(64),
        sha256_hex(candidate_id.as_bytes()),
        sha256_hex(opponent_id.as_bytes()),
    )
}

fn test_prompt_live_assignment_provenance(
    profile: &ConductorPromptGenome,
) -> PromptEvaluationProvenance {
    test_prompt_live_assignment_provenance_for_lineage(profile, "f".repeat(64), 1, 1)
}

fn test_prompt_live_assignment_provenance_for_lineage(
    profile: &ConductorPromptGenome,
    scope_sha256: String,
    source_revision: u64,
    deployment_generation: u64,
) -> PromptEvaluationProvenance {
    let profile_sha256 = prompt_genome_sha256(profile).unwrap();
    PromptEvaluationProvenance {
        protocol: PROMPT_LIVE_ASSIGNMENT_PROVENANCE_SCHEMA_V1.to_string(),
        candidate_prompt_sha256: profile_sha256.clone(),
        live_assignment: Some(PromptLiveAssignmentProvenanceV1 {
            schema: PROMPT_LIVE_ASSIGNMENT_PROVENANCE_SCHEMA_V1.to_string(),
            assignment_receipt_sha256: "a".repeat(64),
            assignment_source: "canary".to_string(),
            profile_id: profile.id.clone(),
            profile_sha256,
            scope_sha256,
            source_revision,
            deployment_generation,
            distillation_lease_sha256: None,
        }),
        ..PromptEvaluationProvenance::default()
    }
}

pub(crate) fn bind_matched_prompt_evidence(
    model: &mut PromptEvolutionReadModel,
    effort: &str,
    candidate_id: &str,
    stable_id: &str,
) {
    model.attempts.retain(|_, attempt| {
        let profiles = attempt
            .started
            .treatments
            .iter()
            .map(|treatment| treatment.profile_id.as_str())
            .collect::<BTreeSet<_>>();
        profiles != BTreeSet::from([candidate_id, stable_id])
    });
    let cases = model
        .observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort
                && observation.mode.is_execution()
                && ((observation.profile_id == candidate_id
                    && observation.opponent_profile_id.as_deref() == Some(stable_id))
                    || (observation.profile_id == stable_id
                        && observation.opponent_profile_id.as_deref() == Some(candidate_id)))
        })
        .map(|(_, observation)| {
            (
                (observation.case_id.clone(), observation.split),
                PromptDatasetCaseIdentityV1 {
                    case_id: observation.case_id.clone(),
                    objective_sha256: sha256_hex(observation.case_id.as_bytes()),
                    task_family_sha256: sha256_hex(observation.task_class.as_bytes()),
                    split: observation.split,
                },
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect::<Vec<_>>();
    let dataset = PromptDatasetIdentityV1::new("test-project", 1, cases).unwrap();
    let cohort = PromptLearningCohortV1::new(
        dataset,
        PromptExecutionContextV1 {
            schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            harness_sha256: "3".repeat(64),
            system_prompt_sha256: "4".repeat(64),
            policy: AgentPolicy::parse_ingress(effort),
            policy_sha256: "5".repeat(64),
            budget_sha256: "6".repeat(64),
            tool_contract_sha256: "7".repeat(64),
            source_revision_sha256: "8".repeat(64),
            workspace_revision_sha256: "9".repeat(64),
        },
    )
    .unwrap();
    let mut pairs = BTreeMap::<String, Vec<usize>>::new();
    for (index, (observed_effort, observation)) in model.observations.iter().enumerate() {
        if observed_effort == effort
            && observation.mode.is_execution()
            && ((observation.profile_id == candidate_id
                && observation.opponent_profile_id.as_deref() == Some(stable_id))
                || (observation.profile_id == stable_id
                    && observation.opponent_profile_id.as_deref() == Some(candidate_id)))
        {
            pairs
                .entry(observation.evaluation_id.clone())
                .or_default()
                .push(index);
        }
    }
    for indices in pairs.into_values().filter(|indices| indices.len() == 2) {
        let first = &model.observations[indices[0]].1;
        let identity = PromptMatchedEvaluationIdentityV1::new(
            first.evaluation_id.clone(),
            &cohort,
            first.case_id.clone(),
            first.split,
            first.mode,
        )
        .unwrap();
        for index in &indices {
            let observation = &mut model.observations[*index].1;
            observation.provenance.dataset_sha256 = cohort.dataset.dataset_sha256.clone();
            observation.provenance.matched_evaluation = Some(identity.clone());
        }
        let candidate = indices
            .iter()
            .map(|index| &model.observations[*index].1)
            .find(|observation| observation.profile_id == candidate_id)
            .unwrap();
        let stable = indices
            .iter()
            .map(|index| &model.observations[*index].1)
            .find(|observation| observation.profile_id == stable_id)
            .unwrap();
        let started = PromptEvaluationAttemptEventV1::started(
            identity,
            &cohort,
            [
                PromptTreatmentIdentityV1 {
                    profile_id: candidate_id.to_string(),
                    prompt_sha256: candidate.provenance.candidate_prompt_sha256.clone(),
                },
                PromptTreatmentIdentityV1 {
                    profile_id: stable_id.to_string(),
                    prompt_sha256: stable.provenance.candidate_prompt_sha256.clone(),
                },
            ],
        )
        .unwrap();
        let terminal = PromptEvaluationAttemptEventV1::terminal(
            &started,
            PromptEvaluationAttemptStatus::CompletedPair,
            [false; 2],
            "",
        )
        .unwrap();
        model.attempts.insert(
            started.identity.evaluation_id.clone(),
            PromptEvaluationAttemptState {
                started,
                terminal: Some(terminal),
            },
        );
    }
    model
        .cohort_sequences
        .insert(cohort.cohort_sha256.clone(), 1);
    model.cohorts.insert(cohort.cohort_sha256.clone(), cohort);
}

fn test_conductor_harness(models: Vec<String>, agent_budget: usize) -> ConductorHarness {
    let routing = RoutingContext::from_prompt("Test the conductor", Vec::new());
    let primary_model = models.first().cloned().unwrap_or_default();
    ConductorHarness::new(ConductorRequest {
        workflow_id: "test-workflow".to_string(),
        objective: "Test the conductor".to_string(),
        recent_context: String::new(),
        effort: "pro".to_string(),
        policy: "best_of_n".to_string(),
        conductor_model: "conductor-model".to_string(),
        primary_model,
        worker_models: models,
        role_hints: ConductorRoleHints {
            planner: "planner-a".to_string(),
            executor: "planner-a".to_string(),
            reviewer: "reviewer-b".to_string(),
            synthesizer: "summary-c".to_string(),
        },
        budget: WorkflowBudget {
            max_steps: adaptive_workflow_step_budget(agent_budget),
            max_models: agent_budget,
            max_model_turns_per_step: DEFAULT_COLLABORATION_WORKER_TURNS,
            max_tool_calls_per_step: MAX_COLLABORATION_WORKER_TOOL_CALLS,
            max_output_tokens_per_step: COLLABORATION_MAX_OUTPUT_TOKENS as usize,
        },
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            "pro",
            OrchestrationPolicy::BestOfN {
                candidates: agent_budget,
            },
        ),
        prior_hint: None,
        prompt_evolution_enabled: true,
        prompt_genome: ConductorPromptGenome::seed_for_effort("pro"),
    })
}

fn test_planned_agent_run(decision: AgentRunDecision, effort: AgentPolicy) -> PlannedAgentRun {
    test_planned_agent_run_with_candidate(decision.clone(), decision, effort)
}

fn test_planned_agent_run_with_candidate(
    conductor_candidate: AgentRunDecision,
    mut decision: AgentRunDecision,
    effort: AgentPolicy,
) -> PlannedAgentRun {
    let route_requirements = AgentRouteRequirements {
        minimum_tool_requirement: decision.tool_requirement,
        effect_authority: if decision.tool_requirement == AgentToolRequirement::Effects {
            AgentEffectAuthority::Required
        } else {
            AgentEffectAuthority::Forbidden
        },
        image_input_required: decision.vision_required,
    };
    let candidates = vec![ModelCandidate {
        name: decision.primary_model.clone(),
        role: ModelRole::Executor,
        supports_tools: true,
        supports_vision: true,
        tools_capability_source: ModelCapabilitySource::Configured,
        vision_capability_source: ModelCapabilitySource::Configured,
        cost_tier: 1,
        latency_tier: 1,
    }];
    let snapshot = RouteFeatureSnapshotV2::from_decision_request(
        &conductor_candidate,
        RouteFeatureRequest {
            objective: "update the workspace",
            recent_context: "",
            effort: effort.label(),
            requirements: route_requirements,
            budget_fingerprint: None,
            prompt_profile_sha256: &"1".repeat(64),
        },
        &candidates,
    );
    let mut receipt = match decision.causal_route.take() {
        Some(receipt) => receipt,
        None => select_causal_route_v2(&conductor_candidate, &snapshot, &candidates, None, 0)
            .expect("test plan should produce a causal route receipt"),
    };
    if receipt.selected_route != decision.route_tier() {
        receipt
            .reconcile_selected_route(
                decision.route_tier(),
                CausalRouteReason::ExecutionConstraint,
            )
            .expect("test plan receipt should reconcile to its fixture route");
    }
    decision.causal_route = None;
    let routing_context = decision.routing_context("update the workspace", candidates);
    let routing_decision = decision.routing_decision();
    let execution_contract = decision.execution_contract(effort.label());
    let prompt_genome = ConductorPromptGenome::seed_for_effort(effort.label());
    let workflow_execution_profile_sha256 = (decision.execution == AgentExecutionMode::Workflow)
        .then(|| prompt_genome.workflow_execution_profile_sha256().unwrap());
    let execution_plan = ExecutionPlan::new(
        conductor_candidate,
        decision,
        receipt,
        workflow_execution_profile_sha256,
    )
    .expect("test plan should satisfy the execution-plan contract");
    PlannedAgentRun {
        policy: effort,
        execution_plan,
        routing_context,
        routing_decision,
        execution_contract,
        source: if effort == AgentPolicy::Fast {
            AgentPlanningSource::FastDirect
        } else {
            AgentPlanningSource::DynamicConductor
        },
        attempts: 1,
        prompt_genome,
        degradation_reason: None,
        attempted_conductor_models: Vec::new(),
        selected_conductor_model: None,
        route_requirements,
    }
}

#[test]
fn execution_plan_context_preserves_conductor_candidate_and_final_authority() {
    let mut candidate = AgentRunDecision::direct("executor");
    candidate.execution = AgentExecutionMode::Workflow;
    candidate.verification = AgentVerificationPolicy::Independent;
    candidate.max_parallelism = 2;
    candidate.min_successful_branches = 2;
    candidate.distinct_contributions = 2;
    candidate.estimated_steps = 3;
    candidate.expected_uplift_bps = 2_999;
    candidate.confidence_bps = 8_000;
    candidate.stop_policy = ConductorStopPolicy::Quorum;
    let selected = candidate.clone().constrained_to_grounded_direct();
    let planned = test_planned_agent_run_with_candidate(candidate, selected, AgentPolicy::Auto);
    let mut context = Metadata::new();

    planned
        .apply_to_context(&mut context)
        .expect("execution plan should enter run context");

    let persisted = serde_json::from_str::<ExecutionPlan>(
        context.get("execution_plan").expect("execution plan json"),
    )
    .expect("valid execution plan");
    assert_eq!(
        persisted.conductor_candidate.execution,
        AgentExecutionMode::Workflow
    );
    assert_eq!(persisted.action.execution, AgentExecutionMode::Direct);
    assert_eq!(
        persisted.authority,
        ExecutionPlanAuthority::CompatibilityValuePolicy
    );
    let full_digest = persisted.digest().unwrap();
    let semantic_digest = persisted.semantic_digest().unwrap();
    assert_eq!(context.get("execution_plan_sha256"), Some(&full_digest));
    assert_eq!(
        context.get("execution_plan_semantic_sha256"),
        Some(&semantic_digest)
    );
}

#[test]
fn conductor_verification_policies_reach_the_persistent_task_contract() {
    let mut none = AgentRunDecision::direct("executor");
    none.verification = AgentVerificationPolicy::None;
    let mut independent = AgentRunDecision::direct("executor");
    independent.execution = AgentExecutionMode::Workflow;
    independent.verification = AgentVerificationPolicy::Independent;
    independent.max_parallelism = 2;
    independent.min_successful_branches = 2;
    independent.distinct_contributions = 2;
    independent.estimated_steps = 3;
    independent.expected_uplift_bps = 2_500;
    independent.confidence_bps = 7_000;
    independent.stop_policy = ConductorStopPolicy::Quorum;
    independent
        .validate(&["executor".to_string(), "reviewer".to_string()], 2)
        .expect("independent fallback should remain valid");

    for (decision, effort, expected) in [
        (
            none,
            AgentPolicy::Fast,
            WorkspaceVerificationPolicy::NotRequired,
        ),
        (
            AgentRunDecision::direct("executor"),
            AgentPolicy::Auto,
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        ),
        (
            independent,
            AgentPolicy::Pro,
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        ),
    ] {
        let planned = test_planned_agent_run(decision, effort);
        let mut run_context = Metadata::new();
        planned
            .apply_to_context(&mut run_context)
            .expect("plan should populate run context");
        let encoded = run_context
            .get("conductor_contract")
            .expect("typed contract should be persisted");
        let decoded = serde_json::from_str::<ConductorExecutionContract>(encoded)
            .expect("persisted contract should decode");
        assert_eq!(decoded.verification_required, expected.is_required());

        let mut runtime = start_agent_loop(
            TaskId("verification-policy-test".to_string()),
            "update the workspace",
            AgentRuntimeConfig::default(),
        );
        apply_run_task_contract(&mut runtime, &run_context, &[], None)
            .expect("runtime contract should accept the policy");
        assert_eq!(
            runtime.task_contract.workspace_verification_policy(),
            expected
        );
        assert_eq!(
            planned.routing_decision.verifier_role == Some(ModelRole::Reviewer),
            planned.execution_plan.action().verification == AgentVerificationPolicy::Independent
        );
    }
}

#[test]
fn collaboration_candidate_quorum_matches_effort_contract() {
    assert_eq!(collaboration_candidate_quorum(0, "fast"), 0);
    assert_eq!(collaboration_candidate_quorum(1, "auto"), 1);
    assert_eq!(collaboration_candidate_quorum(3, "fast"), 1);
    assert_eq!(collaboration_candidate_quorum(2, "auto"), 2);
    assert_eq!(collaboration_candidate_quorum(4, "auto"), 3);
    assert_eq!(collaboration_candidate_quorum(3, "pro"), 2);
    assert_eq!(collaboration_candidate_quorum(4, "pro"), 3);

    assert_eq!(
        collaboration_candidate_quorum_grace("fast"),
        Duration::from_millis(100)
    );
    assert_eq!(
        collaboration_candidate_quorum_grace("auto"),
        Duration::from_millis(400)
    );
    assert_eq!(
        collaboration_candidate_quorum_grace("pro"),
        Duration::from_millis(1_500)
    );
}

#[test]
fn memory_recall_policy_controls_the_context_budget() {
    assert_eq!(memory_recall_limit(MemoryRecallPolicy::None), 0);
    assert!(
        memory_recall_limit(MemoryRecallPolicy::Relevant)
            < memory_recall_limit(MemoryRecallPolicy::Comprehensive)
    );
    assert_eq!(
        memory_recall_limit(MemoryRecallPolicy::Comprehensive),
        crate::runtime_constants::AGENT_MEMORY_RECALL_LIMIT
    );
}

#[test]
fn arbiter_failure_handoff_preserves_candidate_work_for_executor() {
    let candidates = vec![
        ("model-a".to_string(), "first grounded proposal".to_string()),
        (
            "model-b".to_string(),
            "second independent proposal".to_string(),
        ),
    ];
    let handoff = collaboration_candidate_handoff("solve the task", &candidates, "timeout");
    assert!(handoff.contains("INTERNAL TEAM HANDOFF"));
    assert!(handoff.contains("first grounded proposal"));
    assert!(handoff.contains("second independent proposal"));
    assert!(handoff.contains("timeout"));
}

#[test]
fn safety_and_steer_collaboration_errors_block_stale_executor_context() {
    assert!(collaboration_error_blocks_executor(&format!(
        "{WORKFLOW_SAFETY_ERROR_PREFIX} unsafe output"
    )));
    assert!(collaboration_error_blocks_executor(
        COLLABORATION_STEER_INTERRUPTED
    ));
    assert!(!collaboration_error_blocks_executor(&format!(
        "{WORKFLOW_RESUMABLE_ERROR_PREFIX} provider timeout"
    )));
    assert!(!collaboration_error_blocks_executor("arbiter unavailable"));
}

#[test]
fn pending_steer_interrupts_collaboration_without_stopping_the_run() {
    let control = Arc::new(AgentRunControl::new("pro"));
    assert!(!collaboration_run_should_interrupt(&control));

    assert_eq!(control.request_steer("queue-steer"), Ok(true));
    assert!(control.has_pending_steer());
    assert!(collaboration_run_should_interrupt(&control));
    assert!(!agent_run_should_stop(&control));

    let pending = control.take_pending_steers();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].queue_id, "queue-steer");
    assert!(!collaboration_run_should_interrupt(&control));
}

#[test]
fn goal2_collaboration_terminal_status_distinguishes_interruptions_and_stage_deadlines() {
    let completed = CollaborationCompletion::completed_worker(
        "usable decision".to_string(),
        12,
        Metadata::new(),
        Vec::new(),
    );
    let interrupted =
        CollaborationCompletion::failed_with(AgentFailure::cancelled("user_steer", "superseded"));
    let stage_deadline = CollaborationCompletion::failed_with(AgentFailure::budget(
        RunStopReason::StageBudgetExhausted.code(),
        "deadline exhausted",
    ));
    let unavailable = CollaborationCompletion::failed("provider unavailable");

    assert_eq!(
        collaboration_stage_terminal_presentation(&completed),
        ("completed", "finished")
    );
    assert_eq!(
        collaboration_stage_terminal_presentation(&interrupted),
        ("interrupted", "interrupted")
    );
    assert_eq!(
        collaboration_stage_terminal_presentation(&stage_deadline),
        ("degraded", "deadline exhausted")
    );
    assert_eq!(
        collaboration_stage_terminal_presentation(&unavailable),
        ("degraded", "unavailable")
    );
    let deadline_result =
        collaboration_stage_result(CollaborationCompletion::failed_with(AgentFailure::budget(
            RunStopReason::StageBudgetExhausted.code(),
            "deadline exhausted",
        )));
    assert_eq!(deadline_result, Err(CollaborationStageError::StageDeadline));
}

#[test]
fn goal2_collaboration_cancellation_cause_prefers_run_stop_then_steer() {
    let cancelled = ModelError::new(MODEL_REQUEST_CANCELLED);
    let control = Arc::new(AgentRunControl::new("pro"));
    assert_eq!(control.request_steer("steer-first"), Ok(true));
    let steer_failure =
        collaboration_model_failure(&cancelled, Some(&control), RunStageClass::Conductor);
    assert_eq!(steer_failure.code, "user_steer");
    assert_eq!(steer_failure.class, AgentFailureClass::Cancelled);
    let mut steer_completion = CollaborationCompletion::failed_with(steer_failure);
    steer_completion.usage.insert(
        COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
        COLLABORATION_TERMINATION_STEER.to_string(),
    );
    let steer_result = collaboration_stage_result(steer_completion);
    assert_eq!(steer_result, Err(CollaborationStageError::SteerInterrupted));

    control.request_cancel();
    let cancelled_failure =
        collaboration_model_failure(&cancelled, Some(&control), RunStageClass::Conductor);
    assert_eq!(cancelled_failure.code, RunStopReason::UserCancelled.code());
    assert_eq!(cancelled_failure.class, AgentFailureClass::Cancelled);
    let mut stop_completion = CollaborationCompletion::failed_with(cancelled_failure);
    stop_completion.usage.insert(
        COLLABORATION_TERMINATION_SCOPE_KEY.to_string(),
        COLLABORATION_TERMINATION_RUN.to_string(),
    );
    let stop_result = collaboration_stage_result(stop_completion);
    assert_eq!(stop_result, Err(CollaborationStageError::RunStopped));
}

#[test]
fn goal2_conductor_scheduler_fails_over_but_never_retries_after_steer_or_stage_deadline() {
    let models = vec![
        "primary".to_string(),
        "alternate".to_string(),
        "last".to_string(),
    ];
    let mut attempts = 0;
    let mut observed = Vec::new();
    let selected = schedule_conductor_decision(
        &models,
        &mut attempts,
        |index, model, has_alternate, attempts| {
            *attempts += 1;
            observed.push((model.to_string(), has_alternate));
            if index == 0 {
                Err(CollaborationStageError::ModelFailure(AgentFailure::new(
                    "provider_timeout",
                    "provider timeout",
                    AgentFailureClass::ProviderTransient,
                    true,
                )))
            } else {
                Ok(AgentRunDecision::direct(model))
            }
        },
    )
    .expect("a provider failure should fail over");
    assert!(matches!(
        selected.outcome,
        ConductorDecisionOutcome::Selected(_)
    ));
    assert_eq!(selected.selected_model.as_deref(), Some("alternate"));
    assert_eq!(selected.attempted_models, vec!["primary", "alternate"]);
    assert_eq!(selected.failure_reasons.len(), 1);
    assert_eq!(attempts, 2);
    assert_eq!(
        observed,
        vec![
            ("primary".to_string(), true),
            ("alternate".to_string(), true)
        ]
    );

    for terminal_error in [
        CollaborationStageError::ModelFailure(AgentFailure::from_model_error(
            &ModelError::with_status(401, "invalid credentials"),
        )),
        CollaborationStageError::ModelFailure(AgentFailure::internal(
            "collaboration_internal",
            "local persistence failed",
        )),
        CollaborationStageError::Failed("local persistence failed".to_string()),
    ] {
        let mut attempts = 0;
        let mut invoked = Vec::new();
        let exhausted: ConductorDecisionSchedule<AgentRunDecision> =
            schedule_conductor_decision(&models, &mut attempts, |_, model, _, attempts| {
                *attempts += 1;
                invoked.push(model.to_string());
                Err(terminal_error.clone())
            })
            .expect("a deterministic failure should degrade without retrying another model");
        assert!(matches!(
            exhausted.outcome,
            ConductorDecisionOutcome::Exhausted
        ));
        assert_eq!(invoked, vec!["primary"]);
        assert_eq!(attempts, 1);
    }

    let mut attempts = 0;
    let authorized =
        schedule_conductor_decision(&models, &mut attempts, |index, model, _, attempts| {
            *attempts += 1;
            if index == 0 {
                Err(CollaborationStageError::ModelFailure(
                    AgentFailure::from_model_error(&ModelError::with_status(
                        403,
                        "primary model is not allowed",
                    )),
                ))
            } else {
                Ok(AgentRunDecision::direct(model))
            }
        })
        .expect("model-scoped authorization should try an alternate model");
    assert!(matches!(
        authorized.outcome,
        ConductorDecisionOutcome::Selected(_)
    ));
    assert_eq!(authorized.selected_model.as_deref(), Some("alternate"));
    assert_eq!(attempts, 2);

    let mut attempts = 0;
    let mut invoked = Vec::new();
    let steered: Result<ConductorDecisionSchedule<AgentRunDecision>, CollaborationStageError> =
        schedule_conductor_decision(&models, &mut attempts, |_, model, _, attempts| {
            *attempts += 1;
            invoked.push(model.to_string());
            Err(CollaborationStageError::SteerInterrupted)
        });
    assert!(matches!(
        steered,
        Err(CollaborationStageError::SteerInterrupted)
    ));
    assert_eq!(invoked, vec!["primary"]);
    assert_eq!(attempts, 1);

    let mut attempts = 0;
    let mut invoked = Vec::new();
    let exhausted: ConductorDecisionSchedule<AgentRunDecision> =
        schedule_conductor_decision(&models, &mut attempts, |_, model, _, attempts| {
            *attempts += 1;
            invoked.push(model.to_string());
            Err(CollaborationStageError::StageDeadline)
        })
        .expect("a global stage deadline should degrade without another attempt");
    assert!(matches!(
        exhausted.outcome,
        ConductorDecisionOutcome::Exhausted
    ));
    assert_eq!(invoked, vec!["primary"]);
    assert_eq!(attempts, 1);

    let mut attempts = 0;
    let mut invoked = Vec::new();
    let recovered =
        schedule_conductor_decision(&models, &mut attempts, |index, model, _, attempts| {
            *attempts += 1;
            invoked.push(model.to_string());
            if index == 0 {
                Err(CollaborationStageError::AttemptDeadline)
            } else {
                Ok(AgentRunDecision::direct(model))
            }
        })
        .expect("a no-progress attempt deadline should fail over");
    assert!(matches!(
        recovered.outcome,
        ConductorDecisionOutcome::Selected(_)
    ));
    assert_eq!(recovered.selected_model.as_deref(), Some("alternate"));
    assert_eq!(invoked, vec!["primary", "alternate"]);

    let mut attempts = 0;
    let mut invoked = Vec::new();
    let stopped: Result<ConductorDecisionSchedule<AgentRunDecision>, CollaborationStageError> =
        schedule_conductor_decision(&models, &mut attempts, |_, model, _, attempts| {
            *attempts += 1;
            invoked.push(model.to_string());
            Err(CollaborationStageError::RunStopped)
        });
    assert!(matches!(stopped, Err(CollaborationStageError::RunStopped)));
    assert_eq!(invoked, vec!["primary"]);
    assert_eq!(attempts, 1);
}

#[test]
fn goal2_conductor_failover_limits_preserve_quality_after_response_start() {
    assert!(!collaboration_no_progress_should_cancel(
        false,
        Duration::from_secs(44),
        Some(Duration::from_secs(45))
    ));
    assert!(collaboration_no_progress_should_cancel(
        false,
        Duration::from_secs(45),
        Some(Duration::from_secs(45))
    ));
    assert!(!collaboration_no_progress_should_cancel(
        true,
        Duration::from_secs(900),
        Some(Duration::from_secs(45))
    ));
    assert!(!collaboration_no_progress_should_cancel(
        false,
        Duration::from_secs(900),
        None
    ));
    assert_eq!(
        conductor_repair_recovery_window(true),
        Some(Duration::from_secs(60))
    );
    assert_eq!(conductor_repair_recovery_window(false), None);
    let primary = conductor_call_limits(true, false);
    assert_eq!(primary.recovery_window, None);
    assert_eq!(primary.no_progress_timeout, Some(Duration::from_secs(45)));
    let repair = conductor_call_limits(true, true);
    assert_eq!(repair.recovery_window, Some(Duration::from_secs(60)));
    assert_eq!(repair.no_progress_timeout, Some(Duration::from_secs(45)));
    let single_primary = conductor_call_limits(false, false);
    assert_eq!(single_primary.recovery_window, None);
    assert_eq!(
        single_primary.no_progress_timeout,
        Some(Duration::from_secs(45))
    );
    let single_repair = conductor_call_limits(false, true);
    assert_eq!(single_repair.recovery_window, None);
    assert_eq!(
        single_repair.no_progress_timeout,
        Some(Duration::from_secs(45))
    );
}

#[test]
fn goal3_conductor_health_classifies_only_attributable_failures() {
    let valid = Ok(AgentRunDecision::direct("primary"));
    assert_eq!(
        conductor_health_outcome(&valid),
        ConductorHealthOutcome::ValidDecision
    );
    assert_eq!(
        conductor_health_outcome(&Err(CollaborationStageError::AttemptDeadline)),
        ConductorHealthOutcome::RetryableFailure
    );
    assert_eq!(
        conductor_health_outcome(&Err(CollaborationStageError::ModelFailure(
            AgentFailure::new(
                "provider_timeout",
                "provider timed out",
                AgentFailureClass::ProviderTransient,
                true,
            ),
        ))),
        ConductorHealthOutcome::RetryableFailure
    );
    assert_eq!(
        conductor_health_outcome(&Err(CollaborationStageError::ModelFailure(
            AgentFailure::new(
                "provider_transient",
                "non-retryable transport failure",
                AgentFailureClass::ProviderTransient,
                false,
            ),
        ))),
        ConductorHealthOutcome::Censored
    );
    assert_eq!(
        conductor_health_outcome(&Err(CollaborationStageError::ModelFailure(
            AgentFailure::model_output("invalid_output", "invalid output"),
        ))),
        ConductorHealthOutcome::RejectedDecision
    );
    assert_eq!(
        conductor_health_outcome(&Err(CollaborationStageError::DecisionRejected(
            "invalid decision".to_string(),
        ))),
        ConductorHealthOutcome::RejectedDecision
    );

    for censored in [
        CollaborationStageError::RunStopped,
        CollaborationStageError::SteerInterrupted,
        CollaborationStageError::StageDeadline,
        CollaborationStageError::ModelFailure(AgentFailure::new(
            "provider_authentication",
            "invalid credentials",
            AgentFailureClass::ProviderPermanent,
            false,
        )),
        CollaborationStageError::ModelFailure(AgentFailure::internal(
            "persistence_failed",
            "could not persist event",
        )),
        CollaborationStageError::Failed("local configuration error".to_string()),
    ] {
        assert_eq!(
            conductor_health_outcome(&Err(censored)),
            ConductorHealthOutcome::Censored
        );
    }
}

#[test]
fn goal3_conductor_health_never_reorders_without_complete_quality_evidence() {
    let configured = vec![
        "primary".to_string(),
        "alternate".to_string(),
        "reserve".to_string(),
        "terminal".to_string(),
    ];
    let mut cold = ConductorHealthLedger::default();
    assert_eq!(cold.route("scope", &configured).models, configured);

    let mut learned = ConductorHealthLedger::default();
    for _ in 0..5 {
        learned.record(
            "scope",
            "alternate",
            ConductorHealthOutcome::RetryableFailure,
        );
        learned.record("scope", "reserve", ConductorHealthOutcome::ValidDecision);
    }
    let routed = learned.route("scope", &configured);
    assert_eq!(routed.models, configured);
    assert_eq!(routed.source, "quality_evidence_required");
    assert_eq!(routed.candidate_models, vec!["reserve"]);

    let run = |models: &[String]| {
        let mut attempts = 0;
        let scheduled =
            schedule_conductor_decision(models, &mut attempts, |_, model, _, attempts| {
                *attempts += 1;
                if model != "reserve" {
                    Err(CollaborationStageError::AttemptDeadline)
                } else {
                    Ok(AgentRunDecision::direct(model))
                }
            })
            .expect("the alternate should produce a valid decision");
        (attempts, scheduled.selected_model)
    };

    assert_eq!(run(&configured), (3, Some("reserve".to_string())));
    assert_eq!(run(&routed.models), (3, Some("reserve".to_string())));
}

#[test]
fn goal2_preparation_replay_keeps_each_steer_once_and_replans_the_latest_prompt() {
    let mut runtime = start_agent_loop(
        TaskId("preparation-replay".to_string()),
        "original request",
        AgentRuntimeConfig::default(),
    );
    for (queue_id, prompt) in [
        ("steer-one", "preserve compatibility"),
        ("steer-two", "also run the focused tests"),
    ] {
        AgentKernel::new(&mut runtime, &[]).apply_steer(
            prompt,
            [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_mode".to_string(), "steer".to_string()),
                ("model_content".to_string(), prompt.to_string()),
            ]
            .into_iter()
            .collect(),
        );
    }

    let (history, active) = preparation_prompt_parts(&runtime.messages)
        .expect("the latest steer should be the active planning objective");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "original request");
    assert_eq!(history[1].content, "preserve compatibility");
    assert_eq!(active.content, "also run the focused tests");
    assert_eq!(
        active.metadata.get("queue_id").map(String::as_str),
        Some("steer-two")
    );
    let queue_ids = history
        .iter()
        .chain(std::iter::once(&active))
        .filter_map(|message| message.metadata.get("queue_id"))
        .collect::<Vec<_>>();
    assert_eq!(queue_ids, vec!["steer-one", "steer-two"]);
    assert_eq!(
        effective_prompt_objective_for_messages("original request", &runtime.messages),
        "Initial request:\noriginal request\n\nAccepted steering 1:\npreserve compatibility\n\nAccepted steering 2:\nalso run the focused tests"
    );
}

#[test]
fn goal2_effective_objective_reclaims_short_steer_budget_for_initial_constraints() {
    let initial = format!("{}CRITICAL_END_CONSTRAINT", "A".repeat(5_900));
    let mut runtime = start_agent_loop(
        TaskId("effective-objective-budget".to_string()),
        &initial,
        AgentRuntimeConfig::default(),
    );
    for (index, prompt) in ["continue", "preserve data", "keep it fast", "run tests"]
        .into_iter()
        .enumerate()
    {
        AgentKernel::new(&mut runtime, &[]).apply_steer(
            prompt,
            [
                ("queue_id".to_string(), format!("steer-{index}")),
                ("queue_mode".to_string(), "steer".to_string()),
                ("model_content".to_string(), prompt.to_string()),
            ]
            .into_iter()
            .collect(),
        );
    }

    let objective = effective_prompt_objective_for_messages(&initial, &runtime.messages);

    assert!(objective.chars().count() <= 6_000);
    assert!(objective.chars().count() > 5_800);
    assert!(objective.contains("CRITICAL_END_CONSTRAINT"));
    for prompt in ["continue", "preserve data", "keep it fast", "run tests"] {
        assert!(objective.contains(prompt));
    }
}

#[test]
fn goal2_execution_steer_replans_and_feeds_terminal_epoch_learning() {
    let initial =
        "Inspect the Rust implementation; preserve the public API and do not modify files.";
    let steer = "Research the upstream algorithm too.";
    let models = vec!["coding-model".to_string(), "research-model".to_string()];
    let mut conductor_objectives = Vec::new();
    let mut select_plan = |objective: &str| {
        conductor_objectives.push(objective.to_string());
        let revised = objective.contains("Accepted steering 1:");
        let model = if revised {
            "research-model"
        } else {
            "coding-model"
        };
        let mut expected = AgentRunDecision::direct(model);
        expected.task_class = if revised {
            TaskClass::Research
        } else {
            TaskClass::Coding
        };
        let harness = AgentRunDecisionHarness::new(AgentRunDecisionRequest {
            objective: objective.to_string(),
            recent_context: String::new(),
            effort: "auto".to_string(),
            conductor_model: "deterministic-test-conductor".to_string(),
            allowed_models: models.clone(),
            model_candidates: models
                .iter()
                .map(|model| ModelCandidate {
                    name: model.clone(),
                    role: ModelRole::Executor,
                    supports_tools: true,
                    supports_vision: true,
                    tools_capability_source: ModelCapabilitySource::Configured,
                    vision_capability_source: ModelCapabilitySource::Configured,
                    cost_tier: 1,
                    latency_tier: 1,
                })
                .collect(),
            max_parallelism: 2,
            evolved_directive: String::new(),
            historical_evidence: String::new(),
            matched_collaboration_evidence: Arc::new(Default::default()),
            execution_constraints: "isolated workers are read-only".to_string(),
            route_requirements: AgentRouteRequirements::default(),
            budget_fingerprint: None,
            prompt_profile_sha256: "1".repeat(64),
        });
        assert!(harness.planning_prompt().contains(objective));
        let response = serde_json::to_string(&expected).expect("decision should serialize");
        let decision = harness
            .parse(&response)
            .expect("deterministic conductor decision should validate");
        test_planned_agent_run(decision, AgentPolicy::Auto)
    };

    let base_context = [
        (
            "agent_run_id".to_string(),
            "execution-steer-run".to_string(),
        ),
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("initial_prompt_objective".to_string(), initial.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut runtime = start_agent_loop(phase16_task_id(), initial, AgentRuntimeConfig::default());
    let control = AgentRunControl::new("auto");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), initial.to_string())]
                .into_iter()
                .collect(),
            &base_context,
        ),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::MessageAdded,
        "Initial request",
        metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), initial.to_string()),
                ("display_content".to_string(), initial.to_string()),
                ("steer_epoch".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
            &base_context,
        ),
    )
    .expect("initial request should append");

    assert!(control.begin_preparation());
    let initial_objective = effective_prompt_objective_for_messages(initial, &runtime.messages);
    let initial_plan = select_plan(&initial_objective);
    let mut initial_context = base_context.clone();
    initial_context.insert("steer_epoch".to_string(), "0".to_string());
    initial_context.insert("prompt_objective".to_string(), initial_objective.clone());
    initial_context.insert(
        "effective_prompt_objective".to_string(),
        initial_objective.clone(),
    );
    initial_plan
        .apply_to_context(&mut initial_context)
        .expect("initial plan should populate context");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        initial_context.clone(),
    )
    .expect("initial decision should append");
    assert!(control.commit_preparation(0));
    let initial_lease = match control.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("initial execution lease should be acquired: {outcome:?}"),
    };
    assert_eq!(
        initial_context.get("task_class").map(String::as_str),
        Some("coding")
    );
    assert_eq!(
        initial_context.get("agent_model").map(String::as_str),
        Some("coding-model")
    );

    assert_eq!(control.request_steer("short-steer"), Ok(true));
    assert!(!control.execution_epoch_lease_is_current(initial_lease));
    let applied_epoch = match control
        .commit_pending_steers_with(|pending| {
            let accepted = pending.last().expect("the short steer should be pending");
            AgentKernel::new(&mut runtime, &[]).apply_steer(
                steer,
                [
                    ("queue_id".to_string(), accepted.queue_id.clone()),
                    ("queue_mode".to_string(), "steer".to_string()),
                    ("display_content".to_string(), steer.to_string()),
                    ("model_content".to_string(), steer.to_string()),
                ]
                .into_iter()
                .collect(),
            );
            Ok::<_, ()>(accepted.epoch)
        })
        .expect("steer application should succeed")
    {
        agent_runtime::RunSteerBatchCommit::Committed { value, .. } => value,
        outcome => panic!("steer should commit: {outcome:?}"),
    };
    assert_eq!(applied_epoch, 1);
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::MessageAdded,
        "Accepted user steering",
        metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), steer.to_string()),
                ("display_content".to_string(), steer.to_string()),
                ("queue_mode".to_string(), "steer".to_string()),
                ("queue_id".to_string(), "short-steer".to_string()),
                ("steer_epoch".to_string(), applied_epoch.to_string()),
            ]
            .into_iter()
            .collect(),
            &base_context,
        ),
    )
    .expect("accepted steer should append");

    assert!(control.begin_preparation());
    let (history, active) = preparation_prompt_parts(&runtime.messages)
        .expect("the steer should become the active preparation prompt");
    assert_eq!(history[0].content, initial);
    assert_eq!(active.content, steer);
    let revised_objective = effective_prompt_objective_for_messages(initial, &runtime.messages);
    let revised_plan = select_plan(&revised_objective);
    let mut revised_context = base_context.clone();
    revised_context.insert("steer_epoch".to_string(), applied_epoch.to_string());
    revised_context.insert("prompt_objective".to_string(), revised_objective.clone());
    revised_context.insert(
        "effective_prompt_objective".to_string(),
        revised_objective.clone(),
    );
    revised_plan
        .apply_to_context(&mut revised_context)
        .expect("revised plan should populate context");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        revised_context.clone(),
    )
    .expect("revised decision should append");
    assert!(control.commit_preparation(applied_epoch));
    assert_eq!(conductor_objectives.len(), 2);
    assert_eq!(conductor_objectives[0], initial);
    assert_eq!(conductor_objectives[1], revised_objective);
    assert!(revised_objective.contains(initial));
    assert!(revised_objective.contains("preserve the public API"));
    assert!(revised_objective.contains(steer));
    assert_eq!(
        revised_context.get("task_class").map(String::as_str),
        Some("research")
    );
    assert_eq!(
        revised_context
            .get("pre_decision_task_class")
            .map(String::as_str),
        Some("coding")
    );
    assert_eq!(
        revised_context.get("agent_model").map(String::as_str),
        Some("research-model")
    );

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Replanned model finished",
        metadata_with_context(
            [("total_tokens".to_string(), "11".to_string())]
                .into_iter()
                .collect(),
            &revised_context,
        ),
    )
    .expect("terminal epoch cost should append");
    let revised_lease = match control.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("revised execution lease should be acquired: {outcome:?}"),
    };
    control
        .commit_terminal_result_with(revised_lease, || {
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                "Agent task completed",
                revised_context.clone(),
            )
            .map_err(|error| error.to_string())
        })
        .expect("terminal persistence should succeed");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let prompt_cases = prompt_offline_dataset(&events, "project-a", None);
    assert_eq!(prompt_cases.len(), 1);
    assert_eq!(prompt_cases[0].objective, revised_objective);
    assert_eq!(prompt_cases[0].task_class, "coding");
    let routing = routing_telemetry_from_events(&events);
    assert_eq!(routing.len(), 1);
    assert_eq!(routing[0].task_class, TaskClass::Research);
    assert_eq!(routing[0].selected_model, "research-model");
    assert_eq!(routing[0].cost_proxy, 11);
}

#[test]
fn goal2_replanning_replaces_stale_preparation_context_but_keeps_durable_history() {
    let mut history = vec![
        Message {
            role: MessageRole::System,
            content: "old memory".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "project_memory".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::System,
            content: "old collaboration".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("collaboration_stage".to_string(), "guidance".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::System,
            content: "durable artifact manifest".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "artifact_manifest".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::Assistant,
            content: "completed work evidence".to_string(),
            metadata: Metadata::new(),
        },
    ];

    remove_stale_preparation_context(&mut history);

    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "durable artifact manifest");
    assert_eq!(history[1].content, "completed work evidence");
}

#[test]
fn goal2_prompt_derived_image_contract_is_refreshed_in_both_directions() {
    let config = ProviderConfig {
        base_url: "https://provider.example/v1".to_string(),
        image_model: "image-model".to_string(),
        ..ProviderConfig::default()
    };
    let mut run_context = Metadata::new();

    add_image_generation_run_context(
        &mut run_context,
        &config,
        "Generate an image of a lighthouse",
    );
    assert_eq!(
        run_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        run_context
            .get("configured_image_model")
            .map(String::as_str),
        Some("image-model")
    );

    add_image_generation_run_context(&mut run_context, &config, "Review this Rust module");
    assert!(!run_context.contains_key("image_generation_required"));
    assert!(!run_context.contains_key("configured_image_model"));
    assert!(!run_context.contains_key("configured_image_endpoint"));

    add_image_generation_run_context(
        &mut run_context,
        &config,
        "Create a picture for the release notes",
    );
    assert_eq!(
        run_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn goal2_short_image_steer_keeps_the_cumulative_image_contract() {
    let config = ProviderConfig {
        base_url: "https://provider.example/v1".to_string(),
        image_model: "image-model".to_string(),
        ..ProviderConfig::default()
    };
    let initial = "Generate an image of a lighthouse";
    let mut runtime = start_agent_loop(
        TaskId("cumulative-image-contract".to_string()),
        initial,
        AgentRuntimeConfig::default(),
    );
    AgentKernel::new(&mut runtime, &[]).apply_steer(
        "Make the background blue",
        [
            ("queue_id".to_string(), "image-steer".to_string()),
            ("queue_mode".to_string(), "steer".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    let planning_objective = effective_prompt_objective_for_messages(initial, &runtime.messages);
    let mut run_context = Metadata::new();

    add_image_generation_run_context(&mut run_context, &config, &planning_objective);
    run_context.insert(
        "effective_prompt_objective".to_string(),
        planning_objective.clone(),
    );

    assert!(planning_objective.contains(initial));
    assert!(planning_objective.contains("Make the background blue"));
    assert_eq!(
        effective_agent_objective(&run_context, "Make the background blue"),
        planning_objective
    );
    assert_eq!(
        run_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn goal2_prompt_derived_image_contract_is_scoped_to_the_steer_epoch() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "Generate the first image",
        AgentRuntimeConfig::default(),
    );
    let mut run_context = [
        ("steer_epoch".to_string(), "0".to_string()),
        ("image_generation_required".to_string(), "true".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    apply_run_task_contract(&mut runtime, &run_context, &[], None)
        .expect("image contract should apply");
    record_tool_outcome_with_risk(
        &mut runtime,
        "image.generate",
        r#"{"prompt":"first"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::UsesNetwork),
    );
    assert!(runtime
        .task_contract
        .required_tool_satisfied("image.generate"));

    run_context.insert("steer_epoch".to_string(), "1".to_string());
    run_context.remove("image_generation_required");
    apply_run_task_contract(&mut runtime, &run_context, &[], None)
        .expect("text contract should apply");
    assert!(runtime.task_contract.model_context_for_task(&[]).is_none());

    run_context.insert("steer_epoch".to_string(), "2".to_string());
    run_context.insert("image_generation_required".to_string(), "true".to_string());
    apply_run_task_contract(&mut runtime, &run_context, &[], None)
        .expect("new image contract should apply");
    assert!(!runtime
        .task_contract
        .required_tool_satisfied("image.generate"));
}

#[test]
fn goal2_knowledge_preparation_is_superseded_without_stopping_the_run() {
    let control = Arc::new(AgentRunControl::new("pro"));
    let epoch = control.steer_epoch();
    assert!(!knowledge_preparation_should_interrupt(&control, epoch));

    assert_eq!(control.request_steer("replace-objective"), Ok(true));
    assert!(knowledge_preparation_should_interrupt(&control, epoch));
    assert_eq!(control.stop_reason(), None);
}

fn test_message(role: MessageRole, content: impl Into<String>) -> Message {
    Message {
        role,
        content: content.into(),
        metadata: Metadata::new(),
    }
}

fn single_step_workflow_plan(max_model_turns: usize) -> WorkflowPlanIr {
    WorkflowPlanIr::from_adaptive_with_profile(
        "worker-budget-test",
        "Inspect the workspace",
        "pro",
        "best_of_n",
        "planner",
        "seed-pro-v1",
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "inspect".to_string(),
                role: "worker".to_string(),
                model: "worker".to_string(),
                subtask: "Inspect bounded evidence".to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: max_model_turns,
            max_tool_calls_per_step: 6,
            max_output_tokens_per_step: 4_096,
        },
    )
}

#[test]
fn tool_registry_cache_is_versioned_and_bounded() {
    let mut cache = ToolRegistryCache::default();
    let active_root = PathBuf::from("/tmp/cindx-tool-cache-active");
    cache.insert(active_root.clone(), 7, ToolRegistry::new());

    assert!(cache.get(&active_root, 7).is_some());
    assert!(cache.get(&active_root, 8).is_none());

    for index in 0..TOOL_REGISTRY_CACHE_LIMIT {
        cache.insert(
            PathBuf::from(format!("/tmp/cindx-tool-cache-{index}")),
            7,
            ToolRegistry::new(),
        );
    }
    assert_eq!(cache.entries.len(), TOOL_REGISTRY_CACHE_LIMIT);

    cache.clear();
    assert!(cache.entries.is_empty());
}

#[test]
fn collaboration_tool_worker_reserves_a_terminal_answer_turn() {
    let turn_policy = WorkerTurnPolicy::isolated_evidence(3, true);
    let mut runtime = start_agent_loop(
        TaskId("worker-finalization".to_string()),
        "Inspect evidence",
        AgentRuntimeConfig {
            max_turns: turn_policy.runtime_turn_limit(),
        },
    );

    assert_eq!(runtime.max_turns, 5);
    assert!(turn_policy.supports_evidence_repair());
    runtime.turn = 2;
    assert_eq!(
        turn_policy.prepare_turn(&mut runtime),
        WorkerTurnPhase::Evidence
    );
    runtime.turn = 3;
    assert_eq!(
        turn_policy.prepare_turn(&mut runtime),
        WorkerTurnPhase::Finalization
    );
    assert_eq!(
        runtime
            .messages
            .last()
            .and_then(|message| message.metadata.get("kind"))
            .map(String::as_str),
        Some("worker_finalization")
    );
    let message_count = runtime.messages.len();
    assert_eq!(
        turn_policy.prepare_turn(&mut runtime),
        WorkerTurnPhase::Finalization
    );
    assert_eq!(runtime.messages.len(), message_count);
}

#[test]
fn workflow_continuation_reaches_worker_turns_and_attempts() {
    let plan = single_step_workflow_plan(3);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume-worker", plan.clone(), 10);
    let genome = ConductorPromptGenome::seed_for_effort("pro");

    assert_eq!(effective_workflow_model_turn_budget(&plan, &checkpoint), 3);
    assert_eq!(
        effective_workflow_step_attempt_budget(&genome, &checkpoint),
        3
    );

    checkpoint.continue_with_budget(3, 20);

    assert_eq!(effective_workflow_model_turn_budget(&plan, &checkpoint), 6);
    assert_eq!(
        effective_workflow_step_attempt_budget(&genome, &checkpoint),
        6
    );
    checkpoint
        .begin_step_with_attempt_limit("inspect", "worker", 6, 30)
        .expect("continued attempt budget should reach the worker");
}

#[test]
fn running_workflow_attempt_resumes_without_consuming_another_attempt() {
    let plan = single_step_workflow_plan(3);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume-running", plan, 10);

    let claimed = checkpoint
        .claim_steps(&["inspect".to_string()], 3, 20)
        .expect("first attempt should start");
    assert!(!claimed[0].resumed);
    let resumed = checkpoint
        .claim_steps(&["inspect".to_string()], 3, 30)
        .expect("running attempt should resume");
    assert!(resumed[0].resumed);
    assert_eq!(checkpoint.steps["inspect"].attempts, 1);

    checkpoint
        .fail_step("inspect", "transport failed", 40)
        .expect("attempt should fail");
    checkpoint
        .begin_step_with_attempt_limit("inspect", "worker-alt", 3, 50)
        .expect("failed attempt should restart");
    assert_eq!(checkpoint.steps["inspect"].attempts, 2);
    assert_eq!(checkpoint.steps["inspect"].model, "worker-alt");
}

#[test]
fn adaptive_recovery_model_obeys_policy_and_rotates_alternates() {
    let models = vec![
        "worker-a".to_string(),
        "worker-b".to_string(),
        "worker-c".to_string(),
    ];
    let transient = AgentFailure::new(
        "provider_timeout",
        "failed",
        AgentFailureClass::ProviderTransient,
        true,
    );

    assert_eq!(
        adaptive_recovery_model(
            "inspect",
            "worker-a",
            2,
            &models,
            PromptRetryPolicy::AlternateModel,
            &transient,
        )
        .expect("alternate should exist"),
        "worker-b"
    );
    assert_eq!(
        adaptive_recovery_model(
            "inspect",
            "worker-a",
            3,
            &models,
            PromptRetryPolicy::AlternateModel,
            &transient,
        )
        .expect("second alternate should exist"),
        "worker-c"
    );
    assert_eq!(
        adaptive_recovery_model(
            "inspect",
            "worker-a",
            2,
            &models,
            PromptRetryPolicy::SameModel,
            &transient,
        )
        .expect("same-model retry should remain available"),
        "worker-a"
    );
    assert!(adaptive_recovery_model(
        "inspect",
        "worker-a",
        2,
        &models,
        PromptRetryPolicy::FailFast,
        &transient,
    )
    .expect_err("fail-fast should reject recovery")
    .contains("fail-fast"));

    let budget = AgentFailure::budget("turn_budget", "spent");
    assert!(adaptive_recovery_model(
        "inspect",
        "worker-a",
        2,
        &models,
        PromptRetryPolicy::AlternateModel,
        &budget,
    )
    .expect_err("budget exhaustion must not trigger another model")
    .contains("cannot recover"));
}

#[test]
fn adaptive_layer_failure_preserves_every_failed_step() {
    let error = adaptive_layer_failure_error(&[
        "step inspect failed after recovery: timeout".to_string(),
        "step verify failed after recovery: invalid response".to_string(),
    ])
    .expect("layer failures should be resumable");

    assert!(error.starts_with(WORKFLOW_RESUMABLE_ERROR_PREFIX));
    assert!(error.contains("step inspect"));
    assert!(error.contains("step verify"));
    assert!(adaptive_layer_failure_error(&[]).is_none());
}

#[test]
fn adaptive_partial_handoff_preserves_completed_branches_without_claiming_completion() {
    let outputs = BTreeMap::from([
        (
            "inspect".to_string(),
            "Found the failing module and evidence A.".to_string(),
        ),
        (
            "design".to_string(),
            "Proposed a bounded repair with test B.".to_string(),
        ),
    ]);
    let handoff = adaptive_partial_work_handoff(
        "repair the project",
        &outputs,
        &["step verify failed after recovery: timeout".to_string()],
    )
    .expect("completed branches should produce a recoverable handoff");

    assert!(handoff.contains("INTERNAL PARTIAL WORKFLOW HANDOFF"));
    assert!(handoff.contains("Found the failing module"));
    assert!(handoff.contains("Proposed a bounded repair"));
    assert!(handoff.contains("step verify failed"));
    assert!(adaptive_partial_work_handoff(
        "repair the project",
        &BTreeMap::new(),
        &["failed".to_string()]
    )
    .is_none());
}

#[test]
fn anytime_best_known_output_tracks_verification_and_rejection() {
    let plan = single_step_workflow_plan(2);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume", plan, 1);
    let mut controller = AnytimeController::new(AnytimeControllerConfig {
        max_parallelism: 2,
        min_successful_candidates: 1,
        max_candidates: 3,
        min_usable_quality_bps: 4_500,
        stop_policy: ConductorStopPolicy::FirstVerified,
        min_team_uplift_bps: 0,
        min_distinct_contributions: 0,
        requires_synthesis: false,
        verification_required: false,
    });
    controller
        .register(AnytimeCandidate::direct_anchor(DIRECT_ANCHOR_CANDIDATE_ID))
        .unwrap();
    controller
        .register(AnytimeCandidate::workflow("inspect", Vec::new(), 7_000))
        .unwrap();
    controller.mark_running(DIRECT_ANCHOR_CANDIDATE_ID).unwrap();
    controller
        .observe(
            DIRECT_ANCHOR_CANDIDATE_ID,
            AnytimeVerdict {
                quality_bps: 6_000,
                confidence_bps: 5_500,
                constraint_coverage_bps: 6_000,
                evidence_count: 0,
                safety_violations: 0,
                deliverable: true,
                verified: false,
                anchor_uplift_bps: None,
            },
        )
        .unwrap();
    controller.mark_running("inspect").unwrap();
    controller
        .observe(
            "inspect",
            AnytimeVerdict {
                quality_bps: 7_500,
                confidence_bps: 8_000,
                constraint_coverage_bps: 8_000,
                evidence_count: 3,
                safety_violations: 0,
                deliverable: true,
                verified: true,
                anchor_uplift_bps: None,
            },
        )
        .unwrap();
    checkpoint
        .anytime_outputs
        .insert(DIRECT_ANCHOR_CANDIDATE_ID.to_string(), "anchor".to_string());
    checkpoint
        .anytime_outputs
        .insert("inspect".to_string(), "verified workflow".to_string());

    let (candidate_id, output, verdict) =
        anytime_best_known_output(&controller, &checkpoint).unwrap();
    assert_eq!(candidate_id, "inspect");
    assert_eq!(output, "verified workflow");
    assert!(verdict.verified);

    controller
        .revise(
            "inspect",
            AnytimeVerdict {
                safety_violations: 1,
                ..verdict
            },
        )
        .unwrap();
    let (candidate_id, output, _) = anytime_best_known_output(&controller, &checkpoint).unwrap();
    assert_eq!(candidate_id, DIRECT_ANCHOR_CANDIDATE_ID);
    assert_eq!(output, "anchor");
}

#[test]
fn partial_handoff_enters_the_anytime_frontier_and_checkpoint() {
    let plan = single_step_workflow_plan(2);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume", plan, 1);
    let mut controller = AnytimeController::new(AnytimeControllerConfig {
        max_parallelism: 2,
        min_successful_candidates: 1,
        max_candidates: 2,
        min_usable_quality_bps: 4_500,
        stop_policy: ConductorStopPolicy::Quorum,
        min_team_uplift_bps: 0,
        min_distinct_contributions: 0,
        requires_synthesis: false,
        verification_required: false,
    });
    controller
        .register(AnytimeCandidate::direct_anchor(DIRECT_ANCHOR_CANDIDATE_ID))
        .unwrap();
    controller.mark_running(DIRECT_ANCHOR_CANDIDATE_ID).unwrap();
    controller
        .observe(
            DIRECT_ANCHOR_CANDIDATE_ID,
            AnytimeVerdict {
                quality_bps: 6_000,
                confidence_bps: 5_500,
                constraint_coverage_bps: 6_000,
                evidence_count: 0,
                safety_violations: 0,
                deliverable: true,
                verified: false,
                anchor_uplift_bps: None,
            },
        )
        .unwrap();
    checkpoint
        .anytime_outputs
        .insert(DIRECT_ANCHOR_CANDIDATE_ID.to_string(), "anchor".to_string());

    register_partial_handoff_candidate(
        &mut controller,
        &mut checkpoint,
        "partial evidence handoff",
        2,
        1,
        4,
    )
    .unwrap();

    let (candidate_id, output, verdict) =
        anytime_best_known_output(&controller, &checkpoint).unwrap();
    assert_eq!(candidate_id, PARTIAL_HANDOFF_CANDIDATE_ID);
    assert_eq!(output, "partial evidence handoff");
    assert_eq!(verdict.evidence_count, 4);
    assert!(!verdict.verified);
    assert!(!checkpoint.anytime_controller_json.is_empty());
}

#[test]
fn adaptive_quality_gate_is_bounded_and_requires_safe_passing_score() {
    assert_eq!(
        adaptive_quality_repair_budget(PromptVerification::Minimal),
        0
    );
    assert_eq!(
        adaptive_quality_repair_budget(PromptVerification::Evidence),
        1
    );
    assert_eq!(
        adaptive_quality_repair_budget(PromptVerification::Adversarial),
        2
    );

    let mut gate = CollaborationQualityPayload {
        pass: true,
        score: ADAPTIVE_QUALITY_PASS_SCORE,
        issues: Vec::new(),
        safety_violations: 0,
    };
    assert!(adaptive_quality_gate_passes(&gate));
    gate.score = ADAPTIVE_QUALITY_PASS_SCORE - 0.01;
    assert!(!adaptive_quality_gate_passes(&gate));
    gate.score = 1.0;
    gate.safety_violations = 1;
    assert!(!adaptive_quality_gate_passes(&gate));
    gate.safety_violations = 0;
    gate.score = 1.01;
    assert!(!adaptive_quality_gate_passes(&gate));
    gate.score = f32::NAN;
    assert!(!adaptive_quality_gate_passes(&gate));
}

#[test]
fn default_provider_quality_review_prefers_a_distinct_hidden_evaluator_model() {
    let openai = ProviderConfig::default();
    let openai_participants = [
        openai.model_for_conductor(),
        openai.model_for_role(&ModelRole::Planner),
        openai.model_for_role(&ModelRole::Executor),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        adaptive_quality_reviewer_models(&openai, &openai_participants)
            .first()
            .map(String::as_str),
        Some("gpt-4.1-mini")
    );

    let alibaba = ProviderConfig {
        provider_id: PROVIDER_ALIBABA_CN.to_string(),
        model: "qwen3.7-plus".to_string(),
        conductor_model: "qwen3.7-plus".to_string(),
        planner_model: "qwen3.7-plus".to_string(),
        executor_model: "qwen3.7-plus".to_string(),
        reviewer_model: "qwen3.7-plus".to_string(),
        summarizer_model: "qwen3.7-flash".to_string(),
        ..ProviderConfig::default()
    };
    let alibaba_participants = [
        alibaba.model_for_conductor(),
        alibaba.model_for_role(&ModelRole::Planner),
        alibaba.model_for_role(&ModelRole::Executor),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        adaptive_quality_reviewer_models(&alibaba, &alibaba_participants)
            .first()
            .map(String::as_str),
        Some("qwen3.7-flash")
    );
}

#[test]
fn adaptive_quality_search_preserves_the_safest_highest_scoring_anchor() {
    let anchor = CollaborationQualityPayload {
        pass: false,
        score: 0.74,
        issues: vec!["one remaining issue".to_string()],
        safety_violations: 0,
    };
    let regressed_repair = CollaborationQualityPayload {
        pass: true,
        score: 0.96,
        issues: Vec::new(),
        safety_violations: 1,
    };
    assert!(!adaptive_quality_candidate_is_better(
        &regressed_repair,
        &anchor
    ));

    let improved_repair = CollaborationQualityPayload {
        pass: true,
        score: ADAPTIVE_QUALITY_PASS_SCORE,
        issues: Vec::new(),
        safety_violations: 0,
    };
    assert!(adaptive_quality_candidate_is_better(
        &improved_repair,
        &anchor
    ));
}

#[test]
fn adaptive_quality_handoff_preserves_issues_and_fails_closed_on_safety() {
    let unresolved = AdaptiveQualityGateResult {
        output: "candidate guidance".to_string(),
        score: 0.61,
        safety_violations: 0,
        passed: false,
        issues: vec!["verify the generated artifact".to_string()],
    };
    let handoff = adaptive_quality_handoff(&unresolved).expect("safe issues should be delegated");
    assert!(handoff.contains("INTERNAL QUALITY HANDOFF"));
    assert!(handoff.contains("verify the generated artifact"));
    assert!(handoff.contains("candidate guidance"));

    let passed = AdaptiveQualityGateResult {
        passed: true,
        issues: Vec::new(),
        score: 0.9,
        ..unresolved
    };
    assert_eq!(
        adaptive_quality_handoff(&passed).expect("passing guidance should flow through"),
        "candidate guidance"
    );

    let unsafe_result = AdaptiveQualityGateResult {
        safety_violations: 1,
        ..passed
    };
    let error = adaptive_quality_handoff(&unsafe_result)
        .expect_err("safety violations must stop the workflow");
    assert!(error.starts_with(WORKFLOW_SAFETY_ERROR_PREFIX));
}

#[test]
fn transient_provider_failures_are_retryable_but_invalid_requests_are_not() {
    let broken_pipe = ModelError::new("failed to configure curl: Broken pipe (os error 32)");
    let timeout =
        ModelError::new("model stream timed out after 180 seconds without receiving data");
    let invalid = ModelError::with_status(
        400,
        "invalid_request_error: Unexpected item type in content",
    );
    assert!(broken_pipe.is_retryable());
    assert!(timeout.is_retryable());
    assert!(!invalid.is_retryable());
    assert_eq!(
        exhausted_model_transport_error_stop_reason(&broken_pipe),
        Some(RunStopReason::ProviderUnavailable)
    );
    assert_eq!(exhausted_model_transport_error_stop_reason(&invalid), None);
}

#[test]
fn context_estimate_accounts_for_multibyte_text_and_prompt_reserve() {
    let ascii = test_message(MessageRole::User, "abcdefgh");
    let chinese = test_message(MessageRole::User, "你好世界你好世界");
    assert!(estimate_message_tokens(&chinese) > estimate_message_tokens(&ascii));

    let mut image_message = test_message(MessageRole::User, "inspect these images");
    image_message.metadata.insert(
        "image_paths".to_string(),
        "/tmp/one.png\n/tmp/two.png".to_string(),
    );
    assert!(
        estimate_message_tokens(&image_message)
            >= estimate_text_tokens_for_context("inspect these images") + 2_048
    );

    let large_history = vec![test_message(MessageRole::User, "a".repeat(220_000))];
    let plan = session_compaction_plan(&large_history, 100_000);
    assert!(plan.should_compact);
    assert!(plan.estimated_request_tokens > plan.estimated_history_tokens);
}

#[test]
fn context_checkpoint_reuse_has_bounded_hysteresis() {
    let mut history = (0..40)
        .map(|index| {
            test_message(
                if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                format!("message {index}"),
            )
        })
        .collect::<Vec<_>>();
    let checkpoint = ValidatedContextCheckpoint {
        text: "verified checkpoint".to_string(),
        covered_messages: 8,
    };
    let plan = session_compaction_plan(&history, 32_000);
    assert!(context_checkpoint_is_within_reuse_window(
        &checkpoint,
        &history,
        plan,
        32_000,
    ));

    history.extend(
        (40..58).map(|index| test_message(MessageRole::Assistant, format!("message {index}"))),
    );
    let plan = session_compaction_plan(&history, 32_000);
    assert!(!context_checkpoint_is_within_reuse_window(
        &checkpoint,
        &history,
        plan,
        32_000,
    ));
}

#[test]
fn recent_context_starts_on_a_complete_user_turn() {
    let history = vec![
        test_message(MessageRole::User, "old request"),
        test_message(MessageRole::Assistant, "old answer"),
        test_message(MessageRole::Tool, "old tool evidence"),
        test_message(MessageRole::User, "latest request"),
        test_message(MessageRole::Assistant, "latest answer"),
    ];
    let budget = estimate_message_tokens(&history[3]) + estimate_message_tokens(&history[4]);
    let (start, tokens) = recent_history_start(&history, budget);

    assert_eq!(start, 3);
    assert!(is_user_turn_start(&history[start]));
    assert_eq!(tokens, budget);

    let (narrow_start, _) = recent_history_start(
        &history,
        estimate_message_tokens(history.last().expect("latest message")),
    );
    assert_eq!(narrow_start, 3);
}

#[test]
fn context_checkpoint_events_stop_at_the_covered_message_prefix() {
    let session_id = "session-prefix";
    let event = |sequence: u64, kind: EventKind, role: Option<&str>, content: Option<&str>| {
        let mut metadata = [("session_id".to_string(), session_id.to_string())]
            .into_iter()
            .collect::<Metadata>();
        if let Some(role) = role {
            metadata.insert("role".to_string(), role.to_string());
        }
        if let Some(content) = content {
            metadata.insert("content".to_string(), content.to_string());
        }
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: phase16_task_id(),
            timestamp_ms: sequence,
            sequence,
            kind,
            summary: format!("event {sequence}"),
            metadata,
        }
    };
    let events = vec![
        event(1, EventKind::MessageAdded, Some("user"), Some("first")),
        event(2, EventKind::ToolCallFinished, None, None),
        event(
            3,
            EventKind::MessageAdded,
            Some("assistant"),
            Some("answer"),
        ),
        event(4, EventKind::TaskStatusChanged, None, None),
        event(5, EventKind::MessageAdded, Some("user"), Some("retained")),
    ];

    let covered = context_events_for_covered_history_prefix(&events, 2);

    assert_eq!(covered.len(), 3);
    assert_eq!(covered.last().map(|event| event.sequence), Some(3));
    assert!(covered
        .iter()
        .all(|event| event.metadata.get("content").map(String::as_str) != Some("retained")));
}

#[test]
fn installed_app_data_is_user_scoped_and_overrideable() {
    let home = PathBuf::from("/Users/new-cindx-user");
    let expected = home
        .join("Library")
        .join("Application Support")
        .join("Cindx");
    assert_eq!(app_data_root_for(None, Some(home)), expected);

    let override_root = PathBuf::from("/tmp/cindx-portable-data");
    assert_eq!(
        app_data_root_for(Some(override_root.clone()), None),
        override_root
    );
    assert_eq!(database_path(), app_data_root().join("state.sqlite3"));
}

#[test]
fn persistent_app_store_creates_secures_and_reopens_database() {
    let root = temp_test_root("cindx-persistent-store");
    let database = root.join("state.sqlite3");

    let store = open_app_store_at(&database).expect("persistent store should open");
    drop(store);

    assert!(database.is_file());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&database)
            .expect("database metadata should load")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(
        SqliteStore::open_read_only(&database)
            .expect("created persistent store should reopen read-only"),
    );

    fs::remove_dir_all(root).expect("persistent store fixture should be removed");
}

#[test]
fn persistent_app_store_reports_database_path_when_open_fails() {
    let root = temp_test_root("cindx-persistent-store-failure");
    let database = root.join("state.sqlite3");
    fs::create_dir_all(&database).expect("database-path directory fixture should exist");

    let error = match open_app_store_at(&database) {
        Ok(_) => panic!("a directory must not be accepted as a persistent database"),
        Err(error) => error,
    };

    assert!(error
        .message
        .contains("failed to open Cindx state database"));
    assert!(error.message.contains(&database.display().to_string()));

    fs::remove_dir_all(root).expect("persistent store failure fixture should be removed");
}

#[test]
fn attachment_paths_stay_inside_project_managed_storage() {
    let root = std::env::temp_dir().join(format!(
        "cindx-attachment-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    let attachment_dir = root.join(".cindx/attachments/session-a");
    fs::create_dir_all(&attachment_dir).expect("attachment directory should exist");
    let attachment = attachment_dir.join("image.png");
    fs::write(&attachment, b"png").expect("attachment should write");
    let outside = root.join("outside.png");
    fs::write(&outside, b"png").expect("outside fixture should write");

    assert_eq!(
        validated_attachment_path(&root, &attachment.display().to_string())
            .expect("managed attachment should validate"),
        fs::canonicalize(&attachment).expect("attachment should resolve")
    );
    assert!(validated_attachment_path(&root, &outside.display().to_string()).is_err());
    assert_eq!(safe_attachment_name("../nested/screen.png"), "screen.png");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn user_message_projection_preserves_attachment_metadata() {
    let attachment = AgentAttachmentView {
        id: "attachment-1".to_string(),
        name: "screen.png".to_string(),
        path: "/tmp/cindx/screen.png".to_string(),
        mime_type: "image/png".to_string(),
        size_bytes: 1_024,
    };
    let mut metadata = [
        ("role".to_string(), "user".to_string()),
        ("content".to_string(), "Review this screenshot".to_string()),
        ("queue_id".to_string(), "steer-queue-1".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    add_attachment_metadata(&mut metadata, std::slice::from_ref(&attachment));
    let event = Event {
        id: EventId("message-with-attachment".to_string()),
        task_id: phase16_task_id(),
        sequence: 9,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata,
    };

    let message = message_view_from_event(&event).expect("message should project");
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].id, attachment.id);
    assert_eq!(message.attachments[0].name, attachment.name);
    assert_eq!(message.attachments[0].path, attachment.path);
    assert_eq!(message.attachments[0].mime_type, attachment.mime_type);
    assert_eq!(message.attachments[0].size_bytes, attachment.size_bytes);
    assert_eq!(message.queue_id.as_deref(), Some("steer-queue-1"));
}

#[test]
fn attachment_message_separates_display_and_model_content() {
    let event = Event {
        id: EventId("message-with-model-content".to_string()),
        task_id: phase16_task_id(),
        sequence: 10,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), "Review this file".to_string()),
            (
                "model_content".to_string(),
                "Review this file\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let display = message_from_event(&event).expect("display transcript should project");
    let model = model_message_from_event(&event).expect("model transcript should project");
    let runtime = runtime_message_from_event(&event).expect("runtime transcript should project");

    assert_eq!(display.content, "Review this file");
    assert_eq!(model.content, runtime.content);
    assert!(model.content.contains("/workspace/report.pdf"));
}

#[test]
fn recovery_identity_stays_on_root_prompt_after_steer() {
    let run_start = Event {
        id: EventId("run-start".to_string()),
        task_id: phase16_task_id(),
        sequence: 20,
        timestamp_ms: 100,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task started".to_string(),
        metadata: [
            (
                "agent_run_identity_schema".to_string(),
                "cindx.agent-run-identity.v1".to_string(),
            ),
            ("agent_run_id".to_string(), "run-a".to_string()),
            (
                "logical_agent_run_id".to_string(),
                "logical-run-a".to_string(),
            ),
            ("prompt".to_string(), "Review the report".to_string()),
            (
                "model_prompt".to_string(),
                "Review the report\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
            (
                "recovery_prompt".to_string(),
                "Review the report\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let root_turn = Event {
        id: EventId("root-turn".to_string()),
        task_id: phase16_task_id(),
        sequence: 21,
        timestamp_ms: 101,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), "Review the report".to_string()),
            (
                "model_content".to_string(),
                "Review the report\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let steer_turn = Event {
        id: EventId("steer-turn".to_string()),
        task_id: phase16_task_id(),
        sequence: 22,
        timestamp_ms: 102,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("role".to_string(), "user".to_string()),
            (
                "content".to_string(),
                "Focus on security findings".to_string(),
            ),
            (
                "display_content".to_string(),
                "Focus on security findings".to_string(),
            ),
            ("steer".to_string(), "true".to_string()),
            ("queue_mode".to_string(), "steer".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let events = vec![run_start, root_turn, steer_turn];

    assert_eq!(
        latest_agent_prompt_from_active_events(&events).as_deref(),
        Some("Focus on security findings")
    );
    assert_eq!(
        latest_agent_display_prompt_from_active_events(&events).as_deref(),
        Some("Focus on security findings")
    );
    assert_eq!(
        agent_recovery_prompt_from_active_events(&events).as_deref(),
        Some("Review the report\n\nAttached files: /workspace/report.pdf")
    );
    assert_eq!(
        primary_agent_user_turn_event(&events).map(|event| event.sequence),
        Some(21)
    );
    let recovery_context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
    ]
    .into_iter()
    .collect();
    let recovery =
        crate::agent_recovery_identity::resolve_agent_recovery_identity(&events, &recovery_context)
            .expect("recovery identity should resolve");
    assert_eq!(recovery.identity.source_run_id, "run-a");
    assert_eq!(
        recovery.identity.logical_run_id.as_deref(),
        Some("logical-run-a")
    );
    assert_eq!(recovery.identity.user_turn_sequence, 21);
    assert_eq!(
        recovery.identity.prompt_fingerprint,
        sha256_hex("Review the report\n\nAttached files: /workspace/report.pdf".as_bytes())
    );
    assert_eq!(
        recovery.prompt,
        "Review the report\n\nAttached files: /workspace/report.pdf"
    );
}

#[test]
fn runtime_snapshot_matches_the_redacted_durable_projection() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "api_key=super-secret".to_string(),
        AgentRuntimeConfig::default(),
    );
    let effective_objective =
        "Initial request:\napi_key=super-secret\n\nAccepted steering 1:\nInspect token=private-value";
    let prepared_context = [
        (
            "effective_prompt_objective".to_string(),
            effective_objective.to_string(),
        ),
        ("steer_epoch".to_string(), "2".to_string()),
        ("prompt_contract_epoch".to_string(), "1".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    runtime.replace_prepared_task_state(agent_runtime::PreparedTaskState::from_run_context(
        &prepared_context,
        &runtime.user_prompt,
        agent_runtime::prompt_completion_intent(&prepared_context),
    ));
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: "<think>private scratchpad</think>\nFinished".to_string(),
        metadata: Metadata::new(),
    });

    let snapshot = capture_persistable_agent_task_state(&runtime);
    let mut persisted_messages = runtime.messages.clone();
    for message in &mut persisted_messages {
        if message.role == MessageRole::Assistant {
            message.content = sanitize_assistant_content(&message.content);
        }
        message.content = redact_sensitive_text(&message.content);
        message.metadata = redact_metadata(&message.metadata);
    }

    assert!(snapshot
        .restore_with_effective_objective(
            redact_sensitive_text(&runtime.user_prompt),
            persisted_messages,
            redact_sensitive_text(effective_objective),
        )
        .is_ok());
    assert!(snapshot
        .restore(runtime.user_prompt.clone(), runtime.messages.clone())
        .is_err());
}

#[test]
fn assistant_reasoning_control_only_message_is_sanitized_for_chat() {
    let event = Event {
        id: EventId("assistant-reasoning-control".to_string()),
        task_id: phase16_task_id(),
        sequence: 10,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "assistant message".to_string(),
        metadata: [
            ("role".to_string(), "assistant".to_string()),
            ("content".to_string(), "</think>".to_string()),
            ("raw_tool_calls_json".to_string(), "[]".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    let chat_message = message_view_from_event(&event).expect("message should project");
    assert_eq!(chat_message.content, "");
    let transcript_message = message_from_event(&event).expect("tool turn should remain");
    assert_eq!(transcript_message.content, "");
    assert!(transcript_message
        .metadata
        .contains_key("raw_tool_calls_json"));
}

#[test]
fn assistant_dsml_tool_protocol_is_sanitized_for_chat_and_transcript() {
    let event = Event {
        id: EventId("assistant-dsml-tool-protocol".to_string()),
        task_id: phase16_task_id(),
        sequence: 11,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "assistant message".to_string(),
        metadata: [
            ("role".to_string(), "assistant".to_string()),
            (
                "content".to_string(),
                concat!(
                    "<｜DSML｜tool_calls>",
                    "<｜DSML｜invoke name=\"shell_run\">",
                    "<｜DSML｜parameter name=\"command\" string=\"true\">pwd</｜DSML｜parameter>",
                    "</｜DSML｜invoke>",
                    "</｜DSML｜tool_calls>"
                )
                .to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let chat_message = message_view_from_event(&event).expect("message should project");
    assert_eq!(chat_message.content, "");
    let transcript_message = message_from_event(&event).expect("message should remain");
    assert_eq!(transcript_message.content, "");
}

#[test]
fn durable_internal_instruction_is_hidden_from_chat_but_restored_for_runtime() {
    let event = Event {
        id: EventId("internal-verification-instruction".to_string()),
        task_id: phase16_task_id(),
        sequence: 12,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "system message".to_string(),
        metadata: [
            ("role".to_string(), "system".to_string()),
            ("content".to_string(), "verify the mutation".to_string()),
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), "completion_verification".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    assert!(message_from_event(&event).is_none());
    let runtime_message =
        runtime_message_from_event(&event).expect("runtime instruction should restore");
    assert_eq!(runtime_message.content, "verify the mutation");
    assert_eq!(
        runtime_message.metadata.get("kind").map(String::as_str),
        Some("completion_verification")
    );
}

#[test]
fn computer_screenshot_becomes_the_primary_visual_artifact() {
    let root = std::env::temp_dir().join(format!(
        "cindx-screenshot-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(root.join(".cindx/computer-actions"))
        .expect("computer action directory should exist");
    fs::write(root.join(".cindx/computer-actions/request.json"), b"{}")
        .expect("request fixture should write");
    fs::write(root.join(".cindx/computer-actions/screen.png"), b"png")
        .expect("screenshot fixture should write");
    let mut result = ToolResult::text(
        agent_core::ToolCallId("computer-test".to_string()),
        ToolOutcomeStatus::Succeeded,
        "captured",
        [
            (
                "artifact_path".to_string(),
                ".cindx/computer-actions/request.json".to_string(),
            ),
            (
                "screenshot_path".to_string(),
                ".cindx/computer-actions/screen.png".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );

    materialize_tool_result_artifacts(&mut result, &root).expect("artifacts should materialize");

    assert!(result
        .metadata
        .get("artifact_path")
        .is_some_and(|path| path.ends_with("screen.png")));
    assert_eq!(tool_result_image_paths(&result).len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn repeated_image_outputs_keep_distinct_immutable_versions() {
    let root = std::env::temp_dir().join(format!(
        "cindx-versioned-image-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    let relative_path = "generated-images/cat.png";
    let source = root.join(relative_path);
    fs::create_dir_all(source.parent().expect("image parent should exist"))
        .expect("image directory should be created");

    let materialize = |call_id: &str, bytes: &[u8]| {
        fs::write(&source, bytes).expect("image version should write");
        let mut result = ToolResult::text(
            agent_core::ToolCallId(call_id.to_string()),
            ToolOutcomeStatus::Succeeded,
            "generated",
            [("artifact_path".to_string(), relative_path.to_string())]
                .into_iter()
                .collect(),
        );
        result.artifacts.push(ToolArtifact {
            path: relative_path.to_string(),
            mime_type: Some("image/png".to_string()),
            title: Some("Generated image".to_string()),
        });
        materialize_tool_result_artifacts(&mut result, &root)
            .expect("image artifact should materialize");
        result
    };

    let first = materialize("image-call-one", b"version-one");
    let first_snapshot = first.metadata["artifact_path"].clone();
    let second = materialize("image-call-two", b"version-two");
    let second_snapshot = second.metadata["artifact_path"].clone();

    assert_eq!(first.metadata["source_path"], relative_path);
    assert_eq!(second.metadata["source_path"], relative_path);
    assert_ne!(first_snapshot, second_snapshot);
    assert_eq!(
        fs::read(root.join(&first_snapshot)).expect("first snapshot should remain readable"),
        b"version-one"
    );
    assert_eq!(
        fs::read(root.join(&second_snapshot)).expect("second snapshot should remain readable"),
        b"version-two"
    );
    assert_eq!(first.artifacts.len(), 1);
    assert_eq!(second.artifacts.len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_structured_tool_output_is_materialized_without_inline_duplication() {
    let root = std::env::temp_dir().join(format!(
        "cindx-structured-output-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(&root).expect("test workspace should exist");
    let structured = serde_json::json!({ "payload": "x".repeat(300 * 1024) }).to_string();
    let mut result = ToolResult::text(
        agent_core::ToolCallId("call/unsafe".to_string()),
        ToolOutcomeStatus::Succeeded,
        "bounded preview",
        Metadata::new(),
    );
    result.structured_output_json = Some(structured.clone());

    materialize_tool_result_artifacts(&mut result, &root)
        .expect("structured output should materialize");

    let path = result
        .metadata
        .get("structured_output_path")
        .expect("structured output should expose its artifact path");
    assert!(path.ends_with("call_unsafe-structured.json"));
    assert_eq!(
        fs::read_to_string(path).expect("structured artifact should read"),
        structured
    );
    assert!(result
        .structured_output_json
        .as_deref()
        .is_some_and(|value| value.len() < 512));
    assert!(result
        .metadata
        .get("structured_output")
        .is_some_and(|value| value.len() < 512));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_status_exposes_expected_modes() {
    let status = runtime_status_for_root(workspace_root());

    assert_eq!(status.app_version, env!("CARGO_PKG_VERSION"));
    assert!(!status.provider_ready);
    assert!(status
        .orchestration_modes
        .contains(&"plan_execute_review".to_string()));
    assert!(status.registered_tools.contains(&"shell.run".to_string()));
}

#[test]
fn runtime_status_exposes_authoritative_agent_run_budgets() {
    let status = runtime_status_for_root(workspace_root());

    for (effort, exposed) in [
        ("fast", status.agent_run_budgets.fast),
        ("auto", status.agent_run_budgets.auto),
        ("pro", status.agent_run_budgets.pro),
    ] {
        let authoritative = RunBudget::for_effort(effort);
        assert_eq!(
            exposed.max_duration_ms,
            u64::try_from(authoritative.max_duration.as_millis()).unwrap()
        );
        assert_eq!(exposed.max_model_calls, authoritative.max_model_calls);
        assert_eq!(exposed.max_tool_calls, authoritative.max_tool_calls);
    }
}

#[test]
fn quit_confirmation_preference_requires_explicit_suppression() {
    assert!(quit_confirmation_suppressed_text(
        "theme=system\nskip_quit_confirmation=true\n"
    ));
    assert!(!quit_confirmation_suppressed_text(
        "skip_quit_confirmation=false\n"
    ));
}

#[test]
fn phase3_mock_permission_round_trips_through_store() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let state = request_mock_permission_in_store(&mut store).expect("request should save");

    assert_eq!(state.permissions.len(), 1);
    assert_eq!(state.permissions[0].status, "pending");

    let request_id = state.permissions[0].id.clone();
    let resolved = resolve_permission_in_store(&mut store, &request_id, "deny")
        .expect("resolution should save");

    assert_eq!(resolved.permissions.len(), 1);
    assert_eq!(resolved.permissions[0].status, "resolved");
    assert_eq!(resolved.permissions[0].decision.as_deref(), Some("deny"));
    assert!(resolved
        .timeline
        .iter()
        .any(|entry| entry.label == "Permission resolved"));
}

#[test]
fn session_read_model_advances_from_only_new_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-read-model";
    let run_id = "run-read-model";
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("project_name".to_string(), "Project A".to_string()),
        ("session_id".to_string(), session_id.to_string()),
        ("session_name".to_string(), "Read model".to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [
                ("prompt".to_string(), "Inspect the workspace".to_string()),
                ("context_window_tokens".to_string(), "128000".to_string()),
                ("run_budget_ms".to_string(), "987654".to_string()),
                ("run_model_call_budget".to_string(), "37".to_string()),
                ("run_tool_call_budget".to_string(), "83".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("run start should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Inspect the workspace",
        context.clone(),
    )
    .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("prompt_tokens".to_string(), "640".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("model turn should append");

    let initial = load_agent_session_read_model(&mut store, session_id)
        .expect("initial read model should build");
    assert_eq!(initial.event_count, 3);
    assert_eq!(initial.state.turn_count, 1);
    assert_eq!(initial.state.context_tokens_used, 640);
    assert_eq!(initial.state.status, "running");
    assert_eq!(initial.state.run_budget_ms, 987_654);
    assert_eq!(initial.state.run_model_call_budget, 37);
    assert_eq!(initial.state.run_tool_call_budget, 83);
    assert_eq!(initial.state.max_turns, 37);

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "Workspace inspected",
        context.clone(),
    )
    .expect("assistant message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context.clone(),
    )
    .expect("completion should append");

    let updated = load_agent_session_read_model(&mut store, session_id)
        .expect("read model should apply the delta");
    assert_eq!(updated.event_count, 5);
    assert_eq!(updated.revision, initial.revision + 2);
    assert_eq!(updated.state.status, "completed");
    assert_eq!(updated.state.run_budget_ms, 987_654);
    assert_eq!(updated.state.run_model_call_budget, 37);
    assert_eq!(updated.state.run_tool_call_budget, 83);
    assert_eq!(updated.state.max_turns, 37);
    assert_eq!(
        updated.state.latest_answer.as_deref(),
        Some("Workspace inspected")
    );
    assert!(updated.state.can_retry);

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [
                (
                    "agent_run_id".to_string(),
                    "run-read-model-fast".to_string(),
                ),
                ("agent_effort".to_string(), "fast".to_string()),
                ("prompt".to_string(), "Inspect another change".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("next run start should append");

    let restarted = load_agent_session_read_model(&mut store, session_id)
        .expect("read model should apply the next run delta");
    let fast_budget = RunBudget::for_effort("fast");
    assert_eq!(restarted.event_count, 6);
    assert_eq!(restarted.revision, updated.revision + 1);
    assert_eq!(restarted.state.status, "running");
    assert_eq!(
        restarted.state.run_budget_ms,
        u64::try_from(fast_budget.max_duration.as_millis()).unwrap()
    );
    assert_eq!(
        restarted.state.run_model_call_budget,
        fast_budget.max_model_calls
    );
    assert_eq!(restarted.state.max_turns, fast_budget.max_model_calls);
    assert_eq!(
        restarted.state.run_tool_call_budget,
        fast_budget.max_tool_calls
    );

    let persisted = store
        .load_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, session_id)
        .expect("persisted read model should load")
        .expect("persisted read model should exist");
    assert_eq!(persisted.revision, restarted.revision);
}

#[test]
fn session_read_model_normalizes_legacy_cached_max_turns() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-legacy-budget-cache";
    let mut legacy = load_agent_session_read_model(&mut store, session_id)
        .expect("empty read model should build");
    legacy.state.max_turns = RunBudget::for_effort("auto").max_model_calls;
    let payload = serde_json::to_string(&legacy).expect("legacy read model should serialize");
    store
        .save_read_model(
            AGENT_SESSION_READ_MODEL_NAMESPACE,
            session_id,
            legacy.revision,
            &payload,
        )
        .expect("legacy read model should save");

    let normalized = load_agent_session_read_model(&mut store, session_id)
        .expect("legacy read model should normalize");
    assert_eq!(normalized.state.run_model_call_budget, 0);
    assert_eq!(normalized.state.max_turns, 0);

    let persisted = store
        .load_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, session_id)
        .expect("normalized read model should load")
        .expect("normalized read model should exist");
    let persisted: serde_json::Value =
        serde_json::from_str(&persisted.payload).expect("normalized payload should deserialize");
    assert_eq!(persisted["state"]["maxTurns"], 0);
}

#[test]
fn session_read_model_rebuilds_missing_legacy_run_budgets_from_effort() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-legacy-running-budget-cache";
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), "run-legacy-fast".to_string()),
            ("agent_effort".to_string(), "FAST".to_string()),
            ("prompt".to_string(), "Inspect the workspace".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("legacy run start should append");

    let mut legacy = load_agent_session_read_model(&mut store, session_id)
        .expect("read model should initially build");
    legacy.state.run_budget_ms = 0;
    legacy.state.run_model_call_budget = 0;
    legacy.state.run_tool_call_budget = 0;
    legacy.state.max_turns = RunBudget::for_effort("auto").max_model_calls;
    let payload = serde_json::to_string(&legacy).expect("legacy read model should serialize");
    store
        .save_read_model(
            AGENT_SESSION_READ_MODEL_NAMESPACE,
            session_id,
            legacy.revision,
            &payload,
        )
        .expect("legacy read model should save");

    let rebuilt = load_agent_session_read_model(&mut store, session_id)
        .expect("legacy run budget should rebuild");
    let fast_budget = RunBudget::for_effort("fast");
    assert_eq!(
        rebuilt.state.run_budget_ms,
        u64::try_from(fast_budget.max_duration.as_millis()).unwrap()
    );
    assert_eq!(
        rebuilt.state.run_model_call_budget,
        fast_budget.max_model_calls
    );
    assert_eq!(rebuilt.state.max_turns, fast_budget.max_model_calls);
    assert_eq!(
        rebuilt.state.run_tool_call_budget,
        fast_budget.max_tool_calls
    );
}

#[test]
fn project_memory_read_model_persists_deduplicated_cross_session_requirements() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, run_id) in [
        ("session-memory-a", "run-memory-a"),
        ("session-memory-b", "run-memory-b"),
    ] {
        let context = [
            ("project_id".to_string(), "project-memory".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            context.clone(),
        )
        .expect("run should start");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            "Keep effort selection scoped to each session",
            context.clone(),
        )
        .expect("user requirement should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "Effort is now stored per session.",
            context.clone(),
        )
        .expect("assistant outcome should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task completed",
            context,
        )
        .expect("run should complete");
    }

    let ledger = load_project_memory_ledger(&mut store, "project-memory")
        .expect("memory read model should build");
    assert_eq!(ledger.project_id, "project-memory");
    assert_eq!(
        ledger
            .records
            .iter()
            .filter(|record| record.kind == agent_memory::MemoryKind::Requirement)
            .count(),
        1
    );
    let requirement = ledger
        .records
        .iter()
        .find(|record| record.kind == agent_memory::MemoryKind::Requirement)
        .expect("requirement memory should exist");
    assert_eq!(requirement.source_session_ids.len(), 2);
    let requirement_id = requirement.id.clone();
    let persisted = store
        .load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, "project-memory")
        .expect("memory read model should load")
        .expect("memory read model should exist");
    assert_eq!(persisted.revision, ledger.revision);

    let use_context = [
        ("project_id".to_string(), "project-memory".to_string()),
        ("session_id".to_string(), "session-memory-c".to_string()),
        ("agent_run_id".to_string(), "run-memory-c".to_string()),
        ("memory_ids".to_string(), requirement_id.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let used = record_project_memory_observed_use(
        &mut store,
        &phase16_task_id(),
        &use_context,
        "Kept effort selection scoped to each session.",
    )
    .expect("memory utilization should persist");
    assert_eq!(used, 1);
    let updated = load_project_memory_ledger(&mut store, "project-memory")
        .expect("updated memory ledger should load");
    assert_eq!(
        updated
            .records
            .iter()
            .find(|record| record.id == requirement_id)
            .map(|record| record.observed_use_count),
        Some(1)
    );
}

#[test]
fn project_memory_read_model_ignores_unrelated_project_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_a = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Keep project A requirements isolated",
        project_a.clone(),
    )
    .expect("project A message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_a,
    )
    .expect("project A run should complete");
    let initial =
        load_project_memory_ledger(&mut store, "project-a").expect("project A ledger should build");

    let project_b = [
        ("project_id".to_string(), "project-b".to_string()),
        ("session_id".to_string(), "session-b".to_string()),
        ("agent_run_id".to_string(), "run-b".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Unrelated project B requirement",
        project_b.clone(),
    )
    .expect("project B message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_b,
    )
    .expect("project B run should complete");

    let unchanged = load_project_memory_ledger(&mut store, "project-a")
        .expect("project A ledger should remain current");
    assert_eq!(unchanged.revision, initial.revision);
    assert_eq!(unchanged.event_count, initial.event_count);
    assert_eq!(unchanged.records, initial.records);
}

#[test]
fn project_memory_feedback_keeps_project_scoped_revision() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_a = [
        ("project_id".to_string(), "project-feedback-a".to_string()),
        ("session_id".to_string(), "session-feedback-a".to_string()),
        ("agent_run_id".to_string(), "run-feedback-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Always keep memory feedback isolated by project",
        project_a.clone(),
    )
    .expect("project A message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_a,
    )
    .expect("project A run should complete");
    let initial = load_project_memory_ledger(&mut store, "project-feedback-a")
        .expect("project A ledger should build");
    let memory_id = initial
        .records
        .iter()
        .find(|record| record.kind == agent_memory::MemoryKind::Requirement)
        .expect("requirement memory should exist")
        .id
        .clone();

    let project_b = [
        ("project_id".to_string(), "project-feedback-b".to_string()),
        ("session_id".to_string(), "session-feedback-b".to_string()),
        ("agent_run_id".to_string(), "run-feedback-b".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Unrelated project B requirement",
        project_b,
    )
    .expect("project B message should append");

    let use_context = [
        ("project_id".to_string(), "project-feedback-a".to_string()),
        ("session_id".to_string(), "session-feedback-a-2".to_string()),
        ("agent_run_id".to_string(), "run-feedback-a-2".to_string()),
        ("memory_ids".to_string(), memory_id.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    assert_eq!(
        record_project_memory_observed_use(
            &mut store,
            &phase16_task_id(),
            &use_context,
            "Kept memory feedback isolated by project.",
        )
        .expect("memory feedback should persist"),
        1
    );

    let persisted = store
        .load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, "project-feedback-a")
        .expect("memory read model should load")
        .expect("memory read model should exist");
    let scoped_revision = store
        .event_revision_by_metadata(&phase16_task_id(), "project_id", "project-feedback-a")
        .expect("project revision should load");
    assert_eq!(persisted.revision, scoped_revision.latest_sequence);
    let updated = load_project_memory_ledger(&mut store, "project-feedback-a")
        .expect("project A ledger should remain incremental");
    assert_eq!(updated.event_count, scoped_revision.event_count);
    assert_eq!(
        updated
            .records
            .iter()
            .find(|record| record.id == memory_id)
            .map(|record| record.observed_use_count),
        Some(1)
    );
}

#[test]
fn project_memory_projection_persists_and_searches_real_lancedb_vectors() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        (
            "project_id".to_string(),
            "project-vector-memory".to_string(),
        ),
        (
            "session_id".to_string(),
            "session-vector-memory".to_string(),
        ),
        ("agent_run_id".to_string(), "run-vector-memory".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Always keep the inspector frosted and translucent",
        context.clone(),
    )
    .expect("user requirement should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("run should complete");
    let ledger = load_project_memory_ledger(&mut store, "project-vector-memory")
        .expect("memory ledger should build");
    let root = std::env::temp_dir().join(format!(
        "cindx-memory-vector-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));

    let fallback = refresh_project_memory_vector_index(&root, &ProviderConfig::default(), &ledger)
        .expect("memory vectors should persist");
    assert!(fallback.is_none());
    let (database_path, manifest_path, current_generation) =
        memory_vector_paths_for_read(&root, "project-vector-memory");
    assert!(lancedb_index_exists(&database_path));
    let results = search_lancedb_index(
        &database_path,
        &local_query_embedding("frosted translucent inspector"),
        4,
    )
    .expect("memory vectors should search");
    assert_eq!(
        results.first().map(|result| result.chunk.id.as_str()),
        Some(ledger.records[0].id.as_str())
    );
    let manifest = load_memory_vector_manifest(&manifest_path)
        .expect("manifest should load")
        .expect("manifest should exist");
    assert_eq!(manifest.embedding_backend, "local");
    assert_eq!(manifest.record_count, ledger.records.len());
    assert_eq!(
        current_generation.as_deref(),
        Some(manifest.generation_id.as_str())
    );
    let unpublished_generation = unique_id("memory-vector-unpublished");
    let (_, unpublished_manifest_path) =
        memory_vector_generation_paths(&root, "project-vector-memory", &unpublished_generation);
    let mut unpublished_manifest = manifest.clone();
    unpublished_manifest.generation_id = unpublished_generation;
    write_private_file_atomically(
        &unpublished_manifest_path,
        &serde_json::to_vec(&unpublished_manifest).expect("manifest should encode"),
        "test unpublished memory vector manifest",
    )
    .expect("unpublished manifest should stage");
    let (still_published_database, _, still_published_generation) =
        memory_vector_paths_for_read(&root, "project-vector-memory");
    assert_eq!(still_published_database, database_path);
    assert_eq!(still_published_generation, current_generation);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn project_memory_never_persists_raw_secrets() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        (
            "project_id".to_string(),
            "project-memory-secret".to_string(),
        ),
        (
            "session_id".to_string(),
            "session-memory-secret".to_string(),
        ),
        ("agent_run_id".to_string(), "run-memory-secret".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Always use sk-1234567890abcdef for this project",
        context.clone(),
    )
    .expect("redacted message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("run should complete");

    let ledger = load_project_memory_ledger(&mut store, "project-memory-secret")
        .expect("memory ledger should build");
    let payload = serde_json::to_string(&ledger).expect("memory ledger should serialize");

    assert!(!payload.contains("sk-1234567890abcdef"));
    assert!(payload.contains("[REDACTED]"));
}

#[test]
fn session_history_page_reports_a_stable_older_cursor() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-history-page";
    for index in 0..7 {
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            &format!("message-{index}"),
            [("session_id".to_string(), session_id.to_string())]
                .into_iter()
                .collect(),
        )
        .expect("message should append");
    }

    let latest = store
        .list_by_task_and_metadata_before(&phase16_task_id(), "session_id", session_id, u64::MAX, 3)
        .expect("latest page should load");
    assert_eq!(
        latest
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![5, 6, 7]
    );

    let older = store
        .list_by_task_and_metadata_before(
            &phase16_task_id(),
            "session_id",
            session_id,
            latest[0].sequence,
            3,
        )
        .expect("older page should load");
    assert_eq!(
        older.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert!(store
        .has_task_metadata_event_before(
            &phase16_task_id(),
            "session_id",
            session_id,
            older[0].sequence,
        )
        .expect("older cursor should be checked"));
}

#[test]
fn routing_telemetry_read_model_deduplicates_completed_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let start_context = [
        ("session_id".to_string(), "session-router".to_string()),
        ("agent_run_id".to_string(), "run-router-1".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let run_context = [
        ("session_id".to_string(), "session-router".to_string()),
        ("agent_run_id".to_string(), "run-router-1".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("agent_model".to_string(), "model-a".to_string()),
        ("routing_signature".to_string(), "coding:3".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        start_context,
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        run_context.clone(),
    )
    .expect("dynamic run decision should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("total_tokens".to_string(), "900".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("model telemetry should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        run_context.clone(),
    )
    .expect("run should complete");

    let initial =
        load_routing_telemetry_read_model(&mut store).expect("routing read model should build");
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].selected_model, "model-a");
    assert_eq!(initial[0].task_class, TaskClass::Coding);
    assert_eq!(
        initial[0].selected_policy,
        OrchestrationPolicy::PlanExecuteReview
    );
    assert_eq!(initial[0].context_signature, "coding:3");
    assert_eq!(initial[0].cost_proxy, 900);

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "unrelated follow-up",
        [("session_id".to_string(), "session-router".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("message should append");
    let updated =
        load_routing_telemetry_read_model(&mut store).expect("routing read model should advance");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].selected_model, "model-a");
}

#[test]
fn read_only_runtime_snapshots_do_not_write_projection_caches() {
    let root = std::env::temp_dir().join(format!(
        "cindx-read-only-snapshot-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(&root).expect("snapshot test directory should exist");
    let database = root.join("state.sqlite3");
    drop(SqliteStore::open(&database).expect("writable store should initialize"));

    let mut store = SqliteStore::open_read_only(&database).expect("read-only store should open");
    assert!(load_routing_telemetry_read_model_snapshot(&mut store)
        .expect("routing snapshot should remain read-only")
        .is_empty());
    assert!(
        load_project_memory_ledger_snapshot(&mut store, "project-read-only")
            .expect("memory snapshot should remain read-only")
            .records
            .is_empty()
    );
    assert!(
        load_agent_session_read_model_snapshot(&store, "session-read-only")
            .expect("session snapshot should remain read-only")
            .state
            .messages
            .is_empty()
    );

    drop(store);
    fs::remove_dir_all(root).expect("snapshot test directory should be removed");
}

#[test]
fn prompt_evolution_read_model_only_indexes_evaluation_evidence() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let genome = ConductorPromptGenome::seed_for_effort("auto");
    let observation = PromptEvolutionObservation {
        profile_id: genome.id.clone(),
        evaluation_id: "pair-1".to_string(),
        case_id: "case-1".to_string(),
        opponent_profile_id: Some("challenger".to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Train,
        mode: PromptEvaluationMode::PairedShadow,
        format_valid: true,
        succeeded: true,
        quality_score: 0.9,
        latency_ms: 800,
        total_tokens: 500,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.2),
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance: test_prompt_evaluation_provenance("candidate", "opponent"),
    };
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor pairwise evaluation",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).expect("genome should serialize"),
            ),
            (
                "prompt_observation".to_string(),
                serde_json::to_string(&observation).expect("observation should serialize"),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("evaluation should append");

    let initial = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should build");
    assert_eq!(initial.genomes.len(), 1);
    assert!(initial.observations.is_empty());

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "ordinary conversation",
        [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("message should append");
    let updated = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should advance");
    assert_eq!(updated.genomes.len(), 1);
    assert!(updated.observations.is_empty());
    assert!(updated.revision > initial.revision);
}

#[test]
fn prompt_evolution_read_model_indexes_dataset_readiness_without_case_content() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor offline dataset selected",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            ("dataset_sha256".to_string(), "dataset-v1".to_string()),
            ("dataset_case_count".to_string(), "2".to_string()),
            ("dataset_train_count".to_string(), "2".to_string()),
            ("dataset_holdout_count".to_string(), "0".to_string()),
            (
                "dataset_status".to_string(),
                "insufficient_cases".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("dataset readiness should append");

    let initial = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should build");
    let key = prompt_dataset_key("auto", "project-a");
    let dataset = initial
        .datasets
        .get(&key)
        .expect("dataset readiness should be indexed");
    assert_eq!(dataset.case_count, 2);
    assert_eq!(dataset.status, "insufficient_cases");
    assert!(dataset.selected_case_id.is_none());

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor offline dataset selected",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            ("dataset_sha256".to_string(), "dataset-v2".to_string()),
            ("dataset_case_count".to_string(), "3".to_string()),
            ("dataset_train_count".to_string(), "2".to_string()),
            ("dataset_holdout_count".to_string(), "1".to_string()),
            ("dataset_status".to_string(), "ready".to_string()),
            ("selected_case_id".to_string(), "case-3".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("updated dataset readiness should append");

    let updated = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should advance");
    let dataset = updated
        .datasets
        .get(&key)
        .expect("latest dataset readiness should replace the prior snapshot");
    assert_eq!(dataset.digest, "dataset-v2");
    assert_eq!(dataset.case_count, 3);
    assert_eq!(dataset.selected_case_id.as_deref(), Some("case-3"));
}

#[test]
fn prompt_evolution_readiness_reports_each_scientific_gate() {
    let incomplete_dataset = PROMPT_EVOLUTION_OFFLINE_MIN_CASES.saturating_sub(1);
    let incomplete_train = PROMPT_EVOLUTION_MIN_TRAIN_RUNS.saturating_sub(1);
    let incomplete_holdout = PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS.saturating_sub(1);

    assert_eq!(
        prompt_evolution_readiness(PromptEvolutionReadinessInput {
            applicable: false,
            enabled: true,
            dataset_cases: 0,
            paired_runs: 0,
            replay_runs: 0,
            ready_profiles: 0,
            evaluation_inflight: false,
            rollout_status: "stable",
        }),
        "not_applicable"
    );
    assert_eq!(
        prompt_evolution_readiness(PromptEvolutionReadinessInput {
            applicable: true,
            enabled: true,
            dataset_cases: incomplete_dataset,
            paired_runs: 0,
            replay_runs: 0,
            ready_profiles: 0,
            evaluation_inflight: false,
            rollout_status: "stable",
        }),
        "collecting_dataset"
    );
    assert_eq!(
        prompt_evolution_readiness(PromptEvolutionReadinessInput {
            applicable: true,
            enabled: true,
            dataset_cases: PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
            paired_runs: incomplete_train,
            replay_runs: 0,
            ready_profiles: 0,
            evaluation_inflight: false,
            rollout_status: "stable",
        }),
        "collecting_train_evidence"
    );
    assert_eq!(
        prompt_evolution_readiness(PromptEvolutionReadinessInput {
            applicable: true,
            enabled: true,
            dataset_cases: PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
            paired_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            replay_runs: incomplete_holdout,
            ready_profiles: 0,
            evaluation_inflight: false,
            rollout_status: "stable",
        }),
        "collecting_holdout_evidence"
    );
    assert_eq!(
        prompt_evolution_readiness(PromptEvolutionReadinessInput {
            applicable: true,
            enabled: true,
            dataset_cases: PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
            paired_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            replay_runs: PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
            ready_profiles: 0,
            evaluation_inflight: false,
            rollout_status: "stable",
        }),
        "selecting_frontier"
    );
    assert_eq!(
        prompt_evolution_readiness(PromptEvolutionReadinessInput {
            applicable: true,
            enabled: true,
            dataset_cases: PROMPT_EVOLUTION_OFFLINE_MIN_CASES,
            paired_runs: PROMPT_EVOLUTION_MIN_TRAIN_RUNS,
            replay_runs: PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS,
            ready_profiles: 1,
            evaluation_inflight: false,
            rollout_status: "canary",
        }),
        "canary"
    );
}

#[test]
fn prompt_evolution_read_model_restores_an_atomic_observation_pair() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let genome_a = ConductorPromptGenome::seed_for_effort("auto");
    let genome_b = ConductorPromptGenome {
        id: "atomic-challenger".to_string(),
        ..genome_a.clone()
    };
    let observation_a = PromptEvolutionObservation {
        profile_id: genome_a.id.clone(),
        evaluation_id: scoped_prompt_evaluation_id("project-a", "atomic-pair"),
        case_id: "atomic-case".to_string(),
        opponent_profile_id: Some(genome_b.id.clone()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Holdout,
        mode: PromptEvaluationMode::ReplayExecution,
        format_valid: true,
        succeeded: true,
        quality_score: 0.8,
        latency_ms: 100,
        total_tokens: 200,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.2),
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance: test_prompt_evaluation_provenance(&genome_a.id, &genome_b.id),
    };
    let mut observation_b = PromptEvolutionObservation {
        profile_id: genome_b.id.clone(),
        opponent_profile_id: Some(genome_a.id.clone()),
        quality_score: 0.6,
        relative_reward: Some(-0.2),
        ..observation_a.clone()
    };
    observation_b.provenance = test_prompt_evaluation_provenance(&genome_b.id, &genome_a.id);
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor pairwise evaluation",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            (
                "prompt_genomes".to_string(),
                serde_json::to_string(&[&genome_a, &genome_b]).expect("genomes should serialize"),
            ),
            (
                "prompt_observations".to_string(),
                serde_json::to_string(&[&observation_a, &observation_b])
                    .expect("observations should serialize"),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("atomic pair should append");

    let model = load_prompt_evolution_read_model(&mut store).expect("atomic pair should rebuild");
    assert_eq!(model.genomes.len(), 2);
    assert_eq!(model.observations.len(), 2);
    assert!(model.observations.iter().all(|(_, observation)| {
        observation.evaluation_id == scoped_prompt_evaluation_id("project-a", "atomic-pair")
            && observation.mode == PromptEvaluationMode::ReplayExecution
    }));
}

#[test]
fn prompt_evolution_evidence_counts_ignore_legacy_plan_only_modes() {
    let profile_id = "seed-auto-v1";
    let observation =
        |evaluation_id: &str, mode: PromptEvaluationMode| PromptEvolutionObservation {
            profile_id: profile_id.to_string(),
            evaluation_id: evaluation_id.to_string(),
            case_id: evaluation_id.to_string(),
            opponent_profile_id: Some("challenger".to_string()),
            task_class: "coding".to_string(),
            split: if mode.is_replay() {
                PromptEvaluationSplit::Holdout
            } else {
                PromptEvaluationSplit::Train
            },
            mode,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.2),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: test_prompt_evaluation_provenance(profile_id, "challenger"),
        };
    let observations = vec![
        observation("legacy-paired", PromptEvaluationMode::PairedShadow),
        observation("legacy-replay", PromptEvaluationMode::ReplayHoldout),
        observation("train", PromptEvaluationMode::PairedExecution),
        observation("train", PromptEvaluationMode::PairedExecution),
        observation("holdout", PromptEvaluationMode::ReplayExecution),
        observation("holdout", PromptEvaluationMode::ReplayExecution),
    ];

    assert_eq!(
        prompt_profile_evidence_counts(&observations, profile_id),
        (1, 1)
    );
    let mut newer_holdout = observation("newer-holdout", PromptEvaluationMode::ReplayExecution);
    newer_holdout.provenance.dataset_sha256 = "holdout-only-dataset".to_string();
    let mut observations_with_newer_holdout = observations;
    observations_with_newer_holdout.push(newer_holdout);
    assert_eq!(
        prompt_profile_training_evidence_count(&observations_with_newer_holdout, profile_id),
        1
    );
}

#[test]
fn prompt_evolution_direct_evidence_does_not_reuse_a_weaker_opponent() {
    let observation = |evaluation_id: &str, opponent: &str| PromptEvolutionObservation {
        profile_id: "candidate".to_string(),
        evaluation_id: evaluation_id.to_string(),
        case_id: evaluation_id.to_string(),
        opponent_profile_id: Some(opponent.to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Holdout,
        mode: PromptEvaluationMode::ReplayExecution,
        format_valid: true,
        succeeded: true,
        quality_score: 0.9,
        latency_ms: 100,
        total_tokens: 100,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.4),
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance: test_prompt_evaluation_provenance("candidate", opponent),
    };
    let observations = vec![
        observation("weak-1", "weak-profile"),
        observation("weak-2", "weak-profile"),
        observation("stable-1", "stable-profile"),
        observation("stable-1", "stable-profile"),
    ];

    assert_eq!(
        prompt_direct_profile_evidence_counts(&observations, "candidate", "stable-profile"),
        (0, 1)
    );
}

#[test]
fn prompt_instance_pareto_seeds_are_stable_across_event_order() {
    let genome = ConductorPromptGenome::seed_for_effort("auto");
    let observation = |evaluation_id: &str, case_id: &str| PromptEvolutionObservation {
        profile_id: genome.id.clone(),
        evaluation_id: evaluation_id.to_string(),
        case_id: case_id.to_string(),
        opponent_profile_id: Some("challenger".to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Train,
        mode: PromptEvaluationMode::PairedExecution,
        format_valid: true,
        succeeded: true,
        quality_score: 0.9,
        latency_ms: 100,
        total_tokens: 100,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.2),
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance: test_prompt_evaluation_provenance(&genome.id, "challenger"),
    };
    let forward = vec![
        observation("train-a", "case-a"),
        observation("train-b", "case-b"),
    ];
    let reversed = forward.iter().rev().cloned().collect::<Vec<_>>();
    let seeds = |observations: &[PromptEvolutionObservation]| {
        prompt_instance_pareto_scores(std::slice::from_ref(&genome), observations)
            .into_iter()
            .map(|score| (score.run_id, score.seed))
            .collect::<BTreeMap<_, _>>()
    };

    assert_eq!(seeds(&forward), seeds(&reversed));
    assert_ne!(seeds(&forward)["train-a"], seeds(&forward)["train-b"]);
}

#[test]
fn prompt_rollout_advances_by_evidence_and_rolls_back_on_regression() {
    let stable = ConductorPromptGenome::seed_for_effort("auto");
    let mut candidate = stable.clone();
    candidate.id = "candidate-auto".to_string();
    candidate.generation = 1;
    let mut model = PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        projection_version: PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
        revision: 0,
        event_count: 0,
        genomes: Vec::new(),
        genome_identity_fingerprints: BTreeMap::new(),
        observations: Vec::new(),
        failure_curricula: Vec::new(),
        attempts: BTreeMap::new(),
        cohorts: BTreeMap::new(),
        cohort_sequences: BTreeMap::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    };
    let evaluation = |comparisons| PromptEvolutionEvaluation {
        population: vec![stable.clone(), candidate.clone()],
        observations: Vec::new(),
        frontier_ids: [candidate.id.clone()].into_iter().collect(),
        champion_id: Some(candidate.id.clone()),
        champion_score: Some(0.8),
        champion_confidence: Some(PromptPromotionConfidence {
            comparisons,
            wins: comparisons,
            losses: 0,
            ties: 0,
            observed_win_rate: 1.0,
            wilson_lower_bound: 0.6,
        }),
        status: "exploring".to_string(),
        freeze_reason: None,
        stagnant_generations: 0,
        evaluated_generations: 1,
        next_mode: "explore".to_string(),
        next_profile: candidate.clone(),
        mutation_parent: None,
        mutation_trajectories: Vec::new(),
    };

    let direct_observation =
        |index: usize, split: PromptEvaluationSplit| PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: format!("direct-stable-{index}"),
            case_id: format!("direct-case-{index}"),
            opponent_profile_id: Some(stable.id.clone()),
            task_class: if index.is_multiple_of(2) {
                "coding"
            } else {
                "research"
            }
            .to_string(),
            split,
            mode: match split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
            },
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.4),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: test_prompt_evaluation_provenance(&candidate.id, &stable.id),
        };
    for index in 0..6 {
        let candidate_observation = direct_observation(index, PromptEvaluationSplit::Train);
        let mut stable_observation = candidate_observation.clone();
        stable_observation.profile_id = stable.id.clone();
        stable_observation.opponent_profile_id = Some(candidate.id.clone());
        stable_observation.relative_reward = Some(-0.4);
        stable_observation.provenance =
            test_prompt_evaluation_provenance(&stable.id, &candidate.id);
        model
            .observations
            .push(("auto".to_string(), candidate_observation));
        model
            .observations
            .push(("auto".to_string(), stable_observation));
    }
    for index in 0..8 {
        let candidate_observation = direct_observation(index + 6, PromptEvaluationSplit::Holdout);
        let mut stable_observation = candidate_observation.clone();
        stable_observation.profile_id = stable.id.clone();
        stable_observation.opponent_profile_id = Some(candidate.id.clone());
        stable_observation.relative_reward = Some(-0.4);
        stable_observation.provenance =
            test_prompt_evaluation_provenance(&stable.id, &candidate.id);
        model
            .observations
            .push(("auto".to_string(), candidate_observation));
        model
            .observations
            .push(("auto".to_string(), stable_observation));
    }

    bind_matched_prompt_evidence(&mut model, "auto", &candidate.id, &stable.id);
    let started = reconcile_prompt_rollout(&mut model, "auto", &evaluation(8));
    assert_eq!(started.canary_profile_id.as_deref(), Some("candidate-auto"));
    assert_eq!(started.canary_percent, 10);

    let active_lineage = crate::prompt_profile_serving::PromptProfileDeploymentLineage {
        scope_sha256: "f".repeat(64),
        source_revision: 2,
        deployment_generation: 2,
    };

    model.observations.push((
        "auto".to_string(),
        PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: "live-canary-1".to_string(),
            case_id: "live-canary-1".to_string(),
            opponent_profile_id: None,
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::Live,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: None,
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: test_prompt_live_assignment_provenance(&candidate),
        },
    ));
    let mut current_generation_live = model.observations.last().unwrap().1.clone();
    current_generation_live.evaluation_id = "live-canary-current-generation".to_string();
    current_generation_live.case_id = "live-canary-current-generation".to_string();
    current_generation_live.provenance = test_prompt_live_assignment_provenance_for_lineage(
        &candidate,
        active_lineage.scope_sha256.clone(),
        active_lineage.source_revision,
        active_lineage.deployment_generation,
    );
    for index in 0..2 {
        let candidate_observation = direct_observation(index + 14, PromptEvaluationSplit::Holdout);
        let mut stable_observation = candidate_observation.clone();
        stable_observation.profile_id = stable.id.clone();
        stable_observation.opponent_profile_id = Some(candidate.id.clone());
        stable_observation.relative_reward = Some(-0.4);
        stable_observation.provenance =
            test_prompt_evaluation_provenance(&stable.id, &candidate.id);
        model
            .observations
            .push(("auto".to_string(), candidate_observation));
        model
            .observations
            .push(("auto".to_string(), stable_observation));
    }
    bind_matched_prompt_evidence(&mut model, "auto", &candidate.id, &stable.id);
    let delayed_old_generation = reconcile_prompt_rollout_with_lineage(
        &mut model,
        "auto",
        &evaluation(10),
        Some(&active_lineage),
    );
    assert_eq!(delayed_old_generation.canary_percent, 10);
    model
        .observations
        .push(("auto".to_string(), current_generation_live));
    let advanced = reconcile_prompt_rollout_with_lineage(
        &mut model,
        "auto",
        &evaluation(10),
        Some(&active_lineage),
    );
    assert_eq!(advanced.canary_percent, 25);

    model.observations.push((
        "auto".to_string(),
        PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: "live-canary-unsafe".to_string(),
            case_id: "live-canary-unsafe".to_string(),
            opponent_profile_id: None,
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::Live,
            format_valid: true,
            succeeded: false,
            quality_score: 0.0,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 1,
            relative_reward: None,
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: test_prompt_live_assignment_provenance_for_lineage(
                &candidate,
                active_lineage.scope_sha256.clone(),
                active_lineage.source_revision,
                active_lineage.deployment_generation,
            ),
        },
    ));
    let rolled_back = reconcile_prompt_rollout_with_lineage(
        &mut model,
        "auto",
        &evaluation(12),
        Some(&active_lineage),
    );
    assert_eq!(rolled_back.status, "rolled_back");
    assert!(rolled_back.canary_profile_id.is_none());
    assert_eq!(rolled_back.rollback_count, 1);
    assert_eq!(rolled_back.stable_profile_id, stable.id);
}

#[test]
fn completed_gepa_canary_persists_a_verified_frozen_profile() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let stable = seed
        .mutations()
        .into_iter()
        .next()
        .expect("seed should have an evolved stable profile");
    let candidate = stable
        .mutations()
        .into_iter()
        .next()
        .expect("stable profile should have an evolved candidate");
    let mut rollout = crate::prompt_canary_runtime::default_prompt_rollout("auto");
    rollout.stable_profile_id = stable.id.clone();
    rollout.frozen_profile = Some(
        FrozenPromptProfileSnapshot::new_gepa(
            "auto",
            stable.clone(),
            seed.id,
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("existing stable profile should have a valid frozen snapshot"),
    );
    rollout.canary_profile_id = Some(candidate.id.clone());
    rollout.canary_percent = 50;
    rollout.status = "canary".to_string();
    let mut model = PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        projection_version: PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
        revision: 0,
        event_count: 0,
        genomes: vec![PromptGenomeRecord {
            scope: "global".to_string(),
            effort: "auto".to_string(),
            genome: candidate.clone(),
            evolution_method: Some(PromptEvolutionMethod::GepaReflectivePaired),
        }],
        genome_identity_fingerprints: BTreeMap::new(),
        observations: Vec::new(),
        failure_curricula: Vec::new(),
        attempts: BTreeMap::new(),
        cohorts: BTreeMap::new(),
        cohort_sequences: BTreeMap::new(),
        rollouts: BTreeMap::from([("auto".to_string(), rollout)]),
        datasets: BTreeMap::new(),
    };
    let candidate_prompt_sha256 =
        sha256_hex(&serde_json::to_vec(&candidate).expect("candidate should serialize"));
    let stable_prompt_sha256 =
        sha256_hex(&serde_json::to_vec(&stable).expect("stable profile should serialize"));
    for index in 0..14 {
        let split = if index < 6 {
            PromptEvaluationSplit::Train
        } else {
            PromptEvaluationSplit::Holdout
        };
        let mode = match split {
            PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
            PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
        };
        let task_class = if index % 2 == 0 { "coding" } else { "research" };
        let mut candidate_observation = PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: format!("promotion-{index}"),
            case_id: format!("case-{index}"),
            opponent_profile_id: Some(stable.id.clone()),
            task_class: task_class.to_string(),
            split,
            mode,
            format_valid: true,
            succeeded: true,
            quality_score: 0.95,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.5),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: test_prompt_evaluation_provenance(&candidate.id, &stable.id),
        };
        candidate_observation.provenance.candidate_prompt_sha256 = candidate_prompt_sha256.clone();
        candidate_observation.provenance.opponent_prompt_sha256 = stable_prompt_sha256.clone();
        let mut stable_observation = candidate_observation.clone();
        stable_observation.profile_id = stable.id.clone();
        stable_observation.opponent_profile_id = Some(candidate.id.clone());
        stable_observation.quality_score = 0.45;
        stable_observation.relative_reward = Some(-0.5);
        stable_observation.provenance =
            test_prompt_evaluation_provenance(&stable.id, &candidate.id);
        stable_observation.provenance.candidate_prompt_sha256 = stable_prompt_sha256.clone();
        stable_observation.provenance.opponent_prompt_sha256 = candidate_prompt_sha256.clone();
        model
            .observations
            .push(("auto".to_string(), candidate_observation));
        model
            .observations
            .push(("auto".to_string(), stable_observation));
    }
    model.observations.push((
        "auto".to_string(),
        PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: "promotion-live".to_string(),
            case_id: "promotion-live".to_string(),
            opponent_profile_id: None,
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::Live,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: None,
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: test_prompt_live_assignment_provenance(&candidate),
        },
    ));
    let evaluation = PromptEvolutionEvaluation {
        population: vec![stable.clone(), candidate.clone()],
        observations: Vec::new(),
        frontier_ids: [candidate.id.clone()].into_iter().collect(),
        champion_id: Some(candidate.id.clone()),
        champion_score: Some(0.9),
        champion_confidence: None,
        status: "exploring".to_string(),
        freeze_reason: None,
        stagnant_generations: 0,
        evaluated_generations: 1,
        next_mode: "explore".to_string(),
        next_profile: candidate.clone(),
        mutation_parent: None,
        mutation_trajectories: Vec::new(),
    };

    bind_matched_prompt_evidence(&mut model, "auto", &candidate.id, &stable.id);
    assert!(prompt_rollout_transition_has_canonical_evidence(
        &model,
        "auto",
        None,
        &model.rollouts["auto"],
    ));

    let previous_frozen = model.rollouts["auto"].frozen_profile.clone();
    let mut missing_model = model.clone();
    missing_model.genomes.clear();
    let missing_profile = reconcile_prompt_rollout(&mut missing_model, "auto", &evaluation);
    assert_eq!(missing_profile.status, "rolled_back");
    assert!(missing_profile.canary_profile_id.is_none());
    assert_eq!(missing_profile.canary_percent, 0);
    assert_eq!(missing_profile.rollback_count, 1);
    assert_eq!(missing_profile.stable_profile_id, stable.id);
    assert_eq!(missing_profile.frozen_profile, previous_frozen);
    assert!(missing_profile
        .last_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("freeze_failed:")));

    let mut non_gepa_model = model.clone();
    non_gepa_model.genomes[0].evolution_method = None;
    let non_gepa_profile = reconcile_prompt_rollout(&mut non_gepa_model, "auto", &evaluation);
    assert_eq!(non_gepa_profile.status, "rolled_back");
    assert!(non_gepa_profile.canary_profile_id.is_none());
    assert_eq!(non_gepa_profile.canary_percent, 0);
    assert_eq!(non_gepa_profile.rollback_count, 1);
    assert_eq!(non_gepa_profile.stable_profile_id, stable.id);
    assert_eq!(non_gepa_profile.frozen_profile, previous_frozen);
    assert!(non_gepa_profile
        .last_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("freeze_failed:")));

    let promoted = reconcile_prompt_rollout(&mut model, "auto", &evaluation);

    assert_eq!(promoted.status, "promoted");
    assert_eq!(promoted.stable_profile_id, candidate.id);
    assert!(promoted.canary_profile_id.is_none());
    assert_eq!(promoted.canary_percent, 0);
    let snapshot = promoted
        .frozen_profile
        .expect("GEPA promotion should freeze its evidence-bound profile");
    snapshot.validate().expect("snapshot should remain valid");
    assert_eq!(snapshot.stable_profile_id, stable.id);
    assert_eq!(
        snapshot.evolution_method,
        PromptEvolutionMethod::GepaReflectivePaired
    );
}

#[test]
fn prompt_rollout_rebuild_rejects_canary_without_canonical_evidence() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let candidate = seed
        .mutations()
        .into_iter()
        .next()
        .expect("Auto seed should provide a canary");
    let event = Event {
        id: EventId("rollout-update".to_string()),
        task_id: phase16_task_id(),
        sequence: 7,
        timestamp_ms: 70,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor prompt rollout updated".to_string(),
        metadata: [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("stable_profile".to_string(), seed.id.clone()),
            ("canary_profile".to_string(), candidate.id.clone()),
            ("canary_percent".to_string(), "25".to_string()),
            ("rollout_status".to_string(), "canary".to_string()),
            (
                "rollout_reason".to_string(),
                "canary_stage_advanced".to_string(),
            ),
            ("promotion_confidence".to_string(), "0.61".to_string()),
            ("evidence_checkpoint".to_string(), "8".to_string()),
            ("live_checkpoint".to_string(), "3".to_string()),
            ("rollback_count".to_string(), "0".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    let model = build_prompt_evolution_read_model(&[event], 7, 1);
    assert!(model.rollouts.is_empty());
}

#[test]
fn promoted_prompt_rollout_without_a_valid_frozen_profile_is_ignored() {
    let event = Event {
        id: EventId("invalid-promotion".to_string()),
        task_id: phase16_task_id(),
        sequence: 8,
        timestamp_ms: 80,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor prompt rollout updated".to_string(),
        metadata: [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("stable_profile".to_string(), "learned-auto".to_string()),
            ("rollout_status".to_string(), "promoted".to_string()),
            (
                "frozen_prompt_profile".to_string(),
                "{\"schema\":\"tampered\"}".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let model = build_prompt_evolution_read_model(&[event], 8, 1);

    assert!(model.rollouts.is_empty());
}

#[test]
fn stable_prompt_rollout_uses_the_evidence_bound_frozen_genome() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let certified = seed
        .mutations()
        .into_iter()
        .next()
        .expect("seed should have an evolved candidate");
    let snapshot = FrozenPromptProfileSnapshot::new_gepa(
        "auto",
        certified.clone(),
        seed.id,
        "a".repeat(64),
        "b".repeat(64),
    )
    .expect("frozen profile should validate");
    let mut mutable_copy = certified.clone();
    mutable_copy.custom_directive = "changed after certification".to_string();
    let model = PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        projection_version: PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
        revision: 0,
        event_count: 0,
        genomes: vec![PromptGenomeRecord {
            scope: "global".to_string(),
            effort: "auto".to_string(),
            genome: mutable_copy,
            evolution_method: Some(PromptEvolutionMethod::GepaReflectivePaired),
        }],
        genome_identity_fingerprints: BTreeMap::new(),
        observations: Vec::new(),
        failure_curricula: Vec::new(),
        attempts: BTreeMap::new(),
        cohorts: BTreeMap::new(),
        cohort_sequences: BTreeMap::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    };
    let mut rollout = PromptRolloutState {
        stable_profile_id: certified.id.clone(),
        canary_profile_id: None,
        canary_percent: 0,
        evidence_checkpoint: 0,
        live_checkpoint: 0,
        stable_live_checkpoint: 0,
        quarantined_profile_ids: Vec::new(),
        distillation_lease: None,
        rollback_count: 0,
        status: "stable".to_string(),
        last_reason: None,
        promotion_confidence: Some(0.8),
        frozen_profile: Some(snapshot),
    };
    let mut evaluation = PromptEvolutionEvaluation {
        population: Vec::new(),
        observations: Vec::new(),
        frontier_ids: BTreeSet::new(),
        champion_id: None,
        champion_score: None,
        champion_confidence: None,
        status: "stable".to_string(),
        freeze_reason: None,
        stagnant_generations: 0,
        evaluated_generations: 0,
        next_mode: "stable".to_string(),
        next_profile: ConductorPromptGenome::seed_for_effort("auto"),
        mutation_parent: None,
        mutation_trajectories: Vec::new(),
    };

    apply_prompt_rollout_selection(&mut evaluation, &rollout, &model, &Metadata::new(), "auto");

    assert_eq!(evaluation.next_profile, certified);

    let available_canary = certified
        .mutations()
        .into_iter()
        .next()
        .expect("stable profile should have an evolved canary");
    rollout.canary_profile_id = Some(available_canary.id.clone());
    rollout.canary_percent = 50;
    evaluation.population = vec![available_canary];
    evaluation.next_profile = ConductorPromptGenome::seed_for_effort("auto");
    apply_prompt_rollout_selection(&mut evaluation, &rollout, &model, &Metadata::new(), "auto");

    assert_eq!(evaluation.next_profile, certified);
    assert_eq!(evaluation.next_mode, "stable");

    rollout.canary_profile_id = Some("missing-canary".to_string());
    rollout.canary_percent = 50;
    evaluation.population.clear();
    evaluation.next_profile = ConductorPromptGenome::seed_for_effort("auto");
    apply_prompt_rollout_selection(&mut evaluation, &rollout, &model, &Metadata::new(), "auto");

    assert_eq!(evaluation.next_profile, certified);
    assert_eq!(evaluation.next_mode, "stable");
}

#[test]
fn ordinary_prompt_candidate_ignores_distillation_quarantine_capacity() {
    let mut rollout = crate::prompt_canary_runtime::default_prompt_rollout("auto");
    rollout.quarantined_profile_ids = (0..PROMPT_ROLLOUT_MAX_QUARANTINED_PROFILES)
        .map(|index| format!("distilled-{index}"))
        .collect();

    assert!(!prompt_candidate_blocked_by_distillation_quarantine(
        &rollout,
        "ordinary-gepa-candidate",
        false,
    ));
    assert!(prompt_candidate_blocked_by_distillation_quarantine(
        &rollout,
        "another-distilled-candidate",
        true,
    ));
}

#[test]
fn pending_review_state_identifies_the_related_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_sessions = ProjectSessionConfig::default_for_root(&workspace_root());
    let session = project_sessions
        .active_session()
        .expect("default session should exist");
    let project = project_sessions
        .active_project()
        .expect("default project should exist");
    let request = PermissionRequest {
        id: PermissionRequestId("agent-review-1".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "Run project checks.".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("phase".to_string(), "16".to_string()),
            ("session_id".to_string(), session.id.clone()),
            ("session_name".to_string(), session.name.clone()),
            ("project_id".to_string(), project.id.clone()),
            ("project_name".to_string(), project.name.clone()),
            ("tool_input".to_string(), "command=cargo test".to_string()),
            ("session_reusable".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    store
        .save_permission_request(request, current_time_millis())
        .expect("request should save");

    let state =
        permission_review_state(&store, &project_sessions).expect("review state should load");

    assert_eq!(state.pending.len(), 1);
    assert_eq!(state.pending[0].source, "agent");
    assert_eq!(
        state.pending[0].session_id.as_deref(),
        Some(session.id.as_str())
    );
    assert_eq!(
        state.pending[0].session_name.as_deref(),
        Some(session.name.as_str())
    );
    assert!(state.pending[0].can_allow_session);
    assert!(state.pending[0].input.contains("cargo test"));
}

#[test]
fn phase4_state_includes_provider_config_and_messages() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let config = ProviderConfig {
        api_key: "secret".to_string(),
        ..ProviderConfig::default()
    };
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::User,
        "hello model",
    )
    .expect("message should append");

    let state = phase4_state(&mut store, &config, None).expect("state should load");

    assert!(state.provider.ready);
    assert!(state.provider.api_key_set);
    assert!(!provider_config_state(&ProviderConfig::default()).ready);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].content, "hello model");
}

#[test]
fn redacts_sensitive_values_in_new_and_existing_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::Tool,
        "api_key=secret-value\nBearer token-value\nsk-1234567890abcdef",
    )
    .expect("message should append");
    store
        .append(Event {
            id: EventId("legacy-secret".to_string()),
            task_id: phase4_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::ToolCallFinished,
            summary: "legacy secret".to_string(),
            metadata: [("output".to_string(), "password=old-secret".to_string())]
                .into_iter()
                .collect(),
        })
        .expect("legacy event should append");

    let updated = redact_persisted_events(&mut store).expect("history should redact");
    let events = store
        .list_by_task(&phase4_task_id())
        .expect("events should load");
    let rendered = events
        .iter()
        .flat_map(|event| event.metadata.values())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(updated, 1);
    assert!(!rendered.contains("secret-value"));
    assert!(!rendered.contains("token-value"));
    assert!(!rendered.contains("1234567890abcdef"));
    assert!(!rendered.contains("old-secret"));
    assert!(rendered.contains("[REDACTED]"));
}

#[test]
fn redacting_tool_call_metadata_preserves_nested_json() {
    let arguments = serde_json::json!({
        "command": "cat provider.conf | sed -E 's/(key|token|secret|api[_-]?key)=.*/[REDACTED]/g'",
        "api_key": "secret-value"
    })
    .to_string();
    let raw_tool_calls = serde_json::json!([{
        "id": "call_1",
        "type": "function",
        "function": {
            "name": "shell_run",
            "arguments": arguments
        }
    }])
    .to_string();
    let metadata = [("raw_tool_calls_json".to_string(), raw_tool_calls)]
        .into_iter()
        .collect();

    let redacted = redact_metadata(&metadata);
    let parsed: serde_json::Value = serde_json::from_str(
        redacted
            .get("raw_tool_calls_json")
            .expect("tool calls should remain present"),
    )
    .expect("tool calls should remain valid JSON");
    let nested: serde_json::Value = serde_json::from_str(
        parsed[0]["function"]["arguments"]
            .as_str()
            .expect("arguments should remain a JSON string"),
    )
    .expect("tool arguments should remain valid JSON");

    assert_eq!(nested["api_key"], "[REDACTED]");
    assert!(nested["command"].as_str().is_some());
    assert!(!redacted["raw_tool_calls_json"].contains("secret-value"));
}

#[test]
fn generated_project_ids_are_not_mistaken_for_prefixed_secrets() {
    for name in ["SK Model", "AKIA Research"] {
        let project_id = new_project_id(name);
        let metadata = [
            ("project_id".to_string(), project_id.clone()),
            (
                "content".to_string(),
                "token=sk-abcdefghijklmnop".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let redacted = redact_metadata(&metadata);
        assert_eq!(redacted.get("project_id"), Some(&project_id));
        assert_eq!(
            redacted.get("content").map(String::as_str),
            Some("token=[REDACTED]")
        );
    }
}

#[test]
fn event_redaction_marker_records_completed_migration() {
    let root = temp_test_root("cindx-event-redaction-marker");
    assert!(!event_redaction_complete(&root));

    mark_event_redaction_complete(&root).expect("marker should persist");

    assert!(event_redaction_complete(&root));
    assert_eq!(
        fs::read_to_string(event_redaction_marker_path(&root)).expect("marker should be readable"),
        "events-redaction-v1\n"
    );
    fs::remove_dir_all(root).expect("marker fixture should be removed");
}

#[test]
fn tool_event_metadata_is_compacted_before_persistence() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-large",
        "browser.capture",
        "completed",
        &"output".repeat(20_000),
        [("structured_output".to_string(), "structured".repeat(20_000))]
            .into_iter()
            .collect(),
        None,
    )
    .expect("tool event should append");

    let event = store
        .list_by_task(&phase16_task_id())
        .expect("events should load")
        .pop()
        .expect("tool event should exist");
    assert!(event.metadata["output"].len() <= PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES + 3);
    assert_eq!(event.metadata["output_omitted"], "true");
    assert_eq!(event.metadata["result_structured_output_omitted"], "true");
    assert!(event.metadata["result_structured_output"].starts_with("[omitted:"));
}

#[test]
fn persisted_tool_event_compaction_rewrites_legacy_payloads() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    store
        .append(Event {
            id: EventId("legacy-large-tool-event".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::ToolCallFinished,
            summary: "legacy tool output".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("output".to_string(), "legacy-output".repeat(30_000)),
            ]
            .into_iter()
            .collect(),
        })
        .expect("legacy event should append");

    assert_eq!(
        compact_persisted_tool_event_metadata(&mut store).expect("legacy metadata should compact"),
        1
    );
    let event = store
        .event_by_id("legacy-large-tool-event")
        .expect("event should load")
        .expect("event should exist");
    assert_eq!(event.metadata["session_id"], "session-a");
    assert_eq!(event.metadata["output_omitted"], "true");
    assert!(event.metadata["output"].len() <= PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES + 3);
}

#[test]
fn provider_config_input_preserves_existing_key_when_blank() {
    let mut config = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://example.test/v1".to_string(),
        api_key: "existing".to_string(),
        image_endpoint: "https://images.example.test/v1".to_string(),
        ..ProviderConfig::default()
    };

    apply_provider_config_input(
        &mut config,
        ProviderConfigInput {
            provider_id: "custom".to_string(),
            provider_resource: "".to_string(),
            base_url: "https://example.test/v1".to_string(),
            api_key: "".to_string(),
            model: "model-a".to_string(),
            conductor_model: "".to_string(),
            planner_model: "".to_string(),
            executor_model: "".to_string(),
            reviewer_model: "".to_string(),
            summarizer_model: "".to_string(),
            embedding_model: "".to_string(),
            image_model: "image-model-a".to_string(),
            image_endpoint: "https://images.example.test/v1".to_string(),
            voice_model: "gpt-realtime".to_string(),
            collaboration_policy: "auto_router".to_string(),
            prompt_evolution_enabled: true,
            context_window_tokens: 128_000,
            agent_system_prompt: "Be concise.\nUse Chinese when asked.".to_string(),
        },
    );

    assert_eq!(config.api_key, "existing");
    assert_eq!(config.model_for_conductor(), "model-a");
    assert_eq!(config.executor_model, "model-a");
    assert_eq!(config.collaboration_policy, "auto_router");
    assert_eq!(config.context_window_tokens, 128_000);
    assert_eq!(
        config.agent_system_prompt,
        "Be concise.\nUse Chinese when asked."
    );
    assert_eq!(config.image_model, "image-model-a");
    assert_eq!(config.image_endpoint, "https://images.example.test/v1");
    assert_eq!(config.voice_model, "gpt-realtime");
}

fn provider_input_from_config(config: &ProviderConfig) -> ProviderConfigInput {
    ProviderConfigInput {
        provider_id: config.provider_id.clone(),
        provider_resource: config.provider_resource.clone(),
        base_url: config.base_url.clone(),
        api_key: String::new(),
        model: config.model.clone(),
        conductor_model: config.conductor_model.clone(),
        planner_model: config.planner_model.clone(),
        executor_model: config.executor_model.clone(),
        reviewer_model: config.reviewer_model.clone(),
        summarizer_model: config.summarizer_model.clone(),
        embedding_model: config.embedding_model.clone(),
        image_model: config.image_model.clone(),
        image_endpoint: config.image_endpoint.clone(),
        voice_model: config.voice_model.clone(),
        collaboration_policy: config.collaboration_policy.clone(),
        prompt_evolution_enabled: config.prompt_evolution_enabled,
        context_window_tokens: config.context_window_tokens,
        agent_system_prompt: config.agent_system_prompt.clone(),
    }
}

#[test]
fn provider_profiles_resolve_fixed_and_resource_scoped_endpoints() {
    let openai = resolve_provider_profile(
        PROVIDER_OPENAI,
        "ignored",
        "https://ignored.example/v1",
        "https://ignored.example/images",
    );
    assert_eq!(openai.provider_id, PROVIDER_OPENAI);
    assert!(openai.provider_resource.is_empty());
    assert_eq!(openai.base_url, "https://api.openai.com/v1");
    assert!(openai.image_endpoint.is_empty());

    let azure = resolve_provider_profile(
        PROVIDER_AZURE_OPENAI,
        "Team-East",
        "",
        "https://ignored.example/images",
    );
    assert_eq!(azure.provider_resource, "team-east");
    assert_eq!(
        azure.base_url,
        "https://team-east.openai.azure.com/openai/v1"
    );
    assert!(azure.image_endpoint.is_empty());

    let azure_services = resolve_provider_profile(
        PROVIDER_AZURE_OPENAI,
        "Team-East",
        "https://team-east.services.ai.azure.com/openai/v1",
        "",
    );
    assert_eq!(
        azure_services.base_url,
        "https://team-east.services.ai.azure.com/openai/v1"
    );

    let alibaba_shared = resolve_provider_profile(
        PROVIDER_ALIBABA_CN,
        "",
        "https://old-workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        "",
    );
    assert_eq!(
        alibaba_shared.base_url,
        "https://dashscope.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(
        alibaba_shared.image_endpoint,
        "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
    );

    let alibaba_workspace = resolve_provider_profile(PROVIDER_ALIBABA_CN, "WS-123", "", "");
    assert!(alibaba_workspace.provider_resource.is_empty());
    assert_eq!(
        alibaba_workspace.base_url,
        "https://dashscope.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(
        alibaba_workspace.image_endpoint,
        "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
    );

    let max_resource = "a".repeat(63);
    let valid_boundary = resolve_provider_profile(PROVIDER_AZURE_OPENAI, &max_resource, "", "");
    assert_eq!(
        valid_boundary.base_url,
        format!("https://{max_resource}.openai.azure.com/openai/v1")
    );
    let invalid_boundary = resolve_provider_profile(PROVIDER_AZURE_OPENAI, &"a".repeat(64), "", "");
    assert!(invalid_boundary.base_url.is_empty());
}

#[test]
fn legacy_provider_inference_uses_exact_hosts_and_preserves_custom_urls() {
    let openai = provider_config_from_text("base_url=https://api.openai.com/v1\n");
    assert_eq!(openai.provider_id, PROVIDER_OPENAI);
    assert_eq!(openai.base_url, "https://api.openai.com/v1");

    let azure = provider_config_from_text(
        "base_url=https://legacy-east.openai.azure.com/openai/v1\napi_key=secret\n",
    );
    assert_eq!(azure.provider_id, PROVIDER_AZURE_OPENAI);
    assert_eq!(azure.provider_resource, "legacy-east");
    assert_eq!(
        azure.base_url,
        "https://legacy-east.openai.azure.com/openai/v1"
    );
    assert!(!azure.supports_webrtc_voice());

    let alibaba = provider_config_from_text(
        "base_url=https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1\n",
    );
    assert_eq!(alibaba.provider_id, PROVIDER_CUSTOM);
    assert!(alibaba.provider_resource.is_empty());
    assert_eq!(
        alibaba.base_url,
        "https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );
    assert!(!alibaba.supports_webrtc_voice());

    let explicit_legacy_workspace = provider_config_from_text(
        "provider_id=alibaba_cn\n\
         provider_resource=workspace-id\n\
         base_url=https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1\n",
    );
    assert_eq!(explicit_legacy_workspace.provider_id, PROVIDER_CUSTOM);
    assert!(explicit_legacy_workspace.provider_resource.is_empty());
    assert_eq!(
        explicit_legacy_workspace.base_url,
        "https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );

    let alibaba_shared =
        provider_config_from_text("base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n");
    assert_eq!(alibaba_shared.provider_id, PROVIDER_ALIBABA_CN);
    assert!(alibaba_shared.provider_resource.is_empty());

    let malicious = "https://api.openai.com.evil.test/custom/path?mode=1";
    let custom_image = "https://images.evil.test/private/generate";
    let custom = provider_config_from_text(&format!(
        "base_url={malicious}\nimage_endpoint={custom_image}\n"
    ));
    assert_eq!(custom.provider_id, PROVIDER_CUSTOM);
    assert_eq!(custom.base_url, malicious);
    assert_eq!(custom.image_endpoint, custom_image);
    assert!(custom.supports_webrtc_voice());
}

#[test]
fn legacy_provider_inference_preserves_independent_image_overrides() {
    let custom_image = "https://images.example.test/private/generate";
    let migrated = provider_config_from_text(&format!(
        "base_url=https://api.openai.com/v1\nimage_endpoint={custom_image}\n"
    ));
    assert_eq!(migrated.provider_id, PROVIDER_CUSTOM);
    assert_eq!(migrated.base_url, "https://api.openai.com/v1");
    assert_eq!(migrated.image_endpoint, custom_image);

    let automatic = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
         image_endpoint=https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation\n",
    );
    assert_eq!(automatic.provider_id, PROVIDER_ALIBABA_CN);
}

#[test]
fn blank_api_keys_are_reused_only_for_the_same_provider_identity() {
    let mut config = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://gateway.example/v1".to_string(),
        api_key: "existing".to_string(),
        ..ProviderConfig::default()
    };
    let mut same_origin = provider_input_from_config(&config);
    same_origin.base_url = "https://gateway.example/openai/v1".to_string();
    apply_provider_config_input(&mut config, same_origin);
    assert_eq!(config.api_key, "existing");

    let mut other_origin = provider_input_from_config(&config);
    other_origin.base_url = "https://other.example/v1".to_string();
    apply_provider_config_input(&mut config, other_origin);
    assert!(config.api_key.is_empty());

    config.provider_id = PROVIDER_AZURE_OPENAI.to_string();
    config.provider_resource = "resource-a".to_string();
    config.base_url = "https://resource-a.openai.azure.com/openai/v1".to_string();
    config.api_key = "azure-key".to_string();
    let mut other_resource = provider_input_from_config(&config);
    other_resource.provider_resource = "resource-b".to_string();
    apply_provider_config_input(&mut config, other_resource);
    assert!(config.api_key.is_empty());

    let mut openai = ProviderConfig {
        api_key: "openai-key".to_string(),
        ..ProviderConfig::default()
    };
    let mut alibaba = provider_input_from_config(&openai);
    alibaba.provider_id = PROVIDER_ALIBABA_CN.to_string();
    alibaba.base_url.clear();
    apply_provider_config_input(&mut openai, alibaba);
    assert_eq!(openai.provider_id, PROVIDER_ALIBABA_CN);
    assert!(openai.api_key.is_empty());
}

#[test]
fn model_list_reuses_saved_key_only_for_the_same_draft_identity() {
    let saved = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://gateway.example/v1".to_string(),
        api_key: "saved-key".to_string(),
        ..ProviderConfig::default()
    };
    let same_origin = provider_config_for_model_list(
        &saved,
        &ProviderModelsInput {
            provider_id: PROVIDER_CUSTOM.to_string(),
            provider_resource: String::new(),
            base_url: "https://gateway.example/openai/v1".to_string(),
            api_key: String::new(),
        },
    );
    assert_eq!(same_origin.api_key, "saved-key");

    let other_origin = provider_config_for_model_list(
        &saved,
        &ProviderModelsInput {
            provider_id: PROVIDER_CUSTOM.to_string(),
            provider_resource: String::new(),
            base_url: "https://other.example/v1".to_string(),
            api_key: String::new(),
        },
    );
    assert!(other_origin.api_key.is_empty());

    let malicious = provider_config_for_model_list(
        &ProviderConfig {
            api_key: "openai-key".to_string(),
            ..ProviderConfig::default()
        },
        &ProviderModelsInput {
            provider_id: String::new(),
            provider_resource: String::new(),
            base_url: "https://api.openai.com.evil.test/v1".to_string(),
            api_key: String::new(),
        },
    );
    assert_eq!(malicious.base_url, "https://api.openai.com.evil.test/v1");
    assert!(malicious.api_key.is_empty());
}

#[test]
fn openai_voice_model_is_built_in_and_can_still_be_overridden() {
    let mut config = provider_config_from_text(
        "base_url=https://api.openai.com/v1\napi_key=secret\nexecutor_model=model-a\n",
    );
    assert!(config.is_ready());
    assert!(config.voice_is_ready());
    assert_eq!(config.voice_model, "gpt-realtime-2.1");

    config = provider_config_from_text(
        "base_url=https://api.openai.com/v1\napi_key=secret\nexecutor_model=model-a\nvoice_model=gpt-realtime\n",
    );
    assert!(config.is_ready());
    assert!(config.voice_is_ready());
    assert_eq!(config.voice_model, "gpt-realtime");

    config.provider_id = PROVIDER_ALIBABA_CN.to_string();
    config.base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string();
    config.voice_model = "qwen3.5-omni-flash-realtime".to_string();
    assert!(config.voice_is_ready());
    assert_eq!(
        config.voice_transport(),
        ProviderVoiceTransport::DashScopeWebSocket
    );
}

#[test]
fn custom_provider_voice_gating_uses_exact_known_hosts() {
    for base_url in [
        "https://team.openai.azure.com/openai/v1",
        "https://team.services.ai.azure.com/openai/v1",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
    ] {
        let config = provider_config_from_text(&format!(
            "provider_id=custom\nbase_url={base_url}\nvoice_model=realtime-model\n"
        ));
        assert_eq!(config.provider_id, PROVIDER_CUSTOM);
        assert!(!config.supports_webrtc_voice(), "{base_url}");
    }

    for base_url in [
        "https://gateway.example.test/v1",
        "https://team.openai.azure.com.evil.test/v1",
        "https://team.services.ai.azure.com.evil.test/v1",
        "https://dashscope.aliyuncs.com.evil.test/v1",
        "https://workspace.cn-beijing.maas.aliyuncs.com.evil.test/v1",
    ] {
        let config = provider_config_from_text(&format!(
            "provider_id=custom\nbase_url={base_url}\nvoice_model=realtime-model\n"
        ));
        assert!(config.supports_webrtc_voice(), "{base_url}");
    }

    let legacy_azure_services = provider_config_from_text(
        "base_url=https://legacy.services.ai.azure.com/openai/v1\nvoice_model=realtime-model\n",
    );
    assert_eq!(legacy_azure_services.provider_id, PROVIDER_CUSTOM);
    assert!(!legacy_azure_services.supports_webrtc_voice());
}

#[test]
fn legacy_provider_config_inherits_conductor_from_planner() {
    let migrated = provider_config_from_text(
        "model=default-a\nplanner_model=planner-b\nexecutor_model=executor-c\n",
    );
    assert_eq!(migrated.model_for_conductor(), "planner-b");

    let explicit = provider_config_from_text(
        "model=default-a\nconductor_model=conductor-z\nplanner_model=planner-b\n",
    );
    assert_eq!(explicit.model_for_conductor(), "conductor-z");
}

#[test]
fn dashscope_provider_migrates_the_openai_embedding_default() {
    let migrated = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
             model=qwen-plus\n\
             embedding_model=text-embedding-3-small\n",
    );
    assert_eq!(
        migrated.model_for_role(&ModelRole::Embedder),
        DASHSCOPE_DEFAULT_EMBEDDING_MODEL
    );

    let explicit = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
             embedding_model=custom-embedding-model\n",
    );
    assert_eq!(
        explicit.model_for_role(&ModelRole::Embedder),
        "custom-embedding-model"
    );

    let workspace = provider_config_from_text(
        "base_url=https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1\n\
             embedding_model=text-embedding-3-small\n",
    );
    assert_eq!(
        workspace.model_for_role(&ModelRole::Embedder),
        "text-embedding-3-small"
    );

    let openai = ProviderConfig::default();
    assert_eq!(
        openai.model_for_role(&ModelRole::Embedder),
        "text-embedding-3-large"
    );
}

#[test]
fn provider_catalog_supplies_complete_defaults_and_explicit_vision_capabilities() {
    let openai = provider_model_defaults(PROVIDER_OPENAI).expect("OpenAI preset");
    assert_eq!(openai.chat, "gpt-4.1");
    assert_eq!(openai.embedding, "text-embedding-3-large");
    assert_eq!(openai.image, "gpt-image-2");
    assert_eq!(openai.voice, "gpt-realtime-2.1");
    assert_eq!(openai.context_window_tokens, 1_047_576);
    assert!(provider_supports_model_discovery(PROVIDER_OPENAI));
    assert!(provider_model_supports_tools(PROVIDER_OPENAI, "gpt-4.1").unwrap());
    assert_eq!(
        provider_model_supports_vision(PROVIDER_OPENAI, "gpt-4.1"),
        Some(true)
    );

    let alibaba = provider_model_defaults(PROVIDER_ALIBABA_CN).expect("Alibaba preset");
    assert_eq!(alibaba.chat, "qwen3.7-plus");
    assert_eq!(alibaba.embedding, "text-embedding-v4");
    assert_eq!(alibaba.image, "qwen-image-3.0-pro");
    assert_eq!(alibaba.voice, "qwen3-asr-flash-realtime");
    assert_eq!(alibaba.context_window_tokens, 1_000_000);
    assert!(!provider_supports_model_discovery(PROVIDER_AZURE_OPENAI));
    assert!(provider_discovers_modality(
        PROVIDER_ALIBABA_CN,
        "embedding"
    ));
    assert!(!provider_discovers_modality(
        PROVIDER_ALIBABA_CN,
        "imageGeneration"
    ));
    assert_eq!(
        provider_models_for_modality(PROVIDER_ALIBABA_CN, "embedding"),
        vec!["text-embedding-v4"]
    );
    assert_eq!(
        provider_model_supports_vision(PROVIDER_ALIBABA_CN, "qwen3.7-plus"),
        Some(true)
    );
    assert_eq!(
        provider_model_supports_vision(PROVIDER_ALIBABA_CN, "qwen3.7-max"),
        Some(false)
    );
}

#[test]
fn switching_provider_replaces_stale_models_with_builtin_modality_defaults() {
    let mut config = ProviderConfig {
        api_key: "openai-key".to_string(),
        ..ProviderConfig::default()
    };
    let mut input = provider_input_from_config(&config);
    input.provider_id = PROVIDER_ALIBABA_CN.to_string();
    input.provider_resource = "obsolete-workspace".to_string();
    input.base_url.clear();
    apply_provider_config_input(&mut config, input);

    assert_eq!(config.provider_id, PROVIDER_ALIBABA_CN);
    assert!(config.provider_resource.is_empty());
    assert_eq!(config.model, "qwen3.7-plus");
    assert_eq!(config.summarizer_model, "qwen3.7-flash");
    assert_eq!(config.embedding_model, "text-embedding-v4");
    assert_eq!(config.image_model, "qwen-image-3.0-pro");
    assert_eq!(config.voice_model, "qwen3-asr-flash-realtime");
    assert!(config.api_key.is_empty());
    assert!(config.auth_verified_at_ms.is_none());
}

#[test]
fn authenticated_catalog_reconciles_each_modality_without_inventing_access() {
    let mut config = ProviderConfig::default();
    reconcile_provider_models(
        &mut config,
        &[
            "gpt-4.1-mini".to_string(),
            "text-embedding-3-small".to_string(),
        ],
    )
    .expect("an available Chat preset should reconcile");

    assert_eq!(config.model, "gpt-4.1-mini");
    assert_eq!(config.executor_model, "gpt-4.1-mini");
    assert_eq!(config.embedding_model, "text-embedding-3-small");
    assert!(config.image_model.is_empty());
    assert!(config.voice_model.is_empty());
}

#[test]
fn unavailable_modalities_remain_disabled_after_config_round_trip() {
    let mut config = ProviderConfig::default();
    reconcile_provider_models(&mut config, &["gpt-4.1-mini".to_string()])
        .expect("the available Chat model should reconcile");

    let reloaded = provider_config_from_text(&provider_config_text(&config));
    assert_eq!(reloaded.model, "gpt-4.1-mini");
    assert!(reloaded.embedding_model.is_empty());
    assert!(reloaded.model_for_role(&ModelRole::Embedder).is_empty());
    assert!(reloaded.image_model.is_empty());
    assert!(reloaded.voice_model.is_empty());
}

#[test]
fn discovery_does_not_disable_modalities_served_by_a_separate_api() {
    let mut config = provider_config_from_text(
        "provider_id=alibaba_cn\n\
         base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n",
    );
    reconcile_provider_models(
        &mut config,
        &["qwen3.7-plus".to_string(), "text-embedding-v4".to_string()],
    )
    .expect("the standard Alibaba catalog should reconcile");

    assert_eq!(config.image_model, "qwen-image-3.0-pro");
    assert_eq!(config.voice_model, "qwen3-asr-flash-realtime");
}

#[test]
fn custom_catalog_must_include_the_configured_chat_model() {
    let mut config = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://gateway.example/v1".to_string(),
        model: "private-chat".to_string(),
        executor_model: "private-chat".to_string(),
        ..ProviderConfig::default()
    };

    let error = reconcile_provider_models(&mut config, &["other-chat".to_string()])
        .expect_err("a different model must not validate the configured custom model");
    assert!(error.contains("configured Chat model is unavailable"));
}

#[test]
fn ensemble_uses_distinct_role_models_in_stable_order() {
    let config = ProviderConfig {
        model: "default".to_string(),
        conductor_model: "conductor-z".to_string(),
        planner_model: "planner-a".to_string(),
        executor_model: "executor-b".to_string(),
        reviewer_model: "reviewer-c".to_string(),
        summarizer_model: "summary-d".to_string(),
        ..ProviderConfig::default()
    };

    assert_eq!(
        collaboration_candidate_models(&config, 3),
        vec![
            "planner-a".to_string(),
            "executor-b".to_string(),
            "reviewer-c".to_string(),
        ]
    );
    assert_eq!(config.model_for_conductor(), "conductor-z");
    let oversized_pool = collaboration_candidate_models(&config, 5);
    assert_eq!(oversized_pool.len(), MAX_ADAPTIVE_WORKFLOW_AGENTS);
    assert!(!oversized_pool.contains(&"conductor-z".to_string()));

    let worker_models = collaboration_candidate_models(&config, 3);
    let hints = collaboration_role_hints(&config, &worker_models);
    assert_eq!(hints.planner, "planner-a");
    assert_eq!(hints.executor, "executor-b");
    assert_eq!(hints.reviewer, "reviewer-c");
    assert_eq!(hints.synthesizer, "planner-a");
    assert!([
        &hints.planner,
        &hints.executor,
        &hints.reviewer,
        &hints.synthesizer,
    ]
    .iter()
    .all(|model| worker_models.contains(model)));
}

#[test]
fn role_hints_preserve_configured_model_reuse() {
    let config = ProviderConfig {
        model: "frontier".to_string(),
        planner_model: "frontier".to_string(),
        executor_model: "frontier".to_string(),
        reviewer_model: "reviewer".to_string(),
        summarizer_model: "frontier".to_string(),
        ..ProviderConfig::default()
    };
    let worker_models = collaboration_candidate_models(&config, 3);
    let hints = collaboration_role_hints(&config, &worker_models);

    assert_eq!(hints.planner, "frontier");
    assert_eq!(hints.executor, "frontier");
    assert_eq!(hints.reviewer, "reviewer");
    assert_eq!(hints.synthesizer, "frontier");
}

#[test]
fn collaboration_pool_preserves_the_run_conductors_primary_model() {
    let mut models = vec!["planner".to_string(), "reviewer".to_string()];
    prioritize_collaboration_model(&mut models, Some("frontier"), 2);

    assert_eq!(models, vec!["frontier".to_string(), "planner".to_string()]);
}

#[test]
fn pro_role_budget_does_not_collapse_when_roles_share_one_model() {
    let models = vec!["shared-frontier-model".to_string()];
    let budget = collaboration_agent_budget(3);
    let fallback = collaboration_fallback_models(&models, budget);

    assert_eq!(budget, 3);
    assert_eq!(fallback.len(), 3);
    assert!(fallback
        .iter()
        .all(|model| model == "shared-frontier-model"));
    assert_eq!(adaptive_workflow_step_budget(budget), 5);
}

#[test]
fn collaboration_terminal_workers_keep_terminal_stage_reserves() {
    assert_eq!(
        collaboration_worker_stage_class("worker_4", &ModelRole::Summarizer),
        RunStageClass::Synthesizer
    );
    assert_eq!(
        collaboration_worker_stage_class("worker_3", &ModelRole::Reviewer),
        RunStageClass::Reviewer
    );
    assert_eq!(
        collaboration_worker_stage_class("worker_1", &ModelRole::Executor),
        RunStageClass::Worker
    );
}

#[test]
fn run_context_selects_the_primary_model_without_collapsing_to_executor() {
    let config = ProviderConfig {
        model: "default-a".to_string(),
        executor_model: "executor-b".to_string(),
        ..ProviderConfig::default()
    };

    assert_eq!(
        config.model_for_agent_policy(&OrchestrationPolicy::Single),
        "default-a"
    );
    assert_eq!(
        config.model_for_agent_policy(&OrchestrationPolicy::PlanExecuteReview),
        "executor-b"
    );

    let run_context = [
        ("collaboration_policy".to_string(), "single".to_string()),
        ("agent_model".to_string(), "routed-c".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(agent_model_for_run(&config, &run_context), "routed-c");
}

#[test]
fn agent_effort_keeps_auto_and_pro_under_dynamic_policy_selection() {
    assert_eq!(
        AgentPolicy::parse_ingress("fast").requested_policy(),
        OrchestrationPolicy::Single
    );
    assert_eq!(
        AgentPolicy::parse_ingress("auto").requested_policy(),
        OrchestrationPolicy::AutoRouter
    );
    assert_eq!(
        AgentPolicy::parse_ingress("pro").requested_policy(),
        OrchestrationPolicy::AutoRouter
    );
    assert_eq!(AgentPolicy::parse_ingress("unknown"), AgentPolicy::Auto);
    assert_eq!(persisted_agent_policy(None), Ok(AgentPolicy::Auto));
    assert_eq!(persisted_agent_policy(Some("pro")), Ok(AgentPolicy::Pro));
    assert!(persisted_agent_policy(Some("Pro")).is_err());
    assert!(persisted_agent_policy(Some("future")).is_err());
}

#[test]
fn typed_agent_policy_provenance_preserves_the_legacy_runtime_wire() {
    for (policy, expected_policy, expected_source) in [
        (AgentPolicy::Fast, "single", "fast_direct"),
        (AgentPolicy::Auto, "auto_router", "dynamic_conductor_v2"),
        (AgentPolicy::Pro, "auto_router", "dynamic_conductor_v2"),
    ] {
        let planned = test_planned_agent_run(AgentRunDecision::direct("executor"), policy);
        let mut context = Metadata::new();
        planned
            .apply_to_context(&mut context)
            .expect("typed policy should adapt to the stable metadata wire");
        assert_eq!(
            context.get("agent_effort").map(String::as_str),
            Some(policy.label())
        );
        assert_eq!(
            context.get("requested_policy").map(String::as_str),
            Some(expected_policy)
        );
        assert_eq!(
            context.get("router_source").map(String::as_str),
            Some(expected_source)
        );
    }

    for (source, label) in [
        (AgentPlanningSource::FastDirect, "fast_direct"),
        (
            AgentPlanningSource::DynamicConductor,
            "dynamic_conductor_v2",
        ),
        (
            AgentPlanningSource::DynamicConductorReplanned,
            "dynamic_conductor_replanned",
        ),
        (
            AgentPlanningSource::CalibratedDirect,
            "dynamic_conductor_calibrated_direct",
        ),
        (
            AgentPlanningSource::DegradedDirect,
            "dynamic_conductor_degraded_direct",
        ),
        (
            AgentPlanningSource::DegradedWorkflow,
            "dynamic_conductor_degraded_workflow",
        ),
    ] {
        assert_eq!(source.label(), label);
    }
}

#[test]
fn causal_route_provenance_keeps_context_small_and_event_receipt_complete() {
    let planned = test_planned_agent_run(AgentRunDecision::direct("executor"), AgentPolicy::Auto);
    let mut context = [
        ("agent_run_id".to_string(), "causal-route-run".to_string()),
        ("steer_epoch".to_string(), "2".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    planned.apply_to_context(&mut context).unwrap();

    let receipt = &planned.execution_plan.compatibility_route;
    assert_eq!(
        context
            .get("pre_decision_context_fingerprint")
            .map(String::as_str),
        Some(receipt.context_fingerprint.as_str())
    );
    assert_eq!(
        context.get("pre_decision_task_class").map(String::as_str),
        Some(receipt.feature_snapshot.task_class.label())
    );
    assert_eq!(
        context.get("causal_route_receipt_sha256"),
        Some(&receipt.digest().unwrap())
    );
    assert_eq!(
        context.get("causal_route_selected_action_id"),
        Some(&receipt.selected_action_id)
    );
    assert!(!context.contains_key("causal_route_receipt"));

    let event_metadata = causal_route_event_metadata(&context, &planned.execution_plan).unwrap();
    let encoded = event_metadata.get("causal_route_receipt").unwrap();
    assert!(encoded.len() <= CAUSAL_ROUTE_MAX_RECEIPT_BYTES);
    let decoded = serde_json::from_str::<CausalRouteSelectionV2>(encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, *receipt);
    assert_eq!(
        event_metadata.get("causal_route_receipt_sha256"),
        Some(&receipt.digest().unwrap())
    );
    assert_eq!(
        event_metadata.get("route_decision_id").map(String::len),
        Some(64)
    );
}

#[test]
fn preparation_route_requirements_recompute_intent_and_active_images() {
    let mut run_context = [(
        "effective_prompt_objective".to_string(),
        "Audit this repository".to_string(),
    )]
    .into_iter()
    .collect::<Metadata>();
    let mut active_user = test_message(MessageRole::User, "Audit this repository");

    let read_only = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(
        read_only.minimum_tool_requirement,
        AgentToolRequirement::ReadOnly
    );
    assert_eq!(read_only.effect_authority, AgentEffectAuthority::Allowed);
    assert!(!read_only.image_input_required);

    active_user.metadata.insert(
        "image_paths".to_string(),
        "\n/private/tmp/screenshot.png\n".to_string(),
    );
    let with_image = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert!(with_image.image_input_required);

    run_context.insert("image_generation_required".to_string(), "true".to_string());
    let image_generation = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(
        image_generation.minimum_tool_requirement,
        AgentToolRequirement::Effects
    );
    assert_eq!(
        image_generation.effect_authority,
        AgentEffectAuthority::Required
    );

    run_context.insert("steer_epoch".to_string(), "1".to_string());
    run_context.insert(
        "prompt_objective".to_string(),
        "Instead, just explain what an audit is".to_string(),
    );
    run_context.remove("image_generation_required");
    let steered = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(steered.minimum_tool_requirement, AgentToolRequirement::None);
    assert_eq!(steered.effect_authority, AgentEffectAuthority::Allowed);

    run_context.insert("steer_epoch".to_string(), "0".to_string());
    run_context.insert(
        "effective_prompt_objective".to_string(),
        "Review this repository; do not modify anything".to_string(),
    );
    run_context.remove("prompt_objective");
    let explicit_read_only = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(
        explicit_read_only.effect_authority,
        AgentEffectAuthority::Forbidden
    );
}

#[test]
fn preparation_reset_clears_causal_route_epoch_provenance() {
    let keys = [
        "route_effect_authority",
        "pre_decision_context_fingerprint",
        "pre_decision_task_class",
        "route_requirements_fingerprint",
        "causal_route_policy",
        "causal_route_candidate",
        "causal_route_selected",
        "causal_route_selected_action_id",
        "causal_route_reason",
        "causal_route_receipt_sha256",
    ];
    let mut context = keys
        .iter()
        .map(|key| ((*key).to_string(), "stale".to_string()))
        .collect::<Metadata>();

    reset_preparation_run_context(&mut context);

    assert!(keys.iter().all(|key| !context.contains_key(*key)));
}

#[test]
fn additive_steer_retains_only_current_run_image_requirements() {
    let mut run_context = [
        ("agent_run_id".to_string(), "run-current".to_string()),
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            "Initial request:\nInspect this image\n\nAccepted steering 1:\nFocus on the header"
                .to_string(),
        ),
        (
            "prompt_objective".to_string(),
            "Focus on the header".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut current_image = test_message(MessageRole::User, "Inspect this image");
    current_image
        .metadata
        .insert("agent_run_id".to_string(), "run-current".to_string());
    current_image.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/current.png".to_string(),
    );
    let mut old_image = test_message(MessageRole::User, "Old image request");
    old_image
        .metadata
        .insert("agent_run_id".to_string(), "run-old".to_string());
    old_image.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/old.png".to_string(),
    );
    let active_steer = test_message(MessageRole::User, "Focus on the header");

    let additive = route_requirements_for_preparation(
        &run_context,
        &[
            old_image.clone(),
            current_image.clone(),
            active_steer.clone(),
        ],
        &active_steer,
    );
    assert!(additive.image_input_required);

    let old_run_only = route_requirements_for_preparation(
        &run_context,
        &[old_image, active_steer.clone()],
        &active_steer,
    );
    assert!(!old_run_only.image_input_required);

    let mut source_image = test_message(MessageRole::User, "Recovered image request");
    source_image
        .metadata
        .insert("agent_run_id".to_string(), "run-source".to_string());
    source_image.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/source.png".to_string(),
    );
    run_context.insert("source_agent_run_id".to_string(), "run-source".to_string());
    let recovered_additive = route_requirements_for_preparation(
        &run_context,
        &[source_image.clone(), active_steer.clone()],
        &active_steer,
    );
    assert!(recovered_additive.image_input_required);

    run_context.insert(
        "prompt_objective".to_string(),
        "Instead, ignore the image and explain headers generally".to_string(),
    );
    let replacement = route_requirements_for_preparation(
        &run_context,
        &[current_image, active_steer.clone()],
        &active_steer,
    );
    assert!(!replacement.image_input_required);
    let recovered_replacement = route_requirements_for_preparation(
        &run_context,
        &[source_image, active_steer.clone()],
        &active_steer,
    );
    assert!(!recovered_replacement.image_input_required);
}

#[test]
fn replacement_steer_keeps_route_and_execution_intent_aligned() {
    let replacement = "Instead, just explain how crash diagnosis works";
    let run_context = [
        ("agent_run_id".to_string(), "run-current".to_string()),
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            format!(
                "Initial request:\nFix the crash in this image\n\nAccepted steering 1:\n{replacement}"
            ),
        ),
        ("prompt_objective".to_string(), replacement.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut initial = test_message(MessageRole::User, "Fix the crash in this image");
    initial
        .metadata
        .insert("agent_run_id".to_string(), "run-current".to_string());
    initial.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/current.png".to_string(),
    );
    let active_steer = test_message(MessageRole::User, replacement);

    let requirements = route_requirements_for_preparation(
        &run_context,
        &[initial, active_steer.clone()],
        &active_steer,
    );

    assert_eq!(
        requirements.minimum_tool_requirement,
        AgentToolRequirement::None
    );
    assert!(!requirements.image_input_required);
    assert_eq!(
        agent_runtime::prompt_completion_intent(&run_context).tool_requirement,
        agent_runtime::PromptToolRequirement::None
    );
}

#[test]
fn image_generation_route_follows_additive_and_replacement_steers() {
    let config = ProviderConfig {
        base_url: "https://provider.example/v1".to_string(),
        image_model: "image-model".to_string(),
        ..ProviderConfig::default()
    };
    let replacement = "Instead, just explain lighthouse composition";
    let replacement_effective = format!(
        "Initial request:\nGenerate an image of a lighthouse\n\nAccepted steering 1:\n{replacement}"
    );
    let mut replacement_context = [
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            replacement_effective.clone(),
        ),
        ("prompt_objective".to_string(), replacement.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let replacement_objective = image_generation_objective_for_preparation(
        &replacement_context,
        &replacement_effective,
        replacement,
    );
    assert_eq!(replacement_objective, replacement);
    add_image_generation_run_context(&mut replacement_context, &config, replacement_objective);
    assert!(!replacement_context.contains_key("image_generation_required"));

    let additive = "Generate an image of the proposed layout";
    let additive_effective =
        format!("Initial request:\nExplain the layout\n\nAccepted steering 1:\n{additive}");
    let mut additive_context = [
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            additive_effective.clone(),
        ),
        ("prompt_objective".to_string(), additive.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let additive_objective = image_generation_objective_for_preparation(
        &additive_context,
        &additive_effective,
        additive,
    );
    assert_eq!(additive_objective, additive_effective);
    add_image_generation_run_context(&mut additive_context, &config, additive_objective);
    assert_eq!(
        additive_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
    let active_user = test_message(MessageRole::User, additive);
    assert_eq!(
        route_requirements_for_preparation(
            &additive_context,
            std::slice::from_ref(&active_user),
            &active_user,
        )
        .minimum_tool_requirement,
        AgentToolRequirement::Effects
    );
}

#[test]
fn route_fallback_prefers_the_first_compatible_configured_model() {
    let candidates = vec![
        ModelCandidate {
            name: "text-only".to_string(),
            role: ModelRole::Executor,
            supports_tools: false,
            supports_vision: false,
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
            cost_tier: 1,
            latency_tier: 1,
        },
        ModelCandidate {
            name: "capable".to_string(),
            role: ModelRole::Planner,
            supports_tools: true,
            supports_vision: true,
            tools_capability_source: ModelCapabilitySource::Configured,
            vision_capability_source: ModelCapabilitySource::Configured,
            cost_tier: 2,
            latency_tier: 2,
        },
    ];
    let allowed_models = vec!["text-only".to_string(), "capable".to_string()];
    let requirements = AgentRouteRequirements {
        minimum_tool_requirement: AgentToolRequirement::ReadOnly,
        effect_authority: AgentEffectAuthority::Forbidden,
        image_input_required: true,
    };

    assert_eq!(
        preferred_compatible_route_model("text-only", &allowed_models, &candidates, requirements,)
            .as_deref(),
        Some("capable")
    );
    assert_eq!(
        preferred_compatible_route_model(
            "text-only",
            &allowed_models[..1],
            &candidates[..1],
            requirements,
        ),
        None
    );
    assert_eq!(
        preferred_compatible_route_model(
            "capable",
            &allowed_models[..1],
            &candidates,
            requirements,
        ),
        None,
        "a capable preferred model outside the configured pool must not be selected"
    );
}

#[test]
fn calibrated_direct_route_metadata_is_explicit_and_not_degraded() {
    let route_requirements = AgentRouteRequirements {
        minimum_tool_requirement: AgentToolRequirement::ReadOnly,
        effect_authority: AgentEffectAuthority::Forbidden,
        image_input_required: true,
    };
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
    let mut candidate = route_requirements.apply_to_direct(AgentRunDecision::direct("executor"));
    candidate.execution = AgentExecutionMode::Workflow;
    candidate.verification = AgentVerificationPolicy::Independent;
    candidate.max_parallelism = 2;
    candidate.min_successful_branches = 2;
    candidate.distinct_contributions = 2;
    candidate.estimated_steps = 3;
    candidate.expected_uplift_bps = 2_500;
    candidate.confidence_bps = 7_000;
    candidate.stop_policy = ConductorStopPolicy::Quorum;
    let snapshot = RouteFeatureSnapshotV2::from_decision_request(
        &candidate,
        RouteFeatureRequest {
            objective: "update the workspace",
            recent_context: "",
            effort: "auto",
            requirements: route_requirements,
            budget_fingerprint: None,
            prompt_profile_sha256: &"1".repeat(64),
        },
        &candidates,
    );
    let mut receipt = select_causal_route_v2(&candidate, &snapshot, &candidates, None, 0)
        .expect("workflow candidate should produce a causal route receipt");
    let mut decision = candidate.clone().constrained_to_grounded_direct();
    decision.calibration = Some(AgentDecisionCalibration::MatchedEvidence);
    decision.calibration_reason = Some("matched evidence rejects workflow".to_string());
    if receipt.selected_route != decision.route_tier() {
        receipt
            .reconcile_selected_route(
                decision.route_tier(),
                CausalRouteReason::ExecutionConstraint,
            )
            .expect("calibrated direct route should be one of the recorded actions");
    }
    decision.causal_route = Some(receipt);
    let mut planned = test_planned_agent_run_with_candidate(candidate, decision, AgentPolicy::Auto);
    planned.source = AgentPlanningSource::CalibratedDirect;
    planned.route_requirements = route_requirements;
    let mut run_context = Metadata::new();

    planned
        .apply_to_context(&mut run_context)
        .expect("calibrated route metadata should persist");

    assert_eq!(
        run_context
            .get("route_minimum_tool_requirement")
            .map(String::as_str),
        Some("read_only")
    );
    assert_eq!(
        run_context
            .get("route_image_input_required")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        run_context.get("decision_calibration").map(String::as_str),
        Some("matched_evidence_direct")
    );
    assert_eq!(
        run_context.get("candidate_route_tier").map(String::as_str),
        Some("workflow")
    );
    assert_eq!(
        run_context.get("selected_route_tier").map(String::as_str),
        Some("direct")
    );
    assert_eq!(
        run_context.get("conductor_degraded").map(String::as_str),
        Some("false")
    );
}

#[test]
fn fast_policy_never_enters_prompt_evolution_selection() {
    assert!(!should_evaluate_strategy_profile(AgentPolicy::Fast, true));
    assert!(!should_evaluate_strategy_profile(AgentPolicy::Auto, false));
    assert!(should_evaluate_strategy_profile(AgentPolicy::Auto, true));
    assert!(should_evaluate_strategy_profile(AgentPolicy::Pro, true));
}

#[test]
fn retry_recovers_effort_from_the_active_run() {
    let mut event = Event {
        id: EventId("run-start".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task started".to_string(),
        metadata: [("agent_effort".to_string(), "pro".to_string())]
            .into_iter()
            .collect(),
    };

    assert_eq!(
        persisted_agent_policy_from_active_events(std::slice::from_ref(&event)),
        Ok(AgentPolicy::Pro)
    );

    event
        .metadata
        .insert("agent_effort".to_string(), "future".to_string());
    assert!(persisted_agent_policy_from_active_events(&[event]).is_err());
    assert_eq!(
        persisted_agent_policy_from_active_events(&[]),
        Ok(AgentPolicy::Auto)
    );
}

#[test]
fn adaptive_coordinator_parses_bounded_dependency_workflow() {
    let response = r#"```json
        {"steps":[
          {"id":"independent-a","role":"thinker","model":"planner-a","subtask":"Analyze one path","access":[]},
          {"id":"independent-b","role":"worker","model":"reviewer-b","subtask":"Challenge assumptions","access":[]},
          {"id":"verify","role":"verifier","model":"summary-c","subtask":"Verify both paths","access":["independent-a","independent-b"]},
          {"id":"final","role":"synthesizer","model":"summary-c","subtask":"Synthesize guidance","access":["independent-a","independent-b","verify"]}
        ]}
        ```"#;
    let models = vec![
        "planner-a".to_string(),
        "reviewer-b".to_string(),
        "summary-c".to_string(),
    ];

    let workflow = test_conductor_harness(models, 3)
        .parse_plan(response)
        .expect("workflow should parse")
        .adaptive_workflow();

    assert_eq!(workflow.steps.len(), 4);
    assert_eq!(
        workflow.steps[3].access,
        vec![
            "independent-a".to_string(),
            "independent-b".to_string(),
            "verify".to_string()
        ]
    );
    assert_eq!(
        adaptive_workflow_layers(&workflow).expect("layers should build"),
        vec![vec![0, 1], vec![2], vec![3]]
    );
}

#[test]
fn adaptive_coordinator_rejects_models_outside_the_configured_pool() {
    let response = r#"{"steps":[
          {"id":"first","role":"thinker","model":"configured","subtask":"Analyze","access":[]},
          {"id":"second","role":"worker","model":"configured","subtask":"Challenge","access":[]},
          {"id":"verify","role":"verifier","model":"configured","subtask":"Verify","access":["first","second"]},
          {"id":"final","role":"synthesizer","model":"unconfigured","subtask":"Synthesize","access":["first","second","verify"]}
        ]}"#;

    let error = test_conductor_harness(vec!["configured".to_string()], 3)
        .parse_plan(response)
        .expect_err("unknown model should be rejected");

    assert!(error.contains("unknown model"));
}

#[test]
fn adaptive_coordinator_rejects_a_fifth_step_beyond_the_execution_contract() {
    let response = r#"{"steps":[
          {"id":"a","role":"thinker","model":"planner-a","subtask":"Independent approach","access":[]},
          {"id":"b","role":"worker","model":"reviewer-b","subtask":"Independent challenge","access":[]},
          {"id":"verify","role":"verifier","model":"summary-c","subtask":"Verify both","access":["a","b"]},
          {"id":"refine","role":"worker","model":"planner-a","subtask":"Refine verified work","access":["verify"]},
          {"id":"final","role":"synthesizer","model":"reviewer-b","subtask":"Synthesize","access":["b","refine"]}
        ]}"#;
    let models = vec![
        "planner-a".to_string(),
        "reviewer-b".to_string(),
        "summary-c".to_string(),
    ];

    let error = test_conductor_harness(models, 3)
        .parse_plan(response)
        .expect_err("the execution contract must reject a fifth step");

    assert!(error.contains("exceeds the 4-step budget"));
}

#[test]
fn conductor_result_separates_worker_claims_from_tool_evidence() {
    let result = collaboration_step_result(
        "inspect",
        "model-a",
        "The config probably uses model A.",
        &[CollaborationEvidence {
            evidence_schema: String::new(),
            steer_epoch: None,
            collaboration_id: String::new(),
            source_step: "worker_1".to_string(),
            tool_call_id: "call-1".to_string(),
            tool_name: "file.read".to_string(),
            request: "path=config.toml".to_string(),
            status: "succeeded".to_string(),
            output: "model = B".to_string(),
        }],
    );

    assert!(result.contains("treat as a proposal until supported"));
    assert!(result.contains("Tool evidence ledger"));
    assert!(result.contains(
        "ref=worker_1::call-1 source=worker_1 call=call-1 tool=file.read status=succeeded"
    ));
    assert!(result.contains("model = B"));
}

#[test]
fn conductor_merges_authorized_evidence_and_deduplicates_provenance() {
    let a = CollaborationEvidence {
        evidence_schema: crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA
            .to_string(),
        steer_epoch: Some(4),
        collaboration_id: "collaboration-1".to_string(),
        source_step: "worker_a".to_string(),
        tool_call_id: "call-1".to_string(),
        tool_name: "file.read".to_string(),
        request: "path=a.txt".to_string(),
        status: "succeeded".to_string(),
        output: "A".to_string(),
    };
    let b = CollaborationEvidence {
        evidence_schema: crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA
            .to_string(),
        steer_epoch: Some(4),
        collaboration_id: "collaboration-1".to_string(),
        source_step: "worker_b".to_string(),
        tool_call_id: "call-1".to_string(),
        tool_name: "file.read".to_string(),
        request: "path=b.txt".to_string(),
        status: "succeeded".to_string(),
        output: "B".to_string(),
    };
    let inherited = [
        ("a".to_string(), vec![a.clone()]),
        ("b".to_string(), vec![b.clone(), a.clone()]),
    ]
    .into_iter()
    .collect();

    let merged = merge_collaboration_evidence(
        "current",
        &["a".to_string(), "b".to_string()],
        &inherited,
        &[b],
        "collaboration-1",
        4,
    );

    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0].source_step, "a");
    assert_eq!(merged[1].source_step, "b");
    assert_eq!(merged[2].source_step, "current");
}

#[test]
fn conductor_bounds_checkpoint_evidence_and_preserves_current_step_observations() {
    let inherited = (0..40)
        .map(|index| CollaborationEvidence {
            evidence_schema: crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA
                .to_string(),
            steer_epoch: Some(4),
            collaboration_id: "collaboration-1".to_string(),
            source_step: "source".to_string(),
            tool_call_id: format!("inherited-{index}"),
            tool_name: "file.read".to_string(),
            request: "r".repeat(3_000),
            status: "succeeded".to_string(),
            output: "o".repeat(3_000),
        })
        .collect::<Vec<_>>();
    let own = (0..4)
        .map(|index| CollaborationEvidence {
            evidence_schema: crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA
                .to_string(),
            steer_epoch: Some(4),
            collaboration_id: "collaboration-1".to_string(),
            source_step: "current".to_string(),
            tool_call_id: format!("own-{index}"),
            tool_name: "file.read".to_string(),
            request: "request".to_string(),
            status: "succeeded".to_string(),
            output: "current observation".to_string(),
        })
        .collect::<Vec<_>>();
    let evidence_by_step = BTreeMap::from([("source".to_string(), inherited)]);

    let merged = merge_collaboration_evidence(
        "current",
        &["source".to_string()],
        &evidence_by_step,
        &own,
        "collaboration-1",
        4,
    );

    assert_eq!(merged.len(), 32);
    assert_eq!(
        merged
            .iter()
            .filter(|entry| entry.source_step == "current")
            .count(),
        own.len()
    );
    assert!(merged
        .iter()
        .all(|entry| entry.request.chars().count() <= 2_012));
    assert!(merged
        .iter()
        .all(|entry| entry.output.chars().count() <= 2_012));
}

#[test]
fn collaboration_stages_have_visible_timeline_labels() {
    let event = Event {
        id: EventId("candidate-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::ModelRequestStarted,
        summary: "Collaboration candidate_2 started".to_string(),
        metadata: [
            ("collaboration_id".to_string(), "collab-1".to_string()),
            ("stage".to_string(), "candidate_2".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    assert_eq!(timeline_event_label(&event), "Candidate 2");
    assert_eq!(
        collaboration_stage_display_label("coordinator"),
        "Conductor"
    );
    assert_eq!(
        collaboration_stage_display_label("conductor_plan"),
        "Conductor"
    );
    assert_eq!(
        collaboration_stage_display_label("conductor_repair"),
        "Conductor repair"
    );
    assert_eq!(collaboration_stage_display_label("worker_3"), "Worker 3");
    assert_eq!(collaboration_stage_display_label("arbiter"), "Arbiter");
    assert_eq!(
        collaboration_stage_display_label("synthesizer"),
        "Synthesis"
    );
}

#[test]
fn agent_system_prompt_config_encoding_preserves_multiline_unicode() {
    let prompt = "你是 Cindx。\n先检查事实，再执行。";
    let encoded = config_hex_encode(prompt);

    assert_eq!(config_hex_decode(&encoded).as_deref(), Some(prompt));
    assert!(config_hex_decode("not-hex").is_none());
}

#[test]
fn personalization_is_normalized_and_applied_to_agent_instructions() {
    let config = normalized_personalization_config(PersonalizationConfig {
        preferred_name: "  Dale\nAdmin  ".to_string(),
        response_tone: "direct".to_string(),
        response_length: "concise".to_string(),
    });
    let instructions = personalized_agent_instructions(&config, "Use Chinese when asked.");

    assert_eq!(config.preferred_name, "DaleAdmin");
    assert!(instructions.contains("The user's preferred name is \"DaleAdmin\""));
    assert!(instructions.contains("answer with \"DaleAdmin\""));
    assert!(instructions.contains("direct, factual tone"));
    assert!(instructions.contains("Keep responses concise"));
    assert!(instructions.starts_with("Use Chinese when asked."));
    assert!(instructions
        .ends_with("Keep responses concise unless more detail is necessary for correctness."));
}

#[test]
fn invalid_personalization_options_fall_back_to_safe_defaults() {
    let config = normalized_personalization_config(PersonalizationConfig {
        preferred_name: String::new(),
        response_tone: "unknown".to_string(),
        response_length: "unbounded".to_string(),
    });

    assert_eq!(config.response_tone, "natural");
    assert_eq!(config.response_length, "balanced");
}

#[test]
fn agent_runtime_context_includes_the_time_computed_for_the_user_turn() {
    let context = [(
        "current_time".to_string(),
        "2026-07-11 10:30 CST".to_string(),
    )]
    .into_iter()
    .collect();
    let runtime_context =
        agent_runtime_context_for_run(&context).expect("time context should exist");

    assert!(runtime_context.contains("Current date and time: 2026-07-11 10:30 CST"));
    assert!(runtime_context.contains("authoritative for this turn"));
}

#[test]
fn collaboration_prompt_keeps_core_user_instructions_and_runtime_context() {
    let context = [(
        "current_time".to_string(),
        "2026-07-11 10:30 CST".to_string(),
    )]
    .into_iter()
    .collect();
    let prompt = collaboration_system_prompt_for_run("Answer in Chinese.", &context);

    assert!(prompt.starts_with("You are Cindx"));
    assert!(prompt.contains("<user_instructions>\nAnswer in Chinese."));
    assert!(prompt.contains("<runtime_context>"));
    assert!(prompt.contains("Current date and time: 2026-07-11 10:30 CST"));
}

#[test]
fn legacy_default_prompt_migrates_to_empty_custom_instructions() {
    assert!(ProviderConfig::default().agent_system_prompt.is_empty());
    assert!(normalized_agent_instructions("").is_empty());
    assert!(normalized_agent_instructions(LEGACY_AGENT_SYSTEM_PROMPT).is_empty());
    assert_eq!(
        normalized_agent_instructions("  Prefer concise answers.  "),
        "Prefer concise answers."
    );
}

#[test]
fn agent_state_reports_context_usage_and_hides_internal_drafts() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("prompt".to_string(), "inspect".to_string()),
            ("context_window_tokens".to_string(), "100000".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        [("prompt_tokens".to_string(), "25000".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("usage should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "internal draft",
        [("internal".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("draft should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "final answer",
    )
    .expect("final should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.context_tokens_used, 25_000);
    assert_eq!(state.context_window_tokens, 100_000);
    assert_eq!(state.context_remaining_percent, 75.0);
    assert!(!state.context_usage_estimated);
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[1].content, "final answer");
}

#[test]
fn phase5_state_lists_tool_results() {
    let store = Mutex::new(SqliteStore::in_memory().expect("store should open"));
    let execution_gate = Mutex::new(());
    let root = workspace_root();
    let registry = ToolRegistry::with_workspace_tools(root.clone());
    execute_manual_tool_invocation(
        &execution_gate,
        &store,
        &registry,
        ToolInvocation {
            id: agent_core::ToolCallId("tool-1".to_string()),
            task_id: phase5_task_id(),
            tool_name: "file.list".to_string(),
            input_json: encode_input(&[("path", ".")]),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        },
        &root,
        None,
    )
    .expect("tool should execute");

    let store = store.lock().expect("store should lock");
    let state = phase5_state(&store, None, &root).expect("state should load");

    assert!(state.tools.iter().any(|tool| tool.name == "file.write"));
    assert_eq!(state.results.len(), 1);
    assert_eq!(state.results[0].tool_name, "file.list");
}

#[test]
fn phase5_state_lists_pending_tool_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let root = workspace_root();
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("tool-2".to_string()),
        task_id: phase5_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", ".cindx/phase5-test.txt"), ("content", "ok")]),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(root.clone());
    let mut request = registry
        .get("file.write")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("write should request permission");
    request.id = PermissionRequestId("perm-phase5".to_string());
    request
        .metadata
        .insert("phase".to_string(), "5".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    store
        .save_permission_request(request, 123)
        .expect("request should save");

    let state = phase5_state(&store, None, &root).expect("state should load");

    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "file.write");
}

#[test]
fn phase6_state_lists_orchestration_steps() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase6_task_id(),
        EventKind::ModelRequestFinished,
        "planner step finished",
        [
            ("orchestration_id".to_string(), "orch-1".to_string()),
            ("policy".to_string(), "plan_execute_review".to_string()),
            ("step_index".to_string(), "0".to_string()),
            ("role".to_string(), "planner".to_string()),
            ("model".to_string(), "model-a".to_string()),
            ("latency_ms".to_string(), "12".to_string()),
            ("output".to_string(), "Plan first.".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase6_state(&store, None).expect("state should load");

    assert_eq!(state.steps.len(), 1);
    assert_eq!(state.steps[0].policy, "plan_execute_review");
    assert_eq!(state.steps[0].role, "planner");
    assert_eq!(state.steps[0].output, "Plan first.");
}

#[test]
fn workspace_index_falls_back_to_local_embeddings_when_cloud_fails() {
    struct FailingEmbedder;

    impl RagEmbedder for FailingEmbedder {
        fn embed_texts(&mut self, _texts: &[String]) -> Result<EmbeddingBatch, RagError> {
            Err(RagError::new("configured embedding model is unavailable"))
        }
    }

    let root = temp_test_root("phase7-cloud-fallback");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "# Cindx\n\nLocal indexing remains available when cloud embeddings fail.",
    )
    .expect("fixture should write");

    let (index, backend, model, fallback_error) = index_workspace_with_cloud_fallback(
        &root,
        IndexOptions::default(),
        &mut FailingEmbedder,
        "missing-cloud-model",
    )
    .expect("local fallback should build the index");

    assert_eq!(backend, "local-fallback");
    assert!(model.starts_with("local-hash-"));
    assert_eq!(
        fallback_error.as_deref(),
        Some("configured embedding model is unavailable")
    );
    assert!(!index.chunks.is_empty());
    assert!(index
        .chunks
        .iter()
        .all(|chunk| chunk.embedding_provider == "local"));
}

#[test]
fn phase7_state_reports_rag_stats() {
    let root = temp_test_root("phase7-stats");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "# Cindx\n\nThe RAG index stores line-level provenance.",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "Workspace indexed for RAG",
        [
            ("action".to_string(), "index".to_string()),
            ("files_indexed".to_string(), "1".to_string()),
            ("chunks_indexed".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase7_state(
        &store,
        adapter.stats(),
        MemoryStatsView::default(),
        Vec::new(),
        None,
        empty_graph_state(),
        None,
        None,
        None,
    )
    .expect("state should load");

    assert_eq!(state.stats.files_indexed, 1);
    assert_eq!(state.stats.chunks_indexed, 1);
    assert!(state
        .timeline
        .iter()
        .any(|entry| entry.label == "Retrieval"));
}

#[test]
fn phase7_graph_counts_bind_to_the_latest_exact_index_path() {
    let root = temp_test_root("phase7-graph-count-summary");
    let active_path = root.join("generation-new").join("rag-index.tsv");
    let stale_path = root.join("generation-old").join("rag-index.tsv");
    let other_workspace_path = root.join("other-workspace").join("rag-index.tsv");
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (path, nodes, edges) in [
        (&active_path, 3, 2),
        (&other_workspace_path, 99, 88),
        (&active_path, 7, 6),
        (&stale_path, 55, 44),
    ] {
        append_event(
            &mut store,
            &phase7_task_id(),
            EventKind::RetrievalPerformed,
            "Workspace indexed for RAG",
            [
                ("action".to_string(), "index".to_string()),
                ("index_path".to_string(), path.display().to_string()),
                ("graph_nodes".to_string(), nodes.to_string()),
                ("graph_edges".to_string(), edges.to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("index event should append");
    }
    let events = store
        .list_by_task(&phase7_task_id())
        .expect("phase 7 events should load");

    let graph = graph_count_summary_for_index_events(&events, &active_path);

    assert_eq!(graph.total_nodes, 7);
    assert_eq!(graph.total_edges, 6);
    assert!(graph.nodes.is_empty());
    assert!(graph.edges.is_empty());
    assert_eq!(
        graph_count_summary_for_index_events(
            &events,
            &root.join("never-indexed").join("rag-index.tsv")
        )
        .total_nodes,
        0
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rag_sources_include_line_ranges() {
    let root = temp_test_root("phase7-sources");
    fs::create_dir_all(root.join("docs")).expect("temp docs should exist");
    fs::write(
        root.join("docs").join("rag.md"),
        "Intro\nSemantic retrieval should cite exact source lines.\nDone",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");

    let results = adapter
        .search("semantic retrieval source lines", 3)
        .expect("search should run");
    let sources = rag_sources_from_results(&results);

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].path, "docs/rag.md");
    assert_eq!(sources[0].start_line, 1);
    assert_eq!(sources[0].end_line, 3);
    assert!(sources[0].score > 0.0);
}

#[test]
fn context_state_builds_and_persists_restore_pack() {
    let root = temp_test_root("phase15-context");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::User,
        "Continue the MVP context manager",
    )
    .expect("message should append");
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "RAG search completed",
        [
            ("action".to_string(), "search".to_string()),
            ("query".to_string(), "context compression".to_string()),
            ("selected_count".to_string(), "2".to_string()),
            (
                "retrieval_mode".to_string(),
                "four_way_parallel".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("retrieval should append");

    let run_context = Metadata::new();
    let preview =
        context_state(&store, &root, &run_context, None, None).expect("state should load");
    let checkpoint = preview.checkpoint.expect("checkpoint should exist");

    assert_eq!(
        checkpoint.current_goal.as_deref(),
        Some("Continue the MVP context manager")
    );
    assert!(checkpoint.path.is_none());
    assert!(checkpoint.restore_pack.contains("## Current Goal"));
    assert!(checkpoint.restore_pack.contains("four_way_parallel"));

    let events = collect_context_events(&store, &run_context).expect("events should collect");
    let pack = build_restore_context_pack(build_session_checkpoint_at(
        &events,
        CheckpointOptions::default(),
        777,
    ));
    let checkpoint_path =
        write_context_checkpoint(&root, None, &pack.text, None).expect("checkpoint should write");
    append_event(
        &mut store,
        &phase15_task_id(),
        EventKind::TaskStatusChanged,
        "Context checkpoint compacted",
        [
            ("checkpoint_id".to_string(), pack.checkpoint.id.clone()),
            (
                "context_checkpoint_path".to_string(),
                checkpoint_path.display().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("compact event should append");

    let compacted =
        context_state(&store, &root, &run_context, Some(pack), None).expect("state should load");
    let checkpoint = compacted.checkpoint.expect("checkpoint should exist");
    let expected_path = checkpoint_path.display().to_string();

    assert_eq!(checkpoint.path.as_deref(), Some(expected_path.as_str()));
    assert!(fs::read_to_string(checkpoint_path)
        .expect("checkpoint should be readable")
        .contains("Cindx Context Checkpoint"));
    assert!(compacted
        .timeline
        .iter()
        .any(|entry| entry.detail.contains("Context checkpoint compacted")));
}

#[test]
fn context_events_and_checkpoint_paths_are_session_scoped() {
    let root = temp_test_root("phase15-session-scope");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, content) in [("session-a", "alpha"), ("session-b", "beta")] {
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            content,
            [
                ("project_id".to_string(), "project-a".to_string()),
                ("session_id".to_string(), session_id.to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("message should append");
    }
    let run_context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
    ]
    .into_iter()
    .collect();

    let events = collect_context_events(&store, &run_context).expect("events should collect");

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].metadata.get("content").map(String::as_str),
        Some("alpha")
    );
    assert_ne!(
        context_checkpoint_path_for_session(&root, Some("session-a")),
        context_checkpoint_path_for_session(&root, Some("session-b"))
    );
}

#[test]
fn context_checkpoint_coverage_restores_only_the_verified_prefix() {
    let root = temp_test_root("context-checkpoint-coverage");
    let history = vec![
        test_message(MessageRole::User, "first requirement"),
        test_message(MessageRole::Assistant, "first answer"),
        test_message(MessageRole::User, "second requirement"),
        test_message(MessageRole::Assistant, "second answer"),
    ];
    let path = write_context_checkpoint(
        &root,
        Some("session-a"),
        "verified checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 2,
        }),
    )
    .expect("checkpoint should write");

    let checkpoint = read_validated_context_checkpoint(&root, Some("session-a"), &history)
        .expect("coverage should validate");
    assert_eq!(checkpoint.covered_messages, 2);
    let restored = history_with_context_checkpoint(&history, checkpoint, &path)
        .expect("history should restore");

    assert_eq!(restored.len(), 3);
    assert_eq!(restored[1].content, "second requirement");
    assert_eq!(restored[2].content, "second answer");
    assert_eq!(
        restored[0]
            .metadata
            .get("covered_messages")
            .map(String::as_str),
        Some("2")
    );
}

#[test]
fn legacy_context_checkpoint_never_truncates_history() {
    let root = temp_test_root("legacy-context-checkpoint");
    let history = vec![test_message(MessageRole::User, "keep this verbatim")];
    write_context_checkpoint(&root, Some("session-a"), "legacy checkpoint", None)
        .expect("legacy checkpoint should write");

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history,).is_none());
}

#[test]
fn context_checkpoint_rejects_changed_history_prefix() {
    let root = temp_test_root("stale-context-prefix");
    let history = vec![
        test_message(MessageRole::User, "original requirement"),
        test_message(MessageRole::Assistant, "original answer"),
        test_message(MessageRole::User, "uncovered tail"),
    ];
    write_context_checkpoint(
        &root,
        Some("session-a"),
        "checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 2,
        }),
    )
    .expect("checkpoint should write");
    let mut changed = history.clone();
    changed[0].content = "edited requirement".to_string();

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &changed,).is_none());
    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history[..2],).is_none());
}

#[test]
fn context_checkpoint_rejects_manifest_content_mismatch() {
    let root = temp_test_root("stale-context-content");
    let history = vec![test_message(MessageRole::User, "requirement")];
    let path = write_context_checkpoint(
        &root,
        Some("session-a"),
        "checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 1,
        }),
    )
    .expect("checkpoint should write");
    fs::write(path, "different checkpoint").expect("checkpoint should mutate");

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history,).is_none());
}

#[test]
fn context_governor_preserves_canonical_history_and_latest_tool_round() {
    let tools = vec![ToolSpec::builtin(
        "file.read",
        "file",
        "Read a workspace file",
        ToolRisk::ReadOnly,
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
    )];
    let mut runtime = start_agent_loop(
        TaskId("context-governor-test".to_string()),
        "current goal: finish the verified implementation",
        AgentRuntimeConfig { max_turns: 24 },
    );
    let current_user = runtime.messages.pop().expect("current user should exist");
    runtime.messages.push(Message {
        role: MessageRole::System,
        content: "artifact ".repeat(8_000),
        metadata: [("kind".to_string(), "artifact_manifest".to_string())]
            .into_iter()
            .collect(),
    });
    runtime.messages.push(test_message(
        MessageRole::User,
        "旧的中文需求".repeat(5_000),
    ));
    runtime.messages.push(test_message(
        MessageRole::Assistant,
        "old answer ".repeat(5_000),
    ));
    runtime.messages.push(current_user);
    for index in 0..5 {
        runtime.messages.push(Message {
                role: MessageRole::Assistant,
                content: format!("tool round {index}"),
                metadata: [(
                    "raw_tool_calls_json".to_string(),
                    format!(
                        r#"[{{"id":"call-{index}","type":"function","function":{{"name":"file_read","arguments":"{{\"path\":\"file-{index}\"}}"}}}}]"#
                    ),
                )]
                .into_iter()
                .collect(),
            });
        runtime.messages.push(Message {
            role: MessageRole::Tool,
            content: if index == 4 {
                "latest verified evidence".to_string()
            } else {
                "older evidence ".repeat(4_000)
            },
            metadata: [("tool_call_id".to_string(), format!("call-{index}"))]
                .into_iter()
                .collect(),
        });
    }
    let canonical_history = runtime.messages.clone();

    let (request, report) =
        model_request_for_turn_with_context_budget(&mut runtime, &tools, None, None, 16_384, 2_048);

    assert!(report.applied);
    assert!(report.hard_limit_satisfied);
    assert!(report.omitted_messages > 0);
    assert!(request.messages.iter().any(|message| {
        message
            .content
            .contains("current goal: finish the verified implementation")
    }));
    assert!(request.messages.iter().any(|message| {
        message.metadata.get("kind").map(String::as_str) == Some("artifact_manifest")
    }));
    let assistant_index = request
        .messages
        .iter()
        .position(|message| {
            message
                .metadata
                .get("raw_tool_calls_json")
                .is_some_and(|value| value.contains("call-4"))
        })
        .expect("latest assistant tool call should remain");
    let tool_index = request
        .messages
        .iter()
        .position(|message| {
            message.metadata.get("tool_call_id").map(String::as_str) == Some("call-4")
        })
        .expect("latest tool evidence should remain");
    assert!(assistant_index < tool_index);
    assert_eq!(runtime.messages, canonical_history);
}

#[test]
fn routing_telemetry_is_reconstructed_from_completed_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = [
        ("agent_run_id".to_string(), "run-1".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::RetrievalPerformed,
        "RAG agent_context completed",
        run_context.clone(),
    )
    .expect("retrieval should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("total_tokens".to_string(), "120".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("model completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        run_context,
    )
    .expect("completion should append");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");

    let telemetry = routing_telemetry_from_events(&events);

    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].task_class, TaskClass::Coding);
    assert_eq!(telemetry[0].outcome, RoutingOutcome::Succeeded);
    assert_eq!(telemetry[0].cost_proxy, 120);
    assert_eq!(telemetry[0].retrieval_count, 1);
}

#[test]
fn goal2_routing_telemetry_learns_only_the_latest_replayed_decision() {
    let base = [
        ("agent_run_id".to_string(), "run-replanned".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let decision = |task_class: &str, policy: &str, model: &str, epoch: &str| {
        metadata_with_context(
            [
                ("task_class".to_string(), task_class.to_string()),
                ("collaboration_policy".to_string(), policy.to_string()),
                ("router_model".to_string(), model.to_string()),
                ("steer_epoch".to_string(), epoch.to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        )
    };
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        base.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        decision("general", "single", "stale-model", "0"),
    )
    .expect("stale decision should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "stale model finished",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "0".to_string()),
                ("total_tokens".to_string(), "100".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("stale cost should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        decision("coding", "plan_execute_review", "replanned-model", "1"),
    )
    .expect("replayed decision should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "current model finished",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "1".to_string()),
                ("total_tokens".to_string(), "7".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("current cost should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [("steer_epoch".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
            &base,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let telemetry = routing_telemetry_from_events(&events);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].task_class, TaskClass::Coding);
    assert_eq!(telemetry[0].selected_model, "replanned-model");
    assert_eq!(telemetry[0].cost_proxy, 7);
}

#[test]
fn goal2_semantic_memory_keeps_user_intent_lineage_and_only_terminal_epoch_outputs() {
    let base = [
        ("agent_run_id".to_string(), "memory-replanned".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let event = |sequence: u64, summary: &str, role: Option<&str>, epoch: &str| Event {
        id: EventId(format!("memory-epoch-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: if role.is_some() {
            EventKind::MessageAdded
        } else {
            EventKind::TaskStatusChanged
        },
        summary: summary.to_string(),
        metadata: metadata_with_context(
            [
                ("steer_epoch".to_string(), epoch.to_string()),
                ("role".to_string(), role.unwrap_or_default().to_string()),
                ("content".to_string(), format!("objective epoch {epoch}")),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    };
    let events = vec![
        event(1, "Old user objective", Some("user"), "0"),
        event(2, "Old assistant result", Some("assistant"), "0"),
        event(3, "Revised user objective", Some("user"), "1"),
        event(4, "Revised assistant result", Some("assistant"), "1"),
        event(5, "Agent task completed", None, "1"),
    ];

    let filtered = memory_events_for_terminal_steer_epoch(events);
    assert_eq!(filtered.len(), 4);
    assert!(filtered.iter().any(|event| {
        event.summary == "Old user objective"
            && event.metadata.get("steer_epoch").map(String::as_str) == Some("0")
    }));
    assert!(filtered.iter().any(|event| {
        event.summary == "Revised user objective"
            && event.metadata.get("steer_epoch").map(String::as_str) == Some("1")
    }));
    assert!(!filtered
        .iter()
        .any(|event| event.summary == "Old assistant result"));
}

#[test]
fn goal2_semantic_memory_preserves_legacy_runs_without_epochs() {
    let events = vec![Event {
        id: EventId("legacy-complete".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task completed".to_string(),
        metadata: Metadata::new(),
    }];
    assert_eq!(memory_events_for_terminal_steer_epoch(events).len(), 1);
}

#[test]
fn memory_checkpoints_reject_future_and_kind_mismatched_event_tags() {
    let event = |kind: EventKind, summary: &str, event_type: Option<&str>| Event {
        id: EventId(format!("memory-contract-{summary}")),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 10,
        kind,
        summary: summary.to_string(),
        metadata: event_type
            .map(|event_type| {
                [(EVENT_TYPE_METADATA_KEY.to_string(), event_type.to_string())]
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default(),
    };

    assert!(is_memory_checkpoint_event(&event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        None,
    )));
    assert!(!is_memory_checkpoint_event(&event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        Some("cindx.event.v2/agent.run.completed"),
    )));
    assert!(!is_memory_checkpoint_event(&event(
        EventKind::MessageAdded,
        "Agent task completed",
        Some(EventTypeV1::AgentRunCompleted.id()),
    )));
    assert!(!is_memory_checkpoint_event(&event(
        EventKind::TaskStatusChanged,
        "Semantic memory candidates accepted",
        Some("cindx.event.v2/memory.candidates.accepted"),
    )));

    assert!(contains_completed_agent_run(&[event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        None,
    )]));
    assert!(!contains_completed_agent_run(&[event(
        EventKind::TaskStatusChanged,
        "Agent task completed",
        Some("cindx.event.v2/agent.run.completed"),
    )]));
    assert!(!contains_completed_agent_run(&[event(
        EventKind::MessageAdded,
        "Agent task completed",
        Some(EventTypeV1::AgentRunCompleted.id()),
    )]));
}

#[test]
fn routing_telemetry_excludes_unverified_workspace_completions() {
    let run_context = [
        ("agent_run_id".to_string(), "run-unverified".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [
                (
                    "completion_evidence".to_string(),
                    "unverified_mutation".to_string(),
                ),
                ("routing_learning_eligible".to_string(), "false".to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    assert!(routing_telemetry_from_events(&events).is_empty());
}

#[test]
fn routing_telemetry_uses_collaboration_quality_gate_as_outcome() {
    let run_context = [
        ("agent_run_id".to_string(), "run-low-quality".to_string()),
        ("task_class".to_string(), "research".to_string()),
        ("collaboration_policy".to_string(), "best_of_n".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Collaboration quality gate evaluated",
        metadata_with_context(
            [
                ("quality_pass".to_string(), "false".to_string()),
                ("quality_score".to_string(), "0.61".to_string()),
                ("safety_violations".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("quality gate should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [("routing_learning_eligible".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let telemetry = routing_telemetry_from_events(&events);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].outcome, RoutingOutcome::Failed);
    assert_eq!(telemetry[0].quality_score, Some(0.61));
    assert_eq!(telemetry[0].verification_passed, Some(false));
}

#[test]
fn routing_telemetry_excludes_unverified_anytime_delivery() {
    let run_context = [
        ("agent_run_id".to_string(), "run-anytime-draft".to_string()),
        ("task_class".to_string(), "research".to_string()),
        ("collaboration_policy".to_string(), "best_of_n".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Collaboration workflow completed",
        metadata_with_context(
            [
                (
                    "anytime_selected_candidate".to_string(),
                    DIRECT_ANCHOR_CANDIDATE_ID.to_string(),
                ),
                ("anytime_selected_verified".to_string(), "false".to_string()),
                (
                    "anytime_selected_quality_bps".to_string(),
                    "6500".to_string(),
                ),
                (
                    "anytime_routing_learning_eligible".to_string(),
                    "false".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("workflow completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [("routing_learning_eligible".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    assert!(routing_telemetry_from_events(&events).is_empty());
}

#[test]
fn completion_learning_signal_requires_post_mutation_verification() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "update the workspace",
        AgentRuntimeConfig::default(),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("non_mutating", false)
    );

    record_tool_outcome_with_risk(
        &mut runtime,
        "file.write",
        r#"{"path":"src/lib.rs"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::WritesWorkspace),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("unverified_mutation", false)
    );

    record_tool_outcome_with_risk(
        &mut runtime,
        "process.run",
        r#"{"command":"cargo test"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ExecutesProcess),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("unverified_mutation", false)
    );

    record_tool_outcome_with_risk(
        &mut runtime,
        "file.read",
        r#"{"path":"src/lib.rs"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ReadOnly),
    );
    assert_eq!(
        completion_learning_signal(&runtime),
        ("verified_mutation", true)
    );
}

#[test]
fn workflow_telemetry_restores_versioned_plan_and_quality() {
    let models = vec!["planner".to_string(), "reviewer".to_string()];
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "first".to_string(),
                role: "thinker".to_string(),
                model: "planner".to_string(),
                subtask: "independent analysis".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "second".to_string(),
                role: "worker".to_string(),
                model: "reviewer".to_string(),
                subtask: "alternative analysis".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "planner".to_string(),
                subtask: "synthesize both branches".to_string(),
                access: vec!["first".to_string(), "second".to_string()],
            },
        ],
    };
    let plan = WorkflowPlanIr::from_adaptive(
        "collab-1",
        "Compare approaches",
        "pro",
        "best_of_n",
        "planner",
        &workflow,
        WorkflowBudget {
            max_steps: 3,
            max_models: 2,
            max_model_turns_per_step: 5,
            max_tool_calls_per_step: 6,
            max_output_tokens_per_step: 4_096,
        },
    );
    let context = [
        ("collaboration_id".to_string(), "collab-1".to_string()),
        ("task_class".to_string(), "research".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut events = vec![
        Event {
            id: EventId("planned".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow planned".to_string(),
            metadata: metadata_with_context(
                [("workflow_ir".to_string(), plan.to_json().unwrap())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("model".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::ModelRequestFinished,
            summary: "Collaboration worker finished".to_string(),
            metadata: metadata_with_context(
                [("total_tokens".to_string(), "640".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("quality".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 250,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration quality gate evaluated".to_string(),
            metadata: metadata_with_context(
                [("quality_score".to_string(), "0.875".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("tool".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 275,
            kind: EventKind::ToolCallFinished,
            summary: "Tool call finished: file.search".to_string(),
            metadata: metadata_with_context(
                [
                    ("stage".to_string(), "worker_1".to_string()),
                    ("tool".to_string(), "file.search".to_string()),
                    ("status".to_string(), "succeeded".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("completed".to_string()),
            task_id: phase16_task_id(),
            sequence: 5,
            timestamp_ms: 500,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: metadata_with_context(
                [
                    ("fallback_used".to_string(), "false".to_string()),
                    ("anytime_team_score_bps".to_string(), "7400".to_string()),
                    ("anytime_anchor_score_bps".to_string(), "8000".to_string()),
                    ("anytime_team_uplift_bps".to_string(), "-600".to_string()),
                    (
                        "anytime_selected_kind".to_string(),
                        "direct_anchor".to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    ];
    events.insert(
        events.len() - 1,
        Event {
            id: EventId("anchor".to_string()),
            task_id: phase16_task_id(),
            sequence: 4,
            timestamp_ms: 350,
            kind: EventKind::ModelRequestFinished,
            summary: "Collaboration direct_anchor finished".to_string(),
            metadata: metadata_with_context(
                [
                    ("stage".to_string(), "direct_anchor".to_string()),
                    ("latency_ms".to_string(), "120".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    );

    let telemetry = workflow_execution_telemetry_from_events(&events, &models);

    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].plan.schema, WORKFLOW_IR_SCHEMA);
    assert_eq!(telemetry[0].task_class, TaskClass::Research);
    assert_eq!(telemetry[0].quality_score, Some(0.875));
    assert_eq!(telemetry[0].latency_ms, 400);
    assert_eq!(telemetry[0].total_tokens, 640);
    assert_eq!(telemetry[0].tool_calls, 1);
    assert_eq!(telemetry[0].paired_team_score_bps, Some(7_400));
    assert_eq!(telemetry[0].paired_anchor_score_bps, Some(8_000));
    assert_eq!(telemetry[0].paired_uplift_bps, Some(-600));
    assert!(telemetry[0].selected_anchor);
    assert_eq!(telemetry[0].anchor_latency_ms, Some(120));
    assert_eq!(
        telemetry[0].successful_tools_by_step.get("first"),
        Some(&vec!["file.search".to_string()])
    );
    assert!(telemetry[0].succeeded);

    let mut store = SqliteStore::in_memory().expect("workflow telemetry store should open");
    for event in &events {
        append_event(
            &mut store,
            &phase16_task_id(),
            event.kind.clone(),
            event.summary.clone(),
            event.metadata.clone(),
        )
        .expect("workflow event should append");
    }
    let initial = load_workflow_telemetry_read_model(&mut store, &models)
        .expect("workflow telemetry read model should build");
    assert_eq!(initial.len(), 1);
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "unrelated follow-up",
        [("session_id".to_string(), "session-unrelated".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("unrelated event should append");
    let updated = load_workflow_telemetry_read_model(&mut store, &models)
        .expect("workflow telemetry read model should advance incrementally");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].plan.workflow_id, "collab-1");

    events.last_mut().unwrap().metadata.insert(
        "anytime_prompt_learning_eligible".to_string(),
        "false".to_string(),
    );
    let censored = workflow_execution_telemetry_from_events(&events, &models);
    assert_eq!(censored.len(), 1);
    assert!(!censored[0].learning_evidence.is_learnable());
}

#[test]
fn workflow_checkpoint_resume_is_scoped_to_the_latest_user_turn_and_terminal_snapshot() {
    let models = vec!["planner".to_string()];
    let prompt = "Implement and verify the change";
    let plan = WorkflowPlanIr::from_adaptive(
        "collab-resume",
        prompt,
        "pro",
        "best_of_n",
        "planner",
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "planner".to_string(),
                subtask: "produce verified guidance".to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 2,
            max_output_tokens_per_step: 2_048,
        },
    );
    let mut events = vec![Event {
        id: EventId("user-1".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), prompt.to_string()),
        ]
        .into_iter()
        .collect(),
    }];
    let resume_key =
        workflow_resume_key_from_events(&events, Some("session-a"), prompt, "pro", "best_of_n");
    let mut checkpoint = WorkflowExecutionCheckpoint::new(&resume_key, plan, 110);
    events.push(Event {
        id: EventId("checkpoint-running".to_string()),
        task_id: phase16_task_id(),
        sequence: 2,
        timestamp_ms: 120,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow checkpoint created".to_string(),
        metadata: [
            ("workflow_resume_key".to_string(), resume_key.clone()),
            (
                "workflow_checkpoint".to_string(),
                checkpoint.to_json().unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    assert!(
        resumable_workflow_checkpoint_from_events(&events, &resume_key, prompt, &models,).is_some()
    );

    checkpoint.begin_step("final", "planner", 130).unwrap();
    checkpoint
        .complete_step(
            "final",
            "planner",
            "draft".to_string(),
            "[]".to_string(),
            140,
        )
        .unwrap();
    checkpoint.finalize("verified".to_string(), 150).unwrap();
    events.push(Event {
        id: EventId("checkpoint-final".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 150,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow checkpoint finalized".to_string(),
        metadata: [
            ("workflow_resume_key".to_string(), resume_key.clone()),
            (
                "workflow_checkpoint".to_string(),
                checkpoint.to_json().unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    assert!(
        resumable_workflow_checkpoint_from_events(&events, &resume_key, prompt, &models,).is_none()
    );

    events.push(Event {
        id: EventId("continuation-replay".to_string()),
        task_id: phase16_task_id(),
        sequence: 4,
        timestamp_ms: 155,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), prompt.to_string()),
            ("continuation_replay".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect(),
    });
    let continuation_key =
        workflow_resume_key_from_events(&events, Some("session-a"), prompt, "pro", "best_of_n");
    assert_eq!(resume_key, continuation_key);

    events.push(Event {
        id: EventId("user-2".to_string()),
        task_id: phase16_task_id(),
        sequence: 5,
        timestamp_ms: 160,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), prompt.to_string()),
        ]
        .into_iter()
        .collect(),
    });
    let repeated_prompt_key =
        workflow_resume_key_from_events(&events, Some("session-a"), prompt, "pro", "best_of_n");
    assert_ne!(resume_key, repeated_prompt_key);
}

#[test]
fn prompt_evolution_uses_training_results_to_select_a_new_generation() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let genome_json = serde_json::to_string(&seed).expect("genome should serialize");
    let mut events = Vec::new();
    for run_index in 0..6u64 {
        let collaboration_id = format!("evolution-{run_index}");
        let plan = WorkflowPlanIr::from_adaptive_with_profile(
            collaboration_id.clone(),
            "Implement and verify a change",
            "auto",
            "best_of_n",
            "planner",
            seed.id.clone(),
            &AdaptiveWorkflow {
                steps: vec![AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "planner".to_string(),
                    subtask: "produce the verified result".to_string(),
                    access: Vec::new(),
                }],
            },
            WorkflowBudget {
                max_steps: 3,
                max_models: 2,
                max_model_turns_per_step: 5,
                max_tool_calls_per_step: 6,
                max_output_tokens_per_step: 4_096,
            },
        );
        let context = [
            ("collaboration_id".to_string(), collaboration_id),
            ("prompt_profile".to_string(), seed.id.clone()),
            ("prompt_effort".to_string(), "auto".to_string()),
            ("prompt_genome".to_string(), genome_json.clone()),
            ("task_class".to_string(), "coding".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let sequence = run_index * 4 + 1;
        events.push(Event {
            id: EventId(format!("selected-{run_index}")),
            task_id: phase16_task_id(),
            sequence,
            timestamp_ms: 1_000 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        });
        events.push(Event {
            id: EventId(format!("planned-{run_index}")),
            task_id: phase16_task_id(),
            sequence: sequence + 1,
            timestamp_ms: 1_020 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow planned".to_string(),
            metadata: metadata_with_context(
                [("workflow_ir".to_string(), plan.to_json().unwrap())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        });
        events.push(Event {
            id: EventId(format!("quality-{run_index}")),
            task_id: phase16_task_id(),
            sequence: sequence + 2,
            timestamp_ms: 1_040 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration quality gate evaluated".to_string(),
            metadata: metadata_with_context(
                [("quality_score".to_string(), "0.9".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        });
        events.push(Event {
            id: EventId(format!("completed-{run_index}")),
            task_id: phase16_task_id(),
            sequence: sequence + 3,
            timestamp_ms: 1_080 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: context,
        });
    }

    let live_only =
        evaluate_prompt_evolution(&events, "auto").expect("live prompt outcomes should evaluate");
    assert!(!live_only.frontier_ids.contains(&seed.id));
    assert!(live_only.champion_id.is_none());

    for evaluation_index in 0..14u64 {
        let mode = if evaluation_index < 6 {
            PromptEvaluationMode::PairedExecution
        } else {
            PromptEvaluationMode::ReplayExecution
        };
        let split = if mode.is_replay() {
            PromptEvaluationSplit::Holdout
        } else {
            PromptEvaluationSplit::Train
        };
        let reflection_packet = (mode == PromptEvaluationMode::PairedExecution).then(|| {
            AgentEvaluationReflectionPacket {
                suite_id: "runtime-prompt-evolution".to_string(),
                suite_version: 2,
                case_id: format!("case-{evaluation_index}"),
                category: "coding".to_string(),
                run_id: format!("pair-{evaluation_index}"),
                seed: evaluation_index,
                candidate_id: seed.id.clone(),
                candidate_fingerprint: "seed-fingerprint".to_string(),
                model_fingerprints: BTreeMap::new(),
                input: "Implement and verify a change".to_string(),
                steps: Vec::new(),
                final_output: "verified".to_string(),
                verifier: AgentEvaluationVerifierOutcome {
                    source: AgentEvaluationEvidenceSource::Judge,
                    passed: true,
                    score: 0.9,
                    checks: Vec::new(),
                },
                actionable_feedback: ActionableSideInformation {
                    summary: "preserve verification coverage".to_string(),
                    ..ActionableSideInformation::default()
                },
            }
        });
        let evaluation_id =
            scoped_prompt_evaluation_id("project-a", &format!("pair-{evaluation_index}"));
        let observation = PromptEvolutionObservation {
            profile_id: seed.id.clone(),
            evaluation_id,
            case_id: format!("case-{evaluation_index}"),
            opponent_profile_id: Some("baseline-opponent".to_string()),
            task_class: "coding".to_string(),
            split,
            mode,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 1_000,
            total_tokens: 800,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.4),
            step_credits: vec![PromptStepCredit {
                step_id: "final".to_string(),
                role: "synthesizer".to_string(),
                succeeded: true,
                attempts: 1,
                evidence_count: 1,
                latency_ms: 1_000,
                total_tokens: 800,
                credit: 0.9,
            }],
            reflection_packet,
            provenance: test_prompt_evaluation_provenance(&seed.id, "baseline-opponent"),
        };
        let mut opponent_observation = observation.clone();
        opponent_observation.profile_id = "baseline-opponent".to_string();
        opponent_observation.opponent_profile_id = Some(seed.id.clone());
        opponent_observation.quality_score = 0.7;
        opponent_observation.relative_reward = Some(-0.4);
        opponent_observation.step_credits.clear();
        opponent_observation.reflection_packet = None;
        opponent_observation.provenance =
            test_prompt_evaluation_provenance("baseline-opponent", &seed.id);
        events.push(Event {
            id: EventId(format!("pair-event-{evaluation_index}")),
            task_id: phase16_task_id(),
            sequence: 100 + evaluation_index,
            timestamp_ms: 10_000 + evaluation_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor pairwise evaluation".to_string(),
            metadata: [
                ("prompt_effort".to_string(), "auto".to_string()),
                ("project_id".to_string(), "project-a".to_string()),
                (
                    "prompt_observations".to_string(),
                    serde_json::to_string(&[observation, opponent_observation]).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    let evaluation = evaluate_prompt_evolution(&events, "auto")
        .expect("paired and replay prompt outcomes should evaluate");
    let seed_observations = evaluation
        .observations
        .iter()
        .filter(|observation| observation.profile_id == seed.id)
        .collect::<Vec<_>>();

    assert_eq!(seed_observations.len(), 14);
    assert_eq!(
        seed_observations
            .iter()
            .filter(|observation| observation.mode == PromptEvaluationMode::PairedExecution)
            .count(),
        6
    );
    assert_eq!(
        seed_observations
            .iter()
            .filter(|observation| observation.mode == PromptEvaluationMode::ReplayExecution)
            .count(),
        8
    );
    assert!(evaluation.frontier_ids.contains(&seed.id));
    assert_eq!(evaluation.next_profile.generation, 1);
    assert_ne!(evaluation.next_profile.id, seed.id);
    assert_eq!(
        evaluation
            .mutation_parent
            .as_ref()
            .map(|genome| genome.id.as_str()),
        Some(seed.id.as_str())
    );
    assert_eq!(evaluation.mutation_trajectories.len(), 6);
    assert!(evaluation
        .mutation_trajectories
        .iter()
        .all(|packet| packet.candidate_id == seed.id));
}

#[test]
fn replay_holdout_uses_only_a_different_completed_workflow() {
    let workflow = |id: &str, objective: &str| {
        WorkflowPlanIr::from_adaptive_with_profile(
            id.to_string(),
            objective,
            "auto",
            "best_of_n",
            "planner",
            "seed-auto-v1",
            &AdaptiveWorkflow {
                steps: vec![AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "planner".to_string(),
                    subtask: "finish".to_string(),
                    access: Vec::new(),
                }],
            },
            WorkflowBudget {
                max_steps: 4,
                max_models: 2,
                max_model_turns_per_step: 3,
                max_tool_calls_per_step: 4,
                max_output_tokens_per_step: 2_048,
            },
        )
    };
    let planned = |sequence, id: &str, objective: &str| Event {
        id: EventId(format!("planned-{id}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow planned".to_string(),
        metadata: [
            ("collaboration_id".to_string(), id.to_string()),
            (
                "workflow_ir".to_string(),
                workflow(id, objective).to_json().unwrap(),
            ),
            ("task_class".to_string(), "coding".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let completed = |sequence, id: &str| Event {
        id: EventId(format!("completed-{id}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow completed".to_string(),
        metadata: [("collaboration_id".to_string(), id.to_string())]
            .into_iter()
            .collect(),
    };
    let events = vec![
        planned(1, "current", "Current task"),
        completed(2, "current"),
        planned(3, "holdout", "Older completed task"),
        completed(4, "holdout"),
        planned(5, "failed", "Failed historical task"),
    ];

    let replay = prompt_replay_case(&events, "Current task", 0).unwrap();

    assert_eq!(replay.objective, "Older completed task");
    assert_eq!(replay.task_class, "coding");
}

#[test]
fn offline_prompt_evidence_rejects_invalid_agent_boundaries() {
    let run_events = |run_id: &str, start_tag: Option<&str>, terminal_tag: Option<&str>| {
        let metadata = [
            ("agent_run_id".to_string(), run_id.to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            ("prompt".to_string(), "Audit the agent loop".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut started_metadata = metadata.clone();
        if let Some(start_tag) = start_tag {
            started_metadata.insert(EVENT_TYPE_METADATA_KEY.to_string(), start_tag.to_string());
        }
        let mut terminal_metadata = metadata;
        if let Some(terminal_tag) = terminal_tag {
            terminal_metadata.insert(
                EVENT_TYPE_METADATA_KEY.to_string(),
                terminal_tag.to_string(),
            );
        }
        vec![
            Event {
                id: EventId(format!("started-{run_id}")),
                task_id: phase16_task_id(),
                sequence: 1,
                timestamp_ms: 10,
                kind: EventKind::TaskStatusChanged,
                summary: "Agent task started".to_string(),
                metadata: started_metadata,
            },
            Event {
                id: EventId(format!("completed-{run_id}")),
                task_id: phase16_task_id(),
                sequence: 2,
                timestamp_ms: 20,
                kind: EventKind::TaskStatusChanged,
                summary: "Agent task completed".to_string(),
                metadata: terminal_metadata,
            },
        ]
    };

    for events in [
        run_events(
            "future-start",
            Some("cindx.event.v2/agent.run.started"),
            None,
        ),
        run_events(
            "future-terminal",
            None,
            Some("cindx.event.v2/agent.run.completed"),
        ),
        run_events(
            "mismatched-terminal",
            None,
            Some(EventTypeV1::AgentRunFailed.id()),
        ),
    ] {
        assert!(prompt_offline_dataset(&events, "project-a", None).is_empty());
    }
}

#[test]
fn replay_prompt_evidence_rejects_invalid_agent_completion_tags() {
    let profile = Event {
        id: EventId("bounded-profile-invalid-terminal".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor prompt profile selected".to_string(),
        metadata: [
            ("agent_run_id".to_string(), "run-invalid".to_string()),
            ("collaboration_profile".to_string(), "bounded".to_string()),
            (
                "prompt_objective".to_string(),
                "Completed bounded task".to_string(),
            ),
            ("task_class".to_string(), "coding".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    for invalid_tag in [
        "cindx.event.v2/agent.run.completed",
        EventTypeV1::AgentRunFailed.id(),
    ] {
        let terminal = Event {
            id: EventId(format!("invalid-terminal-{invalid_tag}")),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task completed".to_string(),
            metadata: [
                ("agent_run_id".to_string(), "run-invalid".to_string()),
                (EVENT_TYPE_METADATA_KEY.to_string(), invalid_tag.to_string()),
            ]
            .into_iter()
            .collect(),
        };

        assert!(prompt_replay_case(&[profile.clone(), terminal], "Current task", 0).is_none());
    }
}

#[test]
fn offline_prompt_dataset_is_project_scoped_deterministic_and_split_stable() {
    let run_events = |sequence: u64,
                      run_id: &str,
                      project_id: &str,
                      objective: &str,
                      terminal_summary: Option<&str>| {
        let metadata = [
            ("agent_run_id".to_string(), run_id.to_string()),
            ("project_id".to_string(), project_id.to_string()),
            ("session_id".to_string(), format!("session-{run_id}")),
            ("task_class".to_string(), "coding".to_string()),
            ("prompt".to_string(), objective.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut events = vec![Event {
            id: EventId(format!("started-{run_id}")),
            task_id: phase16_task_id(),
            sequence,
            timestamp_ms: sequence * 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata.clone(),
        }];
        if let Some(terminal_summary) = terminal_summary {
            events.push(Event {
                id: EventId(format!("completed-{run_id}")),
                task_id: phase16_task_id(),
                sequence: sequence + 1,
                timestamp_ms: (sequence + 1) * 10,
                kind: EventKind::TaskStatusChanged,
                summary: terminal_summary.to_string(),
                metadata,
            });
        }
        events
    };
    let mut events = Vec::new();
    events.extend(run_events(
        1,
        "a",
        "project-a",
        "Audit the agent loop",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        3,
        "b",
        "project-a",
        "Improve retrieval fusion",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        5,
        "c",
        "project-a",
        "Verify queue steering",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        7,
        "failed",
        "project-a",
        "Recover a failed long-running task",
        Some("Agent task failed"),
    ));
    events.extend(run_events(
        9,
        "cancelled",
        "project-a",
        "Preserve state after cancellation",
        Some("Agent task cancelled"),
    ));
    events.extend(run_events(
        11,
        "other",
        "project-b",
        "Unrelated project",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        13,
        "incomplete",
        "project-a",
        "Incomplete run",
        None,
    ));

    let first = prompt_offline_dataset(&events, "project-a", None);
    let second = prompt_offline_dataset(&events, "project-a", None);

    assert_eq!(first, second);
    assert_eq!(first.len(), 5);
    assert!(first.iter().all(|case| case.project_id == "project-a"));
    assert!(
        first
            .iter()
            .filter(|case| case.split == PromptEvaluationSplit::Train)
            .count()
            >= 2
    );
    assert!(
        first
            .iter()
            .filter(|case| case.split == PromptEvaluationSplit::Holdout)
            .count()
            >= 2
    );
    assert!(!first.iter().any(|case| case.objective == "Incomplete run"));
    assert!(first
        .iter()
        .any(|case| case.objective == "Recover a failed long-running task"));
    assert!(first
        .iter()
        .any(|case| case.objective == "Preserve state after cancellation"));

    let split_manifest = first
        .iter()
        .map(|case| (case.id.clone(), case.split))
        .collect::<BTreeMap<_, _>>();
    events.push(Event {
        id: EventId("offline-split-snapshot".to_string()),
        task_id: phase16_task_id(),
        sequence: 15,
        timestamp_ms: 150,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor offline dataset selected".to_string(),
        metadata: [
            ("project_id".to_string(), "project-a".to_string()),
            (
                "dataset_split_manifest".to_string(),
                serde_json::to_string(&split_manifest).unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    events.extend(run_events(
        16,
        "d",
        "project-a",
        "Check recovery checkpoints",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        18,
        "e",
        "project-a",
        "Review quality gates",
        Some("Agent task completed"),
    ));

    let grown = prompt_offline_dataset(&events, "project-a", None);
    for original in &first {
        assert_eq!(
            grown
                .iter()
                .find(|case| case.id == original.id)
                .map(|case| case.split),
            Some(original.split),
            "existing offline split must not drift as the dataset grows"
        );
    }
}

#[test]
fn offline_prompt_dataset_stratifies_task_classes_without_split_drift() {
    let mut events = Vec::new();
    for (index, (run_id, task_class)) in [
        ("coding-a", "coding"),
        ("coding-b", "coding"),
        ("research-a", "research"),
        ("research-b", "research"),
    ]
    .into_iter()
    .enumerate()
    {
        let sequence = index as u64 * 2 + 1;
        let metadata = [
            ("agent_run_id".to_string(), run_id.to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            ("session_id".to_string(), format!("session-{run_id}")),
            ("task_class".to_string(), task_class.to_string()),
            ("prompt".to_string(), format!("Evaluate {run_id}")),
        ]
        .into_iter()
        .collect::<Metadata>();
        events.push(Event {
            id: EventId(format!("start-{run_id}")),
            task_id: phase16_task_id(),
            sequence,
            timestamp_ms: sequence * 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata.clone(),
        });
        events.push(Event {
            id: EventId(format!("finish-{run_id}")),
            task_id: phase16_task_id(),
            sequence: sequence + 1,
            timestamp_ms: (sequence + 1) * 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task completed".to_string(),
            metadata,
        });
    }

    let dataset = prompt_offline_dataset(&events, "project-a", None);

    for task_class in ["coding", "research"] {
        let splits = dataset
            .iter()
            .filter(|case| case.task_class == task_class)
            .map(|case| case.split)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            splits,
            BTreeSet::from([PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout,])
        );
    }
}

#[test]
fn offline_prompt_dataset_uses_independent_pre_decision_task_class() {
    let metadata = [
        ("agent_run_id".to_string(), "independent-class".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        (
            "prompt".to_string(),
            "Investigate competing evidence".to_string(),
        ),
        (
            "prompt_objective".to_string(),
            "Investigate competing evidence".to_string(),
        ),
        ("task_class".to_string(), "coding".to_string()),
        (
            "pre_decision_task_class".to_string(),
            "research".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = ["Agent run decision selected", "Agent task completed"]
        .into_iter()
        .enumerate()
        .map(|(index, summary)| Event {
            id: EventId(format!("independent-class-{index}")),
            task_id: phase16_task_id(),
            sequence: index as u64 + 1,
            timestamp_ms: index as u64 + 1,
            kind: EventKind::TaskStatusChanged,
            summary: summary.to_string(),
            metadata: metadata.clone(),
        })
        .collect::<Vec<_>>();

    let dataset = prompt_offline_dataset(&events, "project-a", None);

    assert_eq!(dataset.len(), 1);
    assert_eq!(dataset[0].task_class, "research");
}

#[test]
fn goal2_offline_prompt_dataset_uses_only_the_terminal_steer_epoch() {
    let base = [
        ("agent_run_id".to_string(), "replayed-run".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let event = |sequence: u64, summary: &str, metadata: Metadata| Event {
        id: EventId(format!("epoch-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: EventKind::TaskStatusChanged,
        summary: summary.to_string(),
        metadata: metadata_with_context(metadata, &base),
    };
    let events = vec![
        event(
            1,
            "Agent task started",
            [
                ("prompt".to_string(), "Old coding objective".to_string()),
                ("task_class".to_string(), "coding".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            2,
            "Agent run decision selected",
            [
                ("steer_epoch".to_string(), "0".to_string()),
                ("prompt_objective".to_string(), "Old coding objective".to_string()),
                ("task_class".to_string(), "coding".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            3,
            "Agent run decision selected",
            [
                ("steer_epoch".to_string(), "1".to_string()),
                (
                    "prompt_objective".to_string(),
                    "Research the revised objective".to_string(),
                ),
                (
                    "effective_prompt_objective".to_string(),
                    "Initial request:\nOld coding objective\n\nAccepted steering 1:\nResearch the revised objective"
                        .to_string(),
                ),
                ("task_class".to_string(), "research".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            4,
            "Agent task completed",
            [("steer_epoch".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
        ),
    ];

    let dataset = prompt_offline_dataset(&events, "project-a", None);
    assert_eq!(dataset.len(), 1);
    assert_eq!(
        dataset[0].objective,
        "Initial request:\nOld coding objective\n\nAccepted steering 1:\nResearch the revised objective"
    );
    assert_eq!(dataset[0].task_class, "research");
}

#[test]
fn goal2_offline_prompt_dataset_reconstructs_initial_and_accepted_steer_intent() {
    let base = [
        ("agent_run_id".to_string(), "lineage-run".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let event = |sequence: u64, kind: EventKind, summary: &str, metadata: Metadata| Event {
        id: EventId(format!("lineage-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind,
        summary: summary.to_string(),
        metadata: metadata_with_context(metadata, &base),
    };
    let events = vec![
        event(
            1,
            EventKind::TaskStatusChanged,
            "Agent task started",
            [
                (
                    "prompt".to_string(),
                    "Keep all checks and run the full test suite".to_string(),
                ),
                ("steer_epoch".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            2,
            EventKind::MessageAdded,
            "Initial user objective",
            [
                ("role".to_string(), "user".to_string()),
                (
                    "content".to_string(),
                    "Keep all checks and run the full test suite".to_string(),
                ),
                ("steer_epoch".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            3,
            EventKind::MessageAdded,
            "Accepted user steer",
            [
                ("role".to_string(), "user".to_string()),
                (
                    "content".to_string(),
                    "Continue after fixing it".to_string(),
                ),
                ("queue_mode".to_string(), "steer".to_string()),
                ("steer_epoch".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            4,
            EventKind::TaskStatusChanged,
            "Agent run decision selected",
            [
                ("steer_epoch".to_string(), "1".to_string()),
                ("prompt_objective".to_string(), "Continue".to_string()),
                ("task_class".to_string(), "coding".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            5,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            [("steer_epoch".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
        ),
    ];

    let dataset = prompt_offline_dataset(&events, "project-a", None);
    assert_eq!(dataset.len(), 1);
    assert_eq!(
        dataset[0].objective,
        "Initial request:\nKeep all checks and run the full test suite\n\nAccepted steering 1:\nContinue after fixing it"
    );
}

#[test]
fn offline_prompt_dataset_stays_frozen_within_a_generation() {
    let discovered = ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|id| PromptOfflineCase {
            id: id.to_string(),
            objective: format!("Evaluate {id}"),
            task_class: "coding".to_string(),
            project_id: "project-a".to_string(),
            source_run_id: format!("run-{id}"),
            split: PromptEvaluationSplit::Train,
            learning_receipt: None,
            auto_teacher: None,
        })
        .collect::<Vec<_>>();
    let frozen_ids = discovered
        .iter()
        .take(4)
        .map(|case| case.id.clone())
        .collect::<Vec<_>>();
    let frozen_identity = prompt_dataset_identity(&discovered[..4], 2).unwrap();
    let previous = PromptOfflineDatasetState {
        effort: "auto".to_string(),
        project_id: "project-a".to_string(),
        digest: frozen_identity.dataset_sha256.clone(),
        identity: Some(frozen_identity),
        generation: 2,
        case_ids: frozen_ids.clone(),
        case_count: frozen_ids.len(),
        train_count: frozen_ids.len(),
        holdout_count: 0,
        selected_case_id: None,
        status: "ready".to_string(),
        updated_at_ms: 1,
    };

    let same_generation =
        prompt_offline_dataset_for_generation(discovered.clone(), Some(&previous), 2).unwrap();
    let next_generation =
        prompt_offline_dataset_for_generation(discovered.clone(), Some(&previous), 3).unwrap();

    assert_eq!(
        same_generation
            .iter()
            .map(|case| case.id.clone())
            .collect::<Vec<_>>(),
        frozen_ids
    );
    assert_eq!(next_generation, discovered);

    let incomplete = discovered
        .iter()
        .filter(|case| case.id != frozen_ids[0])
        .cloned()
        .collect::<Vec<_>>();
    assert!(prompt_offline_dataset_for_generation(incomplete, Some(&previous), 2).is_err());

    let assert_identity_change_is_rejected = |changed| {
        assert_eq!(
            prompt_offline_dataset_for_generation(changed, Some(&previous), 2).unwrap_err(),
            "frozen prompt dataset identity changed; refusing cohort substitution"
        );
    };
    let mut changed_objective = discovered.clone();
    changed_objective[0].objective = "Different objective".to_string();
    assert_identity_change_is_rejected(changed_objective);

    let mut changed_task_family = discovered.clone();
    changed_task_family[0].task_class = "analysis".to_string();
    assert_identity_change_is_rejected(changed_task_family);

    let mut changed_split = discovered.clone();
    changed_split[0].split = PromptEvaluationSplit::Holdout;
    assert_identity_change_is_rejected(changed_split);

    let mut changed_case_id = discovered.clone();
    changed_case_id[0].id = "different-case".to_string();
    assert_eq!(
        prompt_offline_dataset_for_generation(changed_case_id, Some(&previous), 2).unwrap_err(),
        "frozen prompt dataset is incomplete; refusing cohort substitution"
    );

    let mut legacy_snapshot = previous;
    legacy_snapshot.identity = None;
    assert_eq!(
        prompt_offline_dataset_for_generation(discovered.clone(), Some(&legacy_snapshot), 2,)
            .unwrap(),
        discovered
    );
}

#[test]
fn offline_prompt_dataset_digest_tracks_the_frozen_cohort_not_source_runs() {
    let case = |source_run_id: &str, split| PromptOfflineCase {
        id: "coding-a".to_string(),
        objective: "Evaluate coding-a".to_string(),
        task_class: "coding".to_string(),
        project_id: "project-a".to_string(),
        source_run_id: source_run_id.to_string(),
        split,
        learning_receipt: None,
        auto_teacher: None,
    };
    let original = vec![case("run-a", PromptEvaluationSplit::Train)];
    let repeated_source = vec![case("run-b", PromptEvaluationSplit::Train)];
    let changed_split = vec![case("run-a", PromptEvaluationSplit::Holdout)];

    assert_eq!(
        prompt_offline_dataset_digest(&original),
        prompt_offline_dataset_digest(&repeated_source)
    );
    assert_ne!(
        prompt_offline_dataset_digest(&original),
        prompt_offline_dataset_digest(&changed_split)
    );
}

fn test_auto_teacher_case(output: &str) -> PromptAutoTeacherCase {
    let genome = ConductorPromptGenome::seed_for_effort("auto");
    let plan = WorkflowPlanIr::from_adaptive_with_profile(
        "auto-workflow".to_string(),
        "Evaluate a completed Auto workflow",
        "auto",
        "adaptive",
        "auto-worker",
        &genome.id,
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "auto-worker".to_string(),
                subtask: "Produce the verified answer".to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 2,
            max_models: 1,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 2,
            max_output_tokens_per_step: 1_024,
        },
    );
    PromptAutoTeacherCase {
        source_run_id: "auto-run".to_string(),
        steer_epoch: 0,
        profile_id: genome.id.clone(),
        profile_sha256: prompt_genome_sha256(&genome).unwrap(),
        output_sha256: sha256_hex(output.as_bytes()),
        source_context: orchestrator::AutoTeacherSourceContextV1 {
            schema: orchestrator::AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            system_prompt_sha256: "3".repeat(64),
            policy_sha256: "4".repeat(64),
            budget_sha256: "5".repeat(64),
            tool_contract_sha256: "6".repeat(64),
            source_revision_sha256: "7".repeat(64),
            workspace_revision_sha256: "8".repeat(64),
            evaluator_identity_sha256: "9".repeat(64),
            evaluator_receipt_sha256: "a".repeat(64),
            checkpoint_sha256: "b".repeat(64),
            learning_receipt_sha256: "c".repeat(64),
        },
        genome,
        plan,
        steps: vec![PromptAutoTeacherStep {
            id: "final".to_string(),
            role: "synthesizer".to_string(),
            model: "auto-worker".to_string(),
            attempts: 1,
            status: WorkflowStepStatus::Completed,
            output: output.to_string(),
            errors: Vec::new(),
            latency_ms: 10,
            total_tokens: 100,
            evidence_count: 1,
        }],
        final_output: output.to_string(),
        participant_models: vec!["auto-worker".to_string()],
        quality_score_bps: 9_000,
        latency_ms: 20,
        total_tokens: 100,
    }
}

#[test]
fn current_stable_auto_teacher_outranks_stale_higher_score() {
    let case = |profile_id: &str, profile_sha256: &str, quality_score_bps: u16| {
        let mut teacher = test_auto_teacher_case("verified output");
        teacher.profile_id = profile_id.to_string();
        teacher.profile_sha256 = profile_sha256.to_string();
        teacher.quality_score_bps = quality_score_bps;
        PromptOfflineCase {
            id: "same-case".to_string(),
            objective: "same objective".to_string(),
            task_class: "coding".to_string(),
            project_id: "project-a".to_string(),
            source_run_id: teacher.source_run_id.clone(),
            split: PromptEvaluationSplit::Train,
            learning_receipt: None,
            auto_teacher: Some(teacher),
        }
    };
    let current = case("auto-current", "current-sha", 8_500);
    let stale = case("auto-stale", "stale-sha", 9_900);
    let preferred = Some(("auto-current", "current-sha"));

    assert!(
        prompt_offline_case_rank(&current, preferred) > prompt_offline_case_rank(&stale, preferred)
    );
    assert!(
        prompt_offline_case_rank(&stale, None) > prompt_offline_case_rank(&current, None),
        "without a stable-profile constraint the stronger verified teacher should win"
    );
}

#[test]
fn auto_transfer_digest_tracks_teacher_identity_without_mutating_the_normal_cohort() {
    let case = |teacher: PromptAutoTeacherCase| PromptOfflineCase {
        id: "coding-a".to_string(),
        objective: "Evaluate coding-a".to_string(),
        task_class: "coding".to_string(),
        project_id: "project-a".to_string(),
        source_run_id: teacher.source_run_id.clone(),
        split: PromptEvaluationSplit::Train,
        learning_receipt: None,
        auto_teacher: Some(teacher),
    };
    let first = vec![case(test_auto_teacher_case("first verified output"))];
    let second = vec![case(test_auto_teacher_case("second verified output"))];
    let teacher = first[0].auto_teacher.as_ref().unwrap();

    assert_eq!(
        prompt_offline_dataset_digest(&first),
        prompt_offline_dataset_digest(&second),
        "Auto transfer evidence must not invalidate same-effort GEPA evidence"
    );
    assert_ne!(
        prompt_auto_transfer_dataset_digest(&first, &teacher.profile_id, &teacher.profile_sha256),
        prompt_auto_transfer_dataset_digest(&second, &teacher.profile_id, &teacher.profile_sha256),
        "the transfer cohort must pin the archived Auto output"
    );
    assert!(select_prompt_auto_transfer_case(
        &first,
        &[],
        ["pro-stable", "pro-challenger"],
        [None, None],
        &teacher.profile_id,
        &teacher.profile_sha256,
        PromptEvaluationSplit::Train,
    )
    .is_some());
    assert!(select_prompt_auto_transfer_case(
        &first,
        &[],
        ["pro-stable", "pro-challenger"],
        [None, None],
        &teacher.profile_id,
        &sha256_hex(b"stale-auto-profile"),
        PromptEvaluationSplit::Train,
    )
    .is_none());

    let mut malformed_source = first.clone();
    malformed_source[0]
        .auto_teacher
        .as_mut()
        .unwrap()
        .source_context
        .checkpoint_sha256 = "legacy-missing-checkpoint".to_string();
    assert!(prompt_auto_transfer_dataset_digest(
        &malformed_source,
        &teacher.profile_id,
        &teacher.profile_sha256,
    )
    .is_none());
}

#[test]
fn pro_mutation_reserves_reflection_capacity_for_auto_transfer_evidence() {
    let profile_id = "pro-parent";
    let auto_profile_id = "auto-stable";
    let auto_profile_sha256 = sha256_hex(b"auto-stable-genome");
    let packet =
        |candidate_id: &str, run_id: String, case_id: String| AgentEvaluationReflectionPacket {
            suite_id: "runtime-prompt-evolution".to_string(),
            suite_version: 2,
            case_id,
            category: "coding".to_string(),
            run_id,
            seed: 0,
            candidate_id: candidate_id.to_string(),
            candidate_fingerprint: sha256_hex(candidate_id.as_bytes()),
            model_fingerprints: BTreeMap::new(),
            input: "Implement and verify a change".to_string(),
            steps: Vec::new(),
            final_output: "verified".to_string(),
            verifier: AgentEvaluationVerifierOutcome {
                source: AgentEvaluationEvidenceSource::Judge,
                passed: true,
                score: 0.9,
                checks: Vec::new(),
            },
            actionable_feedback: ActionableSideInformation {
                summary: "preserve verification coverage".to_string(),
                ..ActionableSideInformation::default()
            },
        };
    let observation =
        |observed_profile_id: &str,
         run_id: String,
         case_id: String,
         opponent_profile_id: &str,
         relative_reward: f64,
         provenance: PromptEvaluationProvenance| PromptEvolutionObservation {
            profile_id: observed_profile_id.to_string(),
            evaluation_id: run_id.clone(),
            case_id: case_id.clone(),
            opponent_profile_id: Some(opponent_profile_id.to_string()),
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::PairedExecution,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 1_000,
            total_tokens: 800,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(relative_reward),
            step_credits: Vec::new(),
            reflection_packet: Some(packet(observed_profile_id, run_id, case_id)),
            provenance,
        };
    let mut observations = (0..6)
        .map(|index| {
            let run_id = format!("ordinary-{index}");
            observation(
                profile_id,
                run_id.clone(),
                format!("ordinary-case-{index}"),
                "pro-baseline",
                0.4,
                PromptEvaluationProvenance::blind_pairwise_swap(
                    vec!["independent-judge".to_string()],
                    vec!["candidate-worker".to_string()],
                    "a".repeat(64),
                    sha256_hex(profile_id.as_bytes()),
                    sha256_hex(b"pro-baseline"),
                ),
            )
        })
        .collect::<Vec<_>>();
    let transfer_source_context = test_auto_teacher_case("attested").source_context;
    observations.extend((0..3).flat_map(|index| {
        let run_id = format!("transfer-{index}");
        let case_id = format!("transfer-case-{index}");
        let transfer = PromptTransferProvenance::auto_to_pro(
            format!("auto-run-{index}"),
            0,
            auto_profile_id,
            auto_profile_sha256.clone(),
            sha256_hex(format!("auto-output-{index}").as_bytes()),
        )
        .with_source_context(&transfer_source_context)
        .unwrap();
        let matched = PromptMatchedEvaluationIdentityV1 {
            schema: orchestrator::PROMPT_MATCHED_EVALUATION_SCHEMA_V1.to_string(),
            evaluation_id: run_id.clone(),
            cohort_sha256: "c".repeat(64),
            dataset_sha256: "b".repeat(64),
            case_id: case_id.clone(),
            objective_sha256: sha256_hex(case_id.as_bytes()),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::PairedExecution,
        };
        [
            observation(
                profile_id,
                run_id.clone(),
                case_id.clone(),
                auto_profile_id,
                -0.4,
                PromptEvaluationProvenance::blind_pairwise_swap(
                    vec!["independent-judge".to_string()],
                    vec!["candidate-worker".to_string(), "auto-worker".to_string()],
                    "b".repeat(64),
                    sha256_hex(profile_id.as_bytes()),
                    auto_profile_sha256.clone(),
                )
                .with_transfer(transfer.clone())
                .with_matched_evaluation(matched.clone()),
            ),
            observation(
                auto_profile_id,
                run_id,
                case_id,
                profile_id,
                0.4,
                PromptEvaluationProvenance::blind_pairwise_swap(
                    vec!["independent-judge".to_string()],
                    vec!["candidate-worker".to_string(), "auto-worker".to_string()],
                    "b".repeat(64),
                    auto_profile_sha256.clone(),
                    sha256_hex(profile_id.as_bytes()),
                )
                .with_transfer(transfer)
                .with_matched_evaluation(matched),
            ),
        ]
    }));

    assert!(observations
        .iter()
        .skip(6)
        .all(PromptEvolutionObservation::is_strict_source_attested_transfer_evidence));
    assert_eq!(
        prompt_transfer_reflection_pairs(&observations, profile_id, 2).len(),
        4
    );
    let packets = prompt_mutation_reflection_packets(&observations, &[], profile_id, "pro");
    assert_eq!(packets.len(), 6);
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet.run_id.starts_with("ordinary-"))
            .count(),
        2
    );
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet.run_id.starts_with("transfer-"))
            .count(),
        4
    );

    let failure = PromptFailureCurriculumReceiptV1::new(PromptFailureCurriculumInput {
        kind: PromptFailureCurriculumKind::NoProgress,
        project_id: "project-a",
        run_id: "failed-pro-run",
        profile_id,
        policy: "pro",
        task_class: "coding",
        steer_epoch: 0,
        contract_epoch: 0,
        source_event_sequence: 42,
        source_evidence_sha256: &"f".repeat(64),
        failure_code: "no_progress",
        denial_kind: None,
        tool_name: None,
        input_fingerprint: None,
    })
    .unwrap();
    let contrastive = prompt_mutation_reflection_packets(
        &observations,
        std::slice::from_ref(&failure),
        profile_id,
        "pro",
    );
    assert_eq!(contrastive.len(), 6);
    assert_eq!(
        contrastive
            .iter()
            .filter(|packet| {
                packet.suite_id == orchestrator::PROMPT_FAILURE_CURRICULUM_SCHEMA_V1
            })
            .count(),
        1
    );
    assert!(contrastive.iter().any(|packet| packet.verifier.passed));

    let mut failure_only = observations.clone();
    for observation in &mut failure_only {
        observation.succeeded = false;
        if let Some(packet) = observation.reflection_packet.as_mut() {
            packet.verifier.passed = false;
            packet.verifier.score = 0.0;
        }
    }
    let without_anchor = prompt_mutation_reflection_packets(
        &failure_only,
        std::slice::from_ref(&failure),
        profile_id,
        "pro",
    );
    assert!(without_anchor
        .iter()
        .all(|packet| { packet.suite_id != orchestrator::PROMPT_FAILURE_CURRICULUM_SCHEMA_V1 }));

    let mut invalid_anchor = observations.clone();
    for observation in &mut invalid_anchor {
        if observation.profile_id == profile_id {
            observation.format_valid = false;
        }
    }
    assert!(prompt_mutation_reflection_packets(
        &invalid_anchor,
        std::slice::from_ref(&failure),
        profile_id,
        "pro",
    )
    .iter()
    .all(|packet| packet.suite_id != orchestrator::PROMPT_FAILURE_CURRICULUM_SCHEMA_V1));

    let mut unsafe_anchor = observations;
    for observation in &mut unsafe_anchor {
        if observation.profile_id == profile_id {
            observation.safety_violations = 1;
        }
    }
    assert!(
        prompt_mutation_reflection_packets(&unsafe_anchor, &[failure], profile_id, "pro")
            .iter()
            .all(|packet| packet.suite_id != orchestrator::PROMPT_FAILURE_CURRICULUM_SCHEMA_V1)
    );
}

#[test]
fn completed_verified_auto_workflow_becomes_teacher_and_denied_runs_fail_closed() {
    let teacher = test_auto_teacher_case("Verified Auto answer");
    let mut checkpoint =
        WorkflowExecutionCheckpoint::new("auto-resume-key", teacher.plan.clone(), 30);
    checkpoint.prompt_genome_json = serde_json::to_string(&teacher.genome).unwrap();
    checkpoint
        .complete_step(
            "final",
            "auto-worker",
            "Verified Auto answer".to_string(),
            "[]".to_string(),
            35,
        )
        .unwrap();
    checkpoint.record_step_metrics("final", 10, 100).unwrap();
    checkpoint
        .finalize("Verified Auto answer".to_string(), 40)
        .unwrap();
    let evidence = orchestrator::LearningEvidenceV1::independent_quality(
        orchestrator::LearningTermination::Completed,
        orchestrator::LearningAttribution::Workflow,
        orchestrator::LearningUsageCompleteness::Complete,
        0,
        "a".repeat(64),
        orchestrator::IndependentQualitySource::AnytimeSelector,
        9_000,
        true,
    );
    let base = [
        ("agent_run_id".to_string(), "verified-auto-run".to_string()),
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
        ("project_root".to_string(), "/tmp/project-a".to_string()),
        (
            "collaboration_policy".to_string(),
            "auto_router".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let event = |sequence: u64, kind: EventKind, summary: &str, metadata: Metadata| Event {
        id: EventId(format!("teacher-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind,
        summary: summary.to_string(),
        metadata: metadata_with_context(metadata, &base),
    };
    let coordinator_stage = event(
        30,
        EventKind::ModelRequestFinished,
        "Collaboration conductor_plan finished",
        [
            (
                "collaboration_id".to_string(),
                teacher.plan.workflow_id.clone(),
            ),
            ("request_id".to_string(), "conductor-request-1".to_string()),
            ("stage".to_string(), "conductor_plan".to_string()),
            ("role".to_string(), "planner".to_string()),
            ("model".to_string(), "auto-coordinator".to_string()),
            ("status".to_string(), "completed".to_string()),
            ("usage_source".to_string(), "provider".to_string()),
            ("output".to_string(), teacher.plan.to_json().unwrap()),
        ]
        .into_iter()
        .collect(),
    );
    let worker_stage = event(
        50,
        EventKind::ModelRequestFinished,
        "Collaboration worker_1 finished",
        [
            (
                "collaboration_id".to_string(),
                teacher.plan.workflow_id.clone(),
            ),
            ("request_id".to_string(), "worker-request-1".to_string()),
            ("stage".to_string(), "worker_1".to_string()),
            ("role".to_string(), "summarizer".to_string()),
            ("model".to_string(), "auto-worker".to_string()),
            ("status".to_string(), "completed".to_string()),
            ("usage_source".to_string(), "provider".to_string()),
            ("workflow_step_id".to_string(), "final".to_string()),
            ("output".to_string(), "Verified Auto answer".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    let provider_started = event(
        59,
        EventKind::ModelRequestStarted,
        "Collaboration quality_gate started",
        [
            (
                "collaboration_id".to_string(),
                teacher.plan.workflow_id.clone(),
            ),
            ("request_id".to_string(), "quality-request-1".to_string()),
            ("stage".to_string(), "quality_gate".to_string()),
            ("role".to_string(), "reviewer".to_string()),
            ("model".to_string(), "auto-reviewer".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    let provider_stage = event(
        60,
        EventKind::ModelRequestFinished,
        "Collaboration quality_gate completed",
        [
            (
                "collaboration_id".to_string(),
                teacher.plan.workflow_id.clone(),
            ),
            ("request_id".to_string(), "quality-request-1".to_string()),
            ("stage".to_string(), "quality_gate".to_string()),
            ("role".to_string(), "reviewer".to_string()),
            ("model".to_string(), "auto-reviewer".to_string()),
            ("status".to_string(), "completed".to_string()),
            ("usage_source".to_string(), "provider".to_string()),
            ("total_tokens".to_string(), "10".to_string()),
            (
                "output".to_string(),
                r#"{"pass":true,"score":0.9,"issues":[],"safety_violations":0}"#.to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let evaluator = orchestrator::AutoTeacherProviderIdentityV1::new(
        "openai",
        "",
        "https://api.openai.com/v1",
        "auto-reviewer",
    )
    .unwrap();
    let evaluator_receipt = orchestrator::AutoTeacherEvaluatorReceiptV1 {
        schema: orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_SCHEMA_V1.to_string(),
        protocol: orchestrator::AUTO_TEACHER_PROVIDER_REVIEW_PROTOCOL_V1.to_string(),
        evaluator,
        stage: "quality_gate".to_string(),
        request_id_sha256: sha256_hex(b"quality-request-1"),
        stage_event_sha256: crate::prompt_learning_runtime::prompt_event_sha256(&provider_stage)
            .unwrap(),
        evaluated_artifact_sha256: orchestrator::auto_teacher_evaluated_artifact_sha256(
            "Verified Auto answer",
        )
        .unwrap(),
        system_prompt_sha256: sha256_hex(b"test-system-prompt"),
        tool_contract_sha256: sha256_hex(b"test-tool-contract"),
        source_revision_sha256: sha256_hex(b"test-source-revision"),
        quality_score_bps: 9_000,
        passed: true,
        safety_violations: 0,
    };
    let mut events = vec![
        event(
            10,
            EventKind::TaskStatusChanged,
            "Agent task started",
            [
                (
                    "prompt".to_string(),
                    "Evaluate a completed Auto workflow".to_string(),
                ),
                ("task_class".to_string(), "coding".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            20,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile selected",
            [
                (
                    "collaboration_id".to_string(),
                    teacher.plan.workflow_id.clone(),
                ),
                ("prompt_effort".to_string(), "auto".to_string()),
                ("prompt_profile".to_string(), teacher.profile_id.clone()),
                (
                    "prompt_genome".to_string(),
                    serde_json::to_string(&teacher.genome).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            40,
            EventKind::TaskStatusChanged,
            "Collaboration workflow planned",
            [
                (
                    "collaboration_id".to_string(),
                    teacher.plan.workflow_id.clone(),
                ),
                ("workflow_ir".to_string(), teacher.plan.to_json().unwrap()),
            ]
            .into_iter()
            .collect(),
        ),
        coordinator_stage,
        worker_stage,
        provider_started,
        provider_stage,
        event(
            70,
            EventKind::TaskStatusChanged,
            "Collaboration quality gate evaluated",
            [
                (
                    "collaboration_id".to_string(),
                    teacher.plan.workflow_id.clone(),
                ),
                ("quality_pass".to_string(), "true".to_string()),
                ("quality_score".to_string(), "0.900".to_string()),
                ("safety_violations".to_string(), "0".to_string()),
                (
                    orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY.to_string(),
                    serde_json::to_string(&evaluator_receipt).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            80,
            EventKind::TaskStatusChanged,
            "Collaboration workflow checkpoint finalized",
            [
                (
                    "collaboration_id".to_string(),
                    teacher.plan.workflow_id.clone(),
                ),
                (
                    "workflow_checkpoint".to_string(),
                    checkpoint.to_json().unwrap(),
                ),
                (
                    "anytime_prompt_learning_eligible".to_string(),
                    "true".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            90,
            EventKind::MessageAdded,
            "Assistant answer",
            [
                ("role".to_string(), "assistant".to_string()),
                (
                    "display_content".to_string(),
                    "A downstream user-facing answer that was not reviewed".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        event(
            100,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            [
                (
                    orchestrator::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                    evidence.to_metadata_value().unwrap(),
                ),
                (
                    "run_lineage_physical_model_attempts".to_string(),
                    "1".to_string(),
                ),
                (
                    "run_lineage_provider_usage_attempts".to_string(),
                    "1".to_string(),
                ),
                (
                    "run_lineage_partial_usage_attempts".to_string(),
                    "0".to_string(),
                ),
                (
                    "run_lineage_estimated_usage_attempts".to_string(),
                    "0".to_string(),
                ),
                (
                    "run_lineage_unknown_usage_attempts".to_string(),
                    "0".to_string(),
                ),
                ("run_lineage_total_tokens".to_string(), "100".to_string()),
                ("run_lineage_prompt_tokens".to_string(), "50".to_string()),
                (
                    "run_lineage_completion_tokens".to_string(),
                    "50".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    ];

    let dataset = prompt_offline_dataset(&events, "project-a", None);
    let captured = dataset[0]
        .auto_teacher
        .as_ref()
        .expect("verified Auto workflow should become a teacher");
    assert_eq!(captured.final_output, "Verified Auto answer");
    assert_eq!(
        captured.participant_models,
        vec!["auto-coordinator".to_string(), "auto-worker".to_string()]
    );
    assert_eq!(captured.quality_score_bps, 9_000);
    assert_eq!(captured.profile_sha256, teacher.profile_sha256);
    assert_eq!(
        reconstruct_prompt_auto_teacher_from_canonical_events(
            &events,
            "project-a",
            "verified-auto-run",
        )
        .expect("canonical source events should replay the accepted teacher")
        .source_context,
        captured.source_context,
    );

    let pro_profile = ConductorPromptGenome::seed_for_effort("pro");
    let pro_profile_sha256 = prompt_genome_sha256(&pro_profile).unwrap();
    let transfer_case_id = "canonical-auto-transfer";
    let transfer_evaluation_id = scoped_prompt_evaluation_id("project-a", transfer_case_id);
    let transfer_dataset = PromptDatasetIdentityV1::new(
        "project-a",
        0,
        vec![PromptDatasetCaseIdentityV1 {
            case_id: transfer_case_id.to_string(),
            objective_sha256: "d".repeat(64),
            task_family_sha256: "e".repeat(64),
            split: PromptEvaluationSplit::Train,
        }],
    )
    .unwrap();
    let transfer_cohort = PromptLearningCohortV1::new(
        transfer_dataset,
        PromptExecutionContextV1 {
            schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            harness_sha256: "3".repeat(64),
            system_prompt_sha256: "4".repeat(64),
            policy: AgentPolicy::Pro,
            policy_sha256: "5".repeat(64),
            budget_sha256: "6".repeat(64),
            tool_contract_sha256: "7".repeat(64),
            source_revision_sha256: "8".repeat(64),
            workspace_revision_sha256: "9".repeat(64),
        },
    )
    .unwrap();
    let matched_transfer = PromptMatchedEvaluationIdentityV1::new(
        transfer_evaluation_id.clone(),
        &transfer_cohort,
        transfer_case_id,
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedExecution,
    )
    .unwrap();
    let transfer = PromptTransferProvenance::auto_to_pro(
        captured.source_run_id.clone(),
        captured.steer_epoch,
        captured.profile_id.clone(),
        captured.profile_sha256.clone(),
        captured.output_sha256.clone(),
    )
    .with_source_context(&captured.source_context)
    .unwrap();
    let transfer_observation = |profile_id: &str,
                                opponent_profile_id: &str,
                                profile_sha256: String,
                                opponent_sha256: String,
                                relative_reward: f64| {
        PromptEvolutionObservation {
            profile_id: profile_id.to_string(),
            evaluation_id: transfer_evaluation_id.clone(),
            case_id: transfer_case_id.to_string(),
            opponent_profile_id: Some(opponent_profile_id.to_string()),
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::PairedExecution,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 10,
            total_tokens: 20,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(relative_reward),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: PromptEvaluationProvenance::blind_pairwise_swap(
                vec!["independent-transfer-reviewer".to_string()],
                vec!["pro-worker".to_string(), "auto-worker".to_string()],
                transfer_cohort.dataset.dataset_sha256.clone(),
                profile_sha256,
                opponent_sha256,
            )
            .with_transfer(transfer.clone())
            .with_matched_evaluation(matched_transfer.clone()),
        }
    };
    let transfer_observations = vec![
        transfer_observation(
            &pro_profile.id,
            &captured.profile_id,
            pro_profile_sha256.clone(),
            captured.profile_sha256.clone(),
            0.2,
        ),
        transfer_observation(
            &captured.profile_id,
            &pro_profile.id,
            captured.profile_sha256.clone(),
            pro_profile_sha256,
            -0.2,
        ),
    ];
    let mut transfer_event = event(
        9,
        EventKind::TaskStatusChanged,
        "Conductor Auto transfer evaluation",
        [
            ("prompt_effort".to_string(), "pro".to_string()),
            ("evaluation_id".to_string(), transfer_evaluation_id.clone()),
            (
                "auto_teacher_profile".to_string(),
                captured.profile_id.clone(),
            ),
            (
                "auto_teacher_run_id".to_string(),
                captured.source_run_id.clone(),
            ),
            (
                "prompt_observations".to_string(),
                serde_json::to_string(&transfer_observations).unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    );
    transfer_event.metadata.remove("agent_run_id");
    transfer_event.metadata.remove("collaboration_id");
    let mut full_events = events.clone();
    full_events.push(transfer_event.clone());
    let full_projection = build_prompt_evolution_read_model(
        &full_events,
        transfer_event.sequence,
        full_events.len() as u64,
    );

    let mut store = SqliteStore::in_memory().unwrap();
    for source_event in events.iter().cloned() {
        store.append(source_event).unwrap();
    }
    let before_transfer = load_prompt_evolution_read_model(&mut store).unwrap();
    assert!(before_transfer
        .observations
        .iter()
        .all(|(_, observation)| observation.case_id != transfer_case_id));
    store.append(transfer_event).unwrap();
    let incremental_projection = load_prompt_evolution_read_model(&mut store).unwrap();
    let transfer_records = |model: &PromptEvolutionReadModel| {
        model
            .observations
            .iter()
            .filter(|(_, observation)| observation.case_id == transfer_case_id)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(transfer_records(&full_projection).len(), 2);
    assert_eq!(
        transfer_records(&incremental_projection),
        transfer_records(&full_projection),
        "incremental projection must query the canonical source run and match full replay"
    );

    assert_eq!(
        prompt_learning_dataset(&events, "project-a", None).len(),
        1,
        "only a typed, verified prompt-learning receipt may enter replay"
    );

    let mut unattested = events.clone();
    unattested
        .iter_mut()
        .find(|event| event.summary == "Collaboration quality gate evaluated")
        .unwrap()
        .metadata
        .remove(orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY);
    assert!(prompt_offline_dataset(&unattested, "project-a", None)[0]
        .auto_teacher
        .is_none());

    let mut output_mismatch = events.clone();
    let quality_event = output_mismatch
        .iter_mut()
        .find(|event| event.summary == "Collaboration quality gate evaluated")
        .unwrap();
    let mut mismatched_receipt: orchestrator::AutoTeacherEvaluatorReceiptV1 = serde_json::from_str(
        quality_event
            .metadata
            .get(orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY)
            .unwrap(),
    )
    .unwrap();
    mismatched_receipt.evaluated_artifact_sha256 =
        orchestrator::auto_teacher_evaluated_artifact_sha256("unreviewed replacement").unwrap();
    quality_event.metadata.insert(
        orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY.to_string(),
        serde_json::to_string(&mismatched_receipt).unwrap(),
    );
    assert!(
        prompt_offline_dataset(&output_mismatch, "project-a", None)[0]
            .auto_teacher
            .is_none()
    );

    let mut forged_request_receipt = events.clone();
    let quality_event = forged_request_receipt
        .iter_mut()
        .find(|event| event.summary == "Collaboration quality gate evaluated")
        .unwrap();
    let mut forged_receipt: orchestrator::AutoTeacherEvaluatorReceiptV1 = serde_json::from_str(
        quality_event
            .metadata
            .get(orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY)
            .unwrap(),
    )
    .unwrap();
    forged_receipt.request_id_sha256 = sha256_hex(b"worker-request-1");
    quality_event.metadata.insert(
        orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY.to_string(),
        serde_json::to_string(&forged_receipt).unwrap(),
    );
    assert!(
        prompt_offline_dataset(&forged_request_receipt, "project-a", None)[0]
            .auto_teacher
            .is_none(),
        "a forged receipt cannot rebind the evaluator to a participant request"
    );

    let mut mismatched_checkpoint = events.clone();
    let checkpoint_event = mismatched_checkpoint
        .iter_mut()
        .find(|event| event.summary == "Collaboration workflow checkpoint finalized")
        .unwrap();
    let mut mismatched = WorkflowExecutionCheckpoint::from_json(
        checkpoint_event
            .metadata
            .get("workflow_checkpoint")
            .unwrap(),
        &["auto-worker".to_string()],
    )
    .unwrap();
    mismatched.prompt_genome_json =
        serde_json::to_string(&ConductorPromptGenome::seed_for_effort("pro")).unwrap();
    checkpoint_event.metadata.insert(
        "workflow_checkpoint".to_string(),
        mismatched.to_json().unwrap(),
    );
    assert!(
        prompt_offline_dataset(&mismatched_checkpoint, "project-a", None)[0]
            .auto_teacher
            .is_none()
    );

    let mut runtime_model_overlap = events.clone();
    runtime_model_overlap
        .iter_mut()
        .find(|event| event.metadata.get("stage").map(String::as_str) == Some("worker_1"))
        .unwrap()
        .metadata
        .insert("model".to_string(), "auto-reviewer".to_string());
    runtime_model_overlap
        .iter_mut()
        .find(|event| event.metadata.get("stage").map(String::as_str) == Some("conductor_plan"))
        .unwrap()
        .metadata
        .insert("model".to_string(), "auto-reviewer".to_string());
    let plan_event = runtime_model_overlap
        .iter_mut()
        .find(|event| event.summary == "Collaboration workflow planned")
        .unwrap();
    let mut runtime_plan = WorkflowPlanIr::from_json(
        plan_event.metadata.get("workflow_ir").unwrap(),
        &["auto-coordinator".to_string(), "auto-worker".to_string()],
    )
    .unwrap();
    runtime_plan.coordinator_model = "auto-reviewer".to_string();
    plan_event
        .metadata
        .insert("workflow_ir".to_string(), runtime_plan.to_json().unwrap());
    let checkpoint_event = runtime_model_overlap
        .iter_mut()
        .find(|event| event.summary == "Collaboration workflow checkpoint finalized")
        .unwrap();
    let mut runtime_checkpoint = WorkflowExecutionCheckpoint::from_json(
        checkpoint_event
            .metadata
            .get("workflow_checkpoint")
            .unwrap(),
        &["auto-worker".to_string()],
    )
    .unwrap();
    runtime_checkpoint.plan.coordinator_model = "auto-reviewer".to_string();
    runtime_checkpoint.steps.get_mut("final").unwrap().model = "auto-reviewer".to_string();
    checkpoint_event.metadata.insert(
        "workflow_checkpoint".to_string(),
        runtime_checkpoint.to_json().unwrap(),
    );
    let same_model_teacher = prompt_offline_dataset(&runtime_model_overlap, "project-a", None)[0]
        .auto_teacher
        .clone()
        .expect("a separately receipted evaluator request may reuse the participant model");
    assert!(same_model_teacher
        .participant_models
        .contains(&"auto-reviewer".to_string()));
    assert_eq!(
        same_model_teacher.source_context.evaluator_identity_sha256,
        evaluator_receipt.evaluator.identity_sha256
    );

    let mut reused_evaluator_request = runtime_model_overlap.clone();
    reused_evaluator_request
        .iter_mut()
        .find(|event| event.metadata.get("stage").map(String::as_str) == Some("worker_1"))
        .unwrap()
        .metadata
        .insert("request_id".to_string(), "quality-request-1".to_string());
    assert!(
        prompt_offline_dataset(&reused_evaluator_request, "project-a", None)[0]
            .auto_teacher
            .is_none(),
        "the evaluator request itself cannot also attest a participant output"
    );

    let mut repair_author_overlap = events.clone();
    repair_author_overlap.push(event(
        55,
        EventKind::ModelRequestFinished,
        "Collaboration quality_repair_1 finished",
        [
            (
                "collaboration_id".to_string(),
                teacher.plan.workflow_id.clone(),
            ),
            ("request_id".to_string(), "repair-request-1".to_string()),
            ("stage".to_string(), "quality_repair_1".to_string()),
            ("role".to_string(), "summarizer".to_string()),
            ("model".to_string(), "auto-reviewer".to_string()),
            ("status".to_string(), "completed".to_string()),
            ("usage_source".to_string(), "provider".to_string()),
            ("output".to_string(), "repaired intermediate".to_string()),
        ]
        .into_iter()
        .collect(),
    ));
    assert!(
        prompt_offline_dataset(&repair_author_overlap, "project-a", None)[0]
            .auto_teacher
            .is_some(),
        "model overlap alone must not erase a separately receipted evaluator request"
    );

    events.push(event(
        95,
        EventKind::PermissionResolved,
        "Permission denied",
        [("decision".to_string(), "deny".to_string())]
            .into_iter()
            .collect(),
    ));
    assert!(prompt_offline_dataset(&events, "project-a", None)[0]
        .auto_teacher
        .is_none());
    assert!(prompt_learning_dataset(&events, "project-a", None).is_empty());
}

#[test]
fn offline_prompt_scheduler_prioritizes_underrepresented_task_class() {
    let dataset = [
        ("coding-a", "coding"),
        ("coding-b", "coding"),
        ("research-a", "research"),
    ]
    .into_iter()
    .map(|(id, task_class)| PromptOfflineCase {
        id: id.to_string(),
        objective: format!("Evaluate {id}"),
        task_class: task_class.to_string(),
        project_id: "project-a".to_string(),
        source_run_id: format!("run-{id}"),
        split: PromptEvaluationSplit::Train,
        learning_receipt: None,
        auto_teacher: None,
    })
    .collect::<Vec<_>>();
    let mut observations = ["coding-a", "coding-b"]
        .into_iter()
        .flat_map(|case_id| {
            [("stable", "candidate"), ("candidate", "stable")].map(
                move |(profile_id, opponent_id)| PromptEvolutionObservation {
                    profile_id: profile_id.to_string(),
                    evaluation_id: format!("eval-{profile_id}-{case_id}"),
                    case_id: case_id.to_string(),
                    opponent_profile_id: Some(opponent_id.to_string()),
                    task_class: "coding".to_string(),
                    split: PromptEvaluationSplit::Train,
                    mode: PromptEvaluationMode::PairedExecution,
                    format_valid: true,
                    succeeded: true,
                    quality_score: 0.8,
                    latency_ms: 100,
                    total_tokens: 100,
                    estimated_cost_microusd: 0,
                    safety_violations: 0,
                    relative_reward: Some(0.1),
                    step_credits: Vec::new(),
                    reflection_packet: None,
                    provenance: test_prompt_evaluation_provenance(profile_id, opponent_id),
                },
            )
        })
        .collect::<Vec<_>>();
    let dataset_sha256 = prompt_offline_dataset_digest(&dataset);
    for observation in &mut observations {
        observation.provenance.dataset_sha256 = dataset_sha256.clone();
    }

    let selected = select_prompt_offline_case(
        &dataset,
        &observations,
        "stable",
        "candidate",
        PromptEvaluationSplit::Train,
    )
    .expect("an offline case should be scheduled");

    assert_eq!(selected.id, "research-a");
}

#[test]
fn replay_holdout_accepts_a_completed_bounded_collaboration() {
    let profile = ConductorPromptGenome::seed_for_effort("pro");
    let profile_event = Event {
        id: EventId("bounded-profile".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 100,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor prompt profile selected".to_string(),
        metadata: [
            ("collaboration_id".to_string(), "bounded-1".to_string()),
            ("collaboration_profile".to_string(), "bounded".to_string()),
            ("agent_run_id".to_string(), "run-1".to_string()),
            ("prompt_effort".to_string(), "pro".to_string()),
            ("prompt_profile".to_string(), profile.id.clone()),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&profile).unwrap(),
            ),
            (
                "prompt_objective".to_string(),
                "Completed bounded task".to_string(),
            ),
            ("task_class".to_string(), "coding".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let terminal = Event {
        id: EventId("bounded-terminal".to_string()),
        task_id: phase16_task_id(),
        sequence: 2,
        timestamp_ms: 200,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task completed".to_string(),
        metadata: [("agent_run_id".to_string(), "run-1".to_string())]
            .into_iter()
            .collect(),
    };
    let events = vec![profile_event, terminal];

    let replay = prompt_replay_case(&events, "Current task", 0).unwrap();
    let model = build_prompt_evolution_read_model(&events, 2, 2);

    assert_eq!(replay.objective, "Completed bounded task");
    assert_eq!(replay.task_class, "coding");
    assert_eq!(model.genomes.len(), 1);
    assert!(model.observations.is_empty());
}

#[test]
fn bidirectional_pairwise_judging_normalizes_position_and_merges_feedback() {
    let feedback = |summary: &str, change: &str| ActionableSideInformation {
        summary: summary.to_string(),
        suggested_changes: vec![change.to_string()],
        ..ActionableSideInformation::default()
    };
    let forward = PromptPairwiseEvaluationPayload {
        score_a: 0.8,
        score_b: 0.4,
        safety_violations_a: 0,
        safety_violations_b: 1,
        step_scores_a: [("inspect".to_string(), 0.8)].into_iter().collect(),
        step_scores_b: [("inspect".to_string(), 0.4)].into_iter().collect(),
        feedback_a: feedback("forward A", "preserve evidence"),
        feedback_b: feedback("forward B", "fix verification"),
    };
    let reverse = PromptPairwiseEvaluationPayload {
        score_a: 0.2,
        score_b: 0.6,
        safety_violations_a: 0,
        safety_violations_b: 2,
        step_scores_a: [("inspect".to_string(), 0.2)].into_iter().collect(),
        step_scores_b: [("inspect".to_string(), 0.6)].into_iter().collect(),
        feedback_a: feedback("reverse B", "fix verification"),
        feedback_b: feedback("reverse A", "reduce unsupported claims"),
    };

    let aggregate =
        aggregate_prompt_pairwise_payloads(forward, reverse_prompt_pairwise_payload(reverse));

    assert!((aggregate.score_a - 0.7).abs() < f64::EPSILON * 8.0);
    assert!((aggregate.score_b - 0.3).abs() < f64::EPSILON * 8.0);
    assert_eq!(aggregate.safety_violations_a, 2);
    assert_eq!(aggregate.safety_violations_b, 1);
    assert_eq!(aggregate.step_scores_a.get("inspect"), Some(&0.7));
    assert!(aggregate.feedback_a.summary.contains("forward A"));
    assert!(aggregate.feedback_a.summary.contains("reverse A"));
    assert_eq!(aggregate.feedback_b.suggested_changes.len(), 1);
}

#[test]
fn bidirectional_pairwise_judging_rejects_material_disagreement() {
    let payload = |score_a, score_b| PromptPairwiseEvaluationPayload {
        score_a,
        score_b,
        safety_violations_a: 0,
        safety_violations_b: 0,
        step_scores_a: BTreeMap::new(),
        step_scores_b: BTreeMap::new(),
        feedback_a: ActionableSideInformation::default(),
        feedback_b: ActionableSideInformation::default(),
    };
    let forward = payload(0.8, 0.2);
    let reverse_aligned = payload(0.3, 0.7);

    let error = validate_prompt_pairwise_agreement(&forward, &reverse_aligned).unwrap_err();

    assert!(error.contains("reviewer disagreement"));
}

#[test]
fn pairwise_observation_keeps_relative_and_per_step_credit() {
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let opponent_profile = ConductorPromptGenome {
        id: "opponent".to_string(),
        ..profile.clone()
    };
    let candidate_plan = PromptPlanCandidate {
        genome: profile,
        plan: Some(WorkflowPlanIr::from_adaptive_with_profile(
            "candidate",
            "Verify a change",
            "auto",
            "best_of_n",
            "planner",
            "seed-auto-v1",
            &AdaptiveWorkflow {
                steps: vec![AdaptiveWorkflowStep {
                    id: "verify".to_string(),
                    role: "reviewer".to_string(),
                    model: "planner".to_string(),
                    subtask: "verify".to_string(),
                    access: Vec::new(),
                }],
            },
            WorkflowBudget {
                max_steps: 4,
                max_models: 2,
                max_model_turns_per_step: 3,
                max_tool_calls_per_step: 4,
                max_output_tokens_per_step: 2_048,
            },
        )),
        raw_output: String::new(),
        latency_ms: 120,
        total_tokens: 80,
    };
    let opponent_plan = PromptPlanCandidate {
        genome: opponent_profile,
        plan: candidate_plan.plan.clone(),
        raw_output: String::new(),
        latency_ms: 140,
        total_tokens: 90,
    };
    let candidate = PromptExecutionCandidate {
        plan: candidate_plan,
        execution: PromptWorkflowExecution {
            succeeded: true,
            quality_gate_met: true,
            final_output: "verified".to_string(),
            steps: vec![PromptExecutionStep {
                id: "verify".to_string(),
                role: "reviewer".to_string(),
                model: "reviewer-model".to_string(),
                prompt: "verify the result".to_string(),
                attempts: 1,
                status: WorkflowStepStatus::Completed,
                output: "verified".to_string(),
                tool_calls: Vec::new(),
                errors: Vec::new(),
                latency_ms: 100,
                total_tokens: 60,
                evidence_count: 1,
            }],
            latency_ms: 100,
            total_tokens: 60,
        },
    };
    let opponent = PromptExecutionCandidate {
        plan: opponent_plan,
        execution: PromptWorkflowExecution {
            succeeded: true,
            quality_gate_met: true,
            final_output: "reviewed".to_string(),
            steps: vec![PromptExecutionStep {
                id: "verify".to_string(),
                role: "reviewer".to_string(),
                model: "reviewer-model".to_string(),
                prompt: "review the result".to_string(),
                attempts: 1,
                status: WorkflowStepStatus::Completed,
                output: "reviewed".to_string(),
                tool_calls: Vec::new(),
                errors: Vec::new(),
                latency_ms: 120,
                total_tokens: 70,
                evidence_count: 1,
            }],
            latency_ms: 120,
            total_tokens: 70,
        },
    };
    let observation = prompt_evaluation_feedback::prompt_pairwise_observation(
        &candidate,
        &opponent,
        "Fix the project and run tests",
        "pair-1",
        "coding",
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedShadow,
        0.85,
        0.55,
        0,
        &[("verify".to_string(), 0.78)].into_iter().collect(),
        ActionableSideInformation {
            summary: "candidate verified more completely".to_string(),
            ..ActionableSideInformation::default()
        },
        &[],
        test_prompt_evaluation_provenance(&candidate.plan.genome.id, &opponent.plan.genome.id),
    );

    assert!((observation.relative_reward.unwrap_or_default() - 0.3).abs() < f64::EPSILON * 4.0);
    assert_eq!(observation.step_credits.len(), 1);
    assert_eq!(observation.step_credits[0].step_id, "verify");
    assert_eq!(observation.step_credits[0].credit, 0.78);
    assert!(observation.reflection_packet.is_none());

    let secret = "evaluation-secret-token".to_string();
    let bearer = "Bearer runtime-reflection-token";
    let mut traced_candidate = candidate.clone();
    traced_candidate.execution.steps[0].prompt =
        format!("inspect with {secret}\nAuthorization: {bearer}");
    traced_candidate.execution.steps[0].tool_calls = vec![AgentEvaluationToolTrace {
        tool: "file.read".to_string(),
        request: format!("{{\"token\":\"{secret}\"}}"),
        response: format!("verified with {secret}\n{bearer}"),
        error: None,
    }];
    let traced = prompt_evaluation_feedback::prompt_pairwise_observation(
        &traced_candidate,
        &opponent,
        &format!("Fix the project using {secret}"),
        "pair-2",
        "coding",
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedExecution,
        0.85,
        0.55,
        0,
        &[("verify".to_string(), 0.78)].into_iter().collect(),
        ActionableSideInformation {
            summary: format!("verified without exposing {secret}"),
            ..ActionableSideInformation::default()
        },
        std::slice::from_ref(&secret),
        test_prompt_evaluation_provenance(
            &traced_candidate.plan.genome.id,
            &opponent.plan.genome.id,
        ),
    );
    let packet = traced
        .reflection_packet
        .expect("executed feedback should produce a reflection packet");
    let encoded = serde_json::to_string(&packet).expect("packet should serialize");
    assert!(!encoded.contains(&secret));
    assert!(!encoded.contains("runtime-reflection-token"));
    assert!(encoded.contains("[REDACTED]"));
    assert_eq!(packet.steps[0].tool_calls.len(), 1);

    let mut unsafe_packet = packet.clone();
    unsafe_packet.input = secret.clone();
    assert!(
        !prompt_evaluation_feedback::prompt_reflection_packet_is_safe(
            &unsafe_packet,
            std::slice::from_ref(&secret),
        ),
        "configured sensitive values must fail the reflection boundary"
    );
    let escaped_secret = "evaluation-\"secret".to_string();
    unsafe_packet.input = escaped_secret.clone();
    assert!(
        !prompt_evaluation_feedback::prompt_reflection_packet_is_safe(
            &unsafe_packet,
            std::slice::from_ref(&escaped_secret),
        ),
        "serialized configured sensitive values must fail the reflection boundary"
    );
    unsafe_packet.input =
        "unredacted eyJhbGciOiJIUzI1NiJ9.abcdefghijklmno.pqrstuvwxyz123456".to_string();
    assert!(
        crate::prompt_learning_runtime::prompt_text_contains_residual_secret(&unsafe_packet.input)
    );
    assert!(
        !prompt_evaluation_feedback::prompt_reflection_packet_is_safe(&unsafe_packet, &[]),
        "residual secret patterns must fail the reflection boundary"
    );

    let mut residual_candidate = candidate.clone();
    residual_candidate.execution.final_output = unsafe_packet.input;
    let residual_observation = prompt_evaluation_feedback::prompt_pairwise_observation(
        &residual_candidate,
        &opponent,
        "Fix the project and run tests",
        "pair-3",
        "coding",
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedExecution,
        0.85,
        0.55,
        0,
        &[("verify".to_string(), 0.78)].into_iter().collect(),
        ActionableSideInformation {
            summary: "candidate verified more completely".to_string(),
            ..ActionableSideInformation::default()
        },
        &[],
        test_prompt_evaluation_provenance(
            &residual_candidate.plan.genome.id,
            &opponent.plan.genome.id,
        ),
    );
    assert!(
        residual_observation.reflection_packet.is_none(),
        "unsafe learning material must be omitted without affecting the observation"
    );
}

#[test]
fn agent_runtime_turn_budget_tracks_effort_and_extends_on_resume() {
    let control = AgentRunControl::new("pro");
    let config = control.runtime_config();
    assert_eq!(config.max_turns, 384);

    let mut runtime = start_agent_loop(phase16_task_id(), "continue", config);
    runtime.turn = 23;
    control.extend_runtime_budget(&mut runtime);

    assert_eq!(runtime.max_turns, 407);
}

#[test]
fn agent_run_budget_extends_only_after_material_progress_and_stops_cycles() {
    let control = AgentRunControl::new("fast");
    for call in 1..=6 {
        assert_eq!(control.begin_model_call("executor"), Ok(call));
    }
    assert!(control.record_observation("model_result", "executor", "new model wording"));
    assert_eq!(
        control.begin_model_call("executor"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );

    let control = AgentRunControl::new("fast");
    for call in 1..=6 {
        assert_eq!(control.begin_model_call("executor"), Ok(call));
    }
    let tools = vec![ToolSpec::builtin(
        "file.read",
        "test",
        "Read required evidence",
        ToolRisk::ReadOnly,
        r#"{"type":"object"}"#,
    )];
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "read required evidence",
        AgentRuntimeConfig::default(),
    );
    runtime.task_contract.require_tool_success("file.read");
    let delta = AgentKernel::new(&mut runtime, &tools)
        .apply_tool_observation(
            &AgentToolRequest {
                call_id: agent_core::ToolCallId("goal-delta-read".to_string()),
                tool_name: "file.read".to_string(),
                input: r#"{"path":"goal.md"}"#.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
            "required evidence",
        )
        .expect("the required evidence should emit one Goal Delta");
    assert!(control.record_goal_delta_at(0, &delta));
    assert_eq!(control.begin_model_call("executor"), Ok(7));
    assert!(!control.record_goal_delta_at(0, &delta));

    let cycle_control = AgentRunControl::new("fast");
    for input in ["a", "b", "a", "b", "a", "b", "a"] {
        assert!(cycle_control
            .begin_tool_call("executor", "file.read", input)
            .is_ok());
    }
    assert_eq!(
        cycle_control.begin_tool_call("executor", "file.read", "b"),
        Err(RunStopReason::RepeatedAction)
    );
}

#[test]
fn agent_runtime_never_completes_with_an_empty_model_answer() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "complete the task",
        AgentRuntimeConfig { max_turns: 6 },
    );
    let empty_response = || model_provider::ModelResponse {
        message: Message {
            role: MessageRole::Assistant,
            content: String::new(),
            metadata: Metadata::new(),
        },
        raw_tool_calls_json: None,
        tool_calls: Vec::new(),
        metadata: Metadata::new(),
    };

    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Retry { .. }
    ));
    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Retry { .. }
    ));
    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Failed { .. }
    ));
}

#[test]
fn image_generation_run_cannot_complete_without_the_configured_tool() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "生成一张图片",
        AgentRuntimeConfig { max_turns: 6 },
    );
    let run_context = [("image_generation_required".to_string(), "true".to_string())]
        .into_iter()
        .collect();
    apply_run_task_contract(&mut runtime, &run_context, &[], None).expect("task contract applies");
    assert!(!runtime
        .task_contract
        .required_tool_satisfied("image.generate"));

    record_tool_outcome_with_risk(
        &mut runtime,
        "image.generate",
        r#"{"prompt":"cat"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::UsesNetwork),
    );

    assert!(runtime
        .task_contract
        .required_tool_satisfied("image.generate"));
}

#[test]
fn conductor_evaluation_repairs_invalid_structure_before_scoring() {
    let genome = ConductorPromptGenome::seed_for_effort("fast");
    let routing = RoutingContext::from_prompt("Answer a focused question", Vec::new());
    let harness = ConductorHarness::new(ConductorRequest {
        workflow_id: "repair-evaluation".to_string(),
        objective: "Answer a focused question".to_string(),
        recent_context: String::new(),
        effort: "fast".to_string(),
        policy: "direct".to_string(),
        conductor_model: "planner".to_string(),
        primary_model: "worker-a".to_string(),
        worker_models: vec!["worker-a".to_string()],
        role_hints: ConductorRoleHints {
            planner: "worker-a".to_string(),
            executor: "worker-a".to_string(),
            reviewer: "worker-a".to_string(),
            synthesizer: "worker-a".to_string(),
        },
        budget: WorkflowBudget {
            max_steps: 2,
            max_models: 1,
            max_model_turns_per_step: 1,
            max_tool_calls_per_step: 0,
            max_output_tokens_per_step: 1_024,
        },
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            "fast",
            OrchestrationPolicy::Single,
        ),
        prior_hint: None,
        prompt_evolution_enabled: true,
        prompt_genome: genome.clone(),
    });
    let mut calls = 0usize;
    let mut prompts = Vec::new();

    let candidate = evaluate_conductor_prompt_profile_with_runner(&harness, &genome, |prompt| {
        calls += 1;
        prompts.push(prompt);
        CollaborationCompletion {
            content: Some(if calls == 1 {
                "not a workflow".to_string()
            } else {
                r#"{"steps":[{"id":"final","role":"synthesizer","model":"worker-a","subtask":"answer directly","access":[]}]}"#.to_string()
            }),
            partial_content: None,
            error: None,
            failure: None,
            latency_ms: if calls == 1 { 7 } else { 11 },
            usage: [(
                "total_tokens".to_string(),
                if calls == 1 { "13" } else { "17" }.to_string(),
            )]
            .into_iter()
            .collect(),
            evidence: Vec::new(),
        }
    });

    assert!(candidate.plan.is_some());
    assert_eq!(calls, 2);
    assert_eq!(candidate.latency_ms, 18);
    assert_eq!(candidate.total_tokens, 30);
    assert!(prompts[1].contains("deterministic Cindx Harness"));
    assert!(prompts[1].contains("not a workflow"));
}

#[test]
fn conductor_evaluation_uses_a_collaborative_fallback_after_failed_repair() {
    let genome =
        ConductorPromptGenome::seed_for_effort("auto").with_effort_delivery_contract("auto");
    let routing = RoutingContext::from_prompt(
        "Compare two implementation strategies with evidence",
        Vec::new(),
    );
    let harness = ConductorHarness::new(ConductorRequest {
        workflow_id: "fallback-evaluation".to_string(),
        objective: "Compare two implementation strategies with evidence".to_string(),
        recent_context: String::new(),
        effort: "auto".to_string(),
        policy: "best_of_n".to_string(),
        conductor_model: "planner".to_string(),
        primary_model: "worker-a".to_string(),
        worker_models: vec!["worker-a".to_string(), "worker-b".to_string()],
        role_hints: ConductorRoleHints {
            planner: "worker-a".to_string(),
            executor: "worker-b".to_string(),
            reviewer: "worker-b".to_string(),
            synthesizer: "worker-a".to_string(),
        },
        budget: WorkflowBudget {
            max_steps: 3,
            max_models: 2,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            "auto",
            OrchestrationPolicy::BestOfN { candidates: 2 },
        ),
        prior_hint: None,
        prompt_evolution_enabled: true,
        prompt_genome: genome.clone(),
    });
    let mut calls = 0usize;

    let candidate = evaluate_conductor_prompt_profile_with_runner(&harness, &genome, |_| {
        calls += 1;
        CollaborationCompletion {
            content: Some("still not a workflow".to_string()),
            partial_content: None,
            error: None,
            failure: None,
            latency_ms: 5,
            usage: BTreeMap::new(),
            evidence: Vec::new(),
        }
    });

    let plan = candidate
        .plan
        .expect("failed conductor repair should use the deterministic graph");
    assert_eq!(calls, CONDUCTOR_MAX_ATTEMPTS);
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(
        plan.steps
            .iter()
            .take(2)
            .filter(|step| step.access.is_empty())
            .count(),
        2
    );
    assert!(candidate
        .raw_output
        .contains("Deterministic harness fallback applied"));
}

#[test]
fn execution_arena_runs_dependencies_before_final_synthesis() {
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let plan = WorkflowPlanIr::from_adaptive_with_profile(
        "arena-candidate",
        "Investigate and summarize",
        "auto",
        "best_of_n",
        "planner",
        profile.id.clone(),
        &AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "investigate".to_string(),
                    role: "worker".to_string(),
                    model: "worker-a".to_string(),
                    subtask: "investigate evidence".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "worker-b".to_string(),
                    subtask: "synthesize the result".to_string(),
                    access: vec!["investigate".to_string()],
                },
            ],
        },
        WorkflowBudget {
            max_steps: 4,
            max_models: 2,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 1,
            max_output_tokens_per_step: 2_048,
        },
    );
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&prompts);
    let runner: PromptEvaluationRunner = Arc::new(move |request, _| {
        captured
            .lock()
            .expect("prompt capture lock")
            .push(request.prompt);
        CollaborationCompletion {
            content: Some(if request.model == "worker-a" {
                "branch-output".to_string()
            } else {
                "final-output".to_string()
            }),
            partial_content: None,
            error: None,
            failure: None,
            latency_ms: 10,
            usage: [("total_tokens".to_string(), "20".to_string())]
                .into_iter()
                .collect(),
            evidence: Vec::new(),
        }
    });

    let candidate = execute_prompt_workflow_candidate_with_runner(
        "Investigate and summarize",
        PromptPlanCandidate {
            genome: profile,
            plan: Some(plan),
            raw_output: String::new(),
            latency_ms: 5,
            total_tokens: 10,
        },
        runner,
    );

    assert!(candidate.execution.succeeded);
    assert_eq!(candidate.execution.final_output, "final-output");
    assert_eq!(candidate.execution.steps.len(), 2);
    assert_eq!(candidate.execution.total_tokens, 40);
    let prompts = prompts.lock().expect("prompt capture lock");
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("[investigate status=completed]\nbranch-output"));
}

#[test]
fn evaluation_sandbox_exposes_and_executes_only_read_only_workspace_tools() {
    let root = temp_test_root("cindx-evaluation-sandbox");
    fs::create_dir_all(&root).expect("sandbox root should be created");
    fs::write(root.join("evidence.txt"), "verified workspace evidence")
        .expect("sandbox fixture should be written");
    let registry = ToolRegistry::with_workspace_tools(root.clone());

    let tools = prompt_evaluation_tool_specs(
        &registry,
        "Inspect evidence.txt",
        32_000,
        &WorkflowToolPolicy::ReadOnlyExploration,
    );
    assert!(!tools.is_empty());
    assert!(tools.iter().all(|tool| tool.risk == ToolRisk::ReadOnly));
    assert!(tools.iter().any(|tool| tool.name == "file.read"));
    assert!(!tools.iter().any(|tool| {
        matches!(
            tool.risk,
            ToolRisk::WritesWorkspace
                | ToolRisk::ExecutesProcess
                | ToolRisk::UsesNetwork
                | ToolRisk::SensitiveContext
                | ToolRisk::Destructive
        )
    }));
    assert!(prompt_evaluation_tool_specs(
        &registry,
        "Inspect evidence.txt",
        32_000,
        &WorkflowToolPolicy::None,
    )
    .is_empty());

    let result = registry
        .get("file.read")
        .expect("read tool should exist")
        .execute(ToolInvocation {
            id: agent_core::ToolCallId("evaluation-read".to_string()),
            task_id: phase16_task_id(),
            tool_name: "file.read".to_string(),
            input_json: serde_json::json!({"path":"evidence.txt"}).to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        })
        .expect("read-only sandbox tool should execute");
    assert_eq!(result.output, "verified workspace evidence");
    fs::remove_dir_all(root).expect("sandbox fixture should be removed");
}

#[test]
#[ignore = "requires the user's configured provider and network access"]
fn provider_backed_evaluation_sandbox_reads_real_workspace_evidence() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root should resolve");
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let objective = "Use read-only workspace tools to inspect the root Cargo.toml. Report one workspace member and include the exact token crates/orchestrator. Do not guess.";
    let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
        "provider-sandbox-smoke",
        objective,
        "auto",
        "single_worker",
        config.model_for_conductor(),
        profile.id.clone(),
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "inspect".to_string(),
                role: "worker".to_string(),
                model: config.model_for_role(&ModelRole::Executor),
                subtask: "Read the root Cargo.toml and report a verified workspace member."
                    .to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: 1,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
    );
    plan.steps[0].tool_policy = WorkflowToolPolicy::ReadOnlyEvidence;
    let candidate = execute_prompt_workflow_candidate(
        &config,
        &workspace_root,
        objective,
        PromptPlanCandidate {
            genome: profile,
            plan: Some(plan),
            raw_output: String::new(),
            latency_ms: 0,
            total_tokens: 0,
        },
        &Arc::new(AgentRunControl::new("pro")),
    );

    assert!(candidate.execution.succeeded);
    assert!(candidate
        .execution
        .final_output
        .to_ascii_lowercase()
        .contains("crates/orchestrator"));
    assert!(candidate.execution.steps[0]
        .tool_calls
        .iter()
        .any(|call| call.tool == "file.read"));
}

#[test]
#[ignore = "requires the user's configured provider and network access"]
fn provider_backed_paired_ablation_rewards_read_only_tool_evidence() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let workspace_root = temp_test_root("cindx-provider-ablation");
    fs::create_dir_all(&workspace_root).expect("ablation sandbox should be created");
    let hidden_fact = format!("CINDX-VERIFY-{}", unique_id("fact"));
    fs::write(workspace_root.join("evidence.txt"), &hidden_fact)
        .expect("hidden evidence should be written");
    let objective = "The read-only workspace contains evidence.txt with one verification code. Report the exact code. Use workspace evidence when available and never guess.";
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let plan = |id: &str, tool_policy: WorkflowToolPolicy| {
        let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
                id,
                objective,
                "auto",
                "single_worker",
                config.model_for_conductor(),
                profile.id.clone(),
                &AdaptiveWorkflow {
                    steps: vec![AdaptiveWorkflowStep {
                        id: "inspect".to_string(),
                        role: "worker".to_string(),
                        model: config.model_for_role(&ModelRole::Executor),
                        subtask: "Read evidence.txt when the sandbox exposes a read-only tool and report the exact code."
                            .to_string(),
                        access: Vec::new(),
                    }],
                },
                WorkflowBudget {
                    max_steps: 1,
                    max_models: 1,
                    max_model_turns_per_step: 3,
                    max_tool_calls_per_step: 4,
                    max_output_tokens_per_step: 2_048,
                },
            );
        plan.steps[0].tool_policy = tool_policy;
        plan
    };
    let run = |id: &str, tool_policy: WorkflowToolPolicy| {
        execute_prompt_workflow_candidate(
            &config,
            &workspace_root,
            objective,
            PromptPlanCandidate {
                genome: profile.clone(),
                plan: Some(plan(id, tool_policy)),
                raw_output: String::new(),
                latency_ms: 0,
                total_tokens: 0,
            },
            &Arc::new(AgentRunControl::new("pro")),
        )
    };
    let baseline = run("without-tools", WorkflowToolPolicy::None);
    let candidate = run("with-read-only-tools", WorkflowToolPolicy::ReadOnlyEvidence);
    let verifier = AgentEvaluationVerifier::ContainsAll {
        expected: vec![hidden_fact.clone()],
        case_sensitive: true,
    };
    let baseline_outcome = verifier.verify(&baseline.execution.final_output);
    let candidate_outcome = verifier.verify(&candidate.execution.final_output);
    fs::remove_dir_all(workspace_root).expect("ablation sandbox should be removed");

    assert!(!baseline_outcome.passed);
    assert!(candidate_outcome.passed);
    assert!(candidate.execution.steps[0]
        .tool_calls
        .iter()
        .any(|call| call.tool == "file.read" && call.response.contains(&hidden_fact)));
}

#[test]
#[ignore = "requires the user's configured provider and network access"]
fn provider_backed_gepa_reflection_repairs_a_disabled_tool_gene() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let mut parent = ConductorPromptGenome::seed_for_effort("auto");
    parent.id = "reflection-parent-tools-disabled".to_string();
    parent.tool_policy = orchestrator::PromptToolPolicy::Disabled;
    parent.max_tool_calls_per_step = 0;
    let failure = AgentEvaluationReflectionPacket {
            suite_id: "provider-reflection-smoke".to_string(),
            suite_version: 2,
            case_id: "feedback-random-workspace-fact".to_string(),
            category: "tool-use".to_string(),
            run_id: "provider-reflection-run".to_string(),
            seed: 0,
            candidate_id: parent.id.clone(),
            candidate_fingerprint: "reflection-parent-fingerprint".to_string(),
            model_fingerprints: BTreeMap::from([(
                "worker".to_string(),
                config.model_for_role(&ModelRole::Executor),
            )]),
            input: "Read an unpredictable value from evidence.txt and report it exactly."
                .to_string(),
            steps: vec![AgentEvaluationTraceStep {
                step_id: "inspect".to_string(),
                role: "worker".to_string(),
                model: config.model_for_role(&ModelRole::Executor),
                prompt: "No tools are available; report the exact unpredictable file value."
                    .to_string(),
                output: "I cannot access evidence.txt without a workspace tool.".to_string(),
                tool_calls: Vec::new(),
                errors: vec!["required workspace evidence was unavailable".to_string()],
                latency_ms: 100,
                total_tokens: 50,
            }],
            final_output: "I cannot access evidence.txt without a workspace tool.".to_string(),
            verifier: AgentEvaluationVerifierOutcome {
                source: AgentEvaluationEvidenceSource::Deterministic,
                passed: false,
                score: 0.0,
                checks: vec![AgentEvaluationCheck {
                    id: "contains_unpredictable_value".to_string(),
                    passed: false,
                    detail: "the output omitted the exact value stored in evidence.txt".to_string(),
                }],
            },
            actionable_feedback: ActionableSideInformation {
                summary: "The harness disabled the only safe evidence path required by this task."
                    .to_string(),
                failed_constraints: vec![
                    "The worker could not inspect a required workspace file.".to_string(),
                ],
                errors: vec!["No read-only workspace tool was exposed.".to_string()],
                suggested_changes: vec![
                    "Enable the existing read-only evidence tool policy; do not add writes or network access."
                        .to_string(),
                ],
                ..ActionableSideInformation::default()
            },
        };
    let mutation_prompt = parent
        .reflective_mutation_prompt(&[failure])
        .expect("reflection prompt should build");
    let control = Arc::new(AgentRunControl::new("pro"));
    let completion = complete_collaboration_model_with_control(
        config.clone(),
        ModelRole::Planner,
        config.model_for_conductor(),
        collaboration_system_prompt_for_run(&config.agent_system_prompt, &Metadata::new()),
        mutation_prompt,
        Some(control),
        |_| {},
    );
    let response = completion
        .content
        .unwrap_or_else(|| panic!("reflection model failed: {:?}", completion.error));
    let mutation = match parent
        .learned_mutation_from_response(&response, "reflection-child-tools-enabled")
    {
        Ok(mutation) => mutation,
        Err(error) => {
            let repair = complete_collaboration_model_with_control(
                config.clone(),
                ModelRole::Planner,
                config.model_for_conductor(),
                collaboration_system_prompt_for_run(&config.agent_system_prompt, &Metadata::new()),
                parent.mutation_repair_prompt(&response, &error),
                Some(Arc::new(AgentRunControl::new("pro"))),
                |_| {},
            );
            let repaired_response = repair
                .content
                .unwrap_or_else(|| panic!("reflection repair model failed: {:?}", repair.error));
            parent
                .learned_mutation_from_response(
                    &repaired_response,
                    "reflection-child-tools-enabled",
                )
                .expect("repaired reflection should be a bounded genome mutation")
        }
    };

    assert_ne!(
        mutation.tool_policy,
        orchestrator::PromptToolPolicy::Disabled
    );
    assert!(mutation.effective_max_tool_calls_per_step() > 0);
    mutation.validate().expect("mutation should remain valid");
}

#[test]
#[ignore = "runs the full 30-case by 3-repeat provider-backed hidden gate"]
fn provider_backed_hidden_gate_compares_pre_gepa_and_read_only_sandbox() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root should resolve");
    let workspace_root = temp_test_root("cindx-provider-hidden-gate");
    let evidence_root = workspace_root.join("evidence");
    fs::create_dir_all(&evidence_root).expect("hidden evidence root should be created");
    let cases = (0..30usize)
            .map(|index| {
                let case_id = format!("provider-hidden-{index:02}");
                let category = match index % 3 {
                    0 => "coding",
                    1 => "research",
                    _ => "tool-use",
                }
                .to_string();
                let relative_path = format!("evidence/{case_id}.txt");
                let expected = format!("CINDX-HIDDEN-{}", unique_id("fact"));
                fs::write(workspace_root.join(&relative_path), &expected)
                    .expect("hidden evidence should be written");
                let objective = format!(
                    "The read-only workspace contains {relative_path} with one unpredictable verification code. Report the exact code. Use workspace evidence when available and never guess."
                );
                (case_id, category, relative_path, objective, expected)
            })
            .collect::<Vec<_>>();
    let dataset = orchestrator::AgentEvaluationDataset {
        schema: orchestrator::AGENT_EVALUATION_DATASET_SCHEMA.to_string(),
        suite_id: "core-agent-quality".to_string(),
        suite_version: 2,
        split: AgentEvaluationSplit::Test,
        description: "Runtime-generated provider-backed hidden workspace facts.".to_string(),
        cases: cases
            .iter()
            .map(
                |(case_id, category, _, objective, expected)| orchestrator::AgentEvaluationCase {
                    id: case_id.clone(),
                    category: category.clone(),
                    objective: objective.clone(),
                    verifier: AgentEvaluationVerifier::ContainsAll {
                        expected: vec![expected.clone()],
                        case_sensitive: true,
                    },
                    metadata: BTreeMap::from([(
                        "provenance".to_string(),
                        "runtime-random-fact".to_string(),
                    )]),
                },
            )
            .collect(),
    };
    dataset.validate().expect("hidden dataset should validate");
    let dataset_json =
        serde_json::to_string_pretty(&dataset).expect("hidden dataset should serialize");
    let dataset_sha256 = sha256_hex(dataset_json.as_bytes());
    let jobs = Arc::new(Mutex::new(
        (0..cases.len())
            .flat_map(|case_index| (0..3u64).map(move |seed| (case_index, seed)))
            .rev()
            .collect::<Vec<_>>(),
    ));
    let cases = Arc::new(cases);
    let records = Arc::new(Mutex::new((
        Vec::<AgentEvaluationCaseScore>::new(),
        Vec::<AgentEvaluationCaseScore>::new(),
    )));
    let baseline_id = "pre-gepa-no-evaluation-tools".to_string();
    let candidate_id = "gepa-read-only-sandbox".to_string();
    let baseline_fingerprint = sha256_hex(baseline_id.as_bytes());
    let candidate_profile = ConductorPromptGenome::seed_for_effort("auto");
    let candidate_fingerprint = sha256_hex(
        &serde_json::to_vec(&candidate_profile).expect("candidate genome should serialize"),
    );
    let concurrency = std::env::var("CINDX_EVAL_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 8);
    let handles = (0..concurrency)
            .map(|_| {
                let config = config.clone();
                let workspace_root = workspace_root.clone();
                let jobs = Arc::clone(&jobs);
                let cases = Arc::clone(&cases);
                let records = Arc::clone(&records);
                let baseline_id = baseline_id.clone();
                let candidate_id = candidate_id.clone();
                let baseline_fingerprint = baseline_fingerprint.clone();
                let candidate_fingerprint = candidate_fingerprint.clone();
                let candidate_profile = candidate_profile.clone();
                std::thread::spawn(move || loop {
                    let Some((case_index, seed)) = jobs
                        .lock()
                        .expect("hidden job lock")
                        .pop()
                    else {
                        break;
                    };
                    let (case_id, category, relative_path, objective, expected) =
                        &cases[case_index];
                    let run = |workflow_id: &str, tool_policy: WorkflowToolPolicy| {
                        let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
                            workflow_id,
                            objective,
                            "auto",
                            "single_worker",
                            config.model_for_conductor(),
                            candidate_profile.id.clone(),
                            &AdaptiveWorkflow {
                                steps: vec![AdaptiveWorkflowStep {
                                    id: "inspect".to_string(),
                                    role: "worker".to_string(),
                                    model: config.model_for_role(&ModelRole::Executor),
                                    subtask: format!(
                                        "Read {relative_path} when a read-only tool is exposed and report its exact unpredictable code."
                                    ),
                                    access: Vec::new(),
                                }],
                            },
                            WorkflowBudget {
                                max_steps: 1,
                                max_models: 1,
                                max_model_turns_per_step: 3,
                                max_tool_calls_per_step: 4,
                                max_output_tokens_per_step: 2_048,
                            },
                        );
                        plan.steps[0].tool_policy = tool_policy;
                        execute_prompt_workflow_candidate(
                            &config,
                            &workspace_root,
                            objective,
                            PromptPlanCandidate {
                                genome: candidate_profile.clone(),
                                plan: Some(plan),
                                raw_output: String::new(),
                                latency_ms: 0,
                                total_tokens: 0,
                            },
                            &Arc::new(AgentRunControl::new("pro")),
                        )
                    };
                    let baseline = run(
                        &format!("baseline-{case_index}-{seed}"),
                        WorkflowToolPolicy::None,
                    );
                    let candidate = run(
                        &format!("candidate-{case_index}-{seed}"),
                        WorkflowToolPolicy::ReadOnlyEvidence,
                    );
                    let verifier = AgentEvaluationVerifier::ContainsAll {
                        expected: vec![expected.clone()],
                        case_sensitive: true,
                    };
                    let baseline_outcome = verifier.verify(&baseline.execution.final_output);
                    let candidate_outcome = verifier.verify(&candidate.execution.final_output);
                    let record = |candidate_id: &str,
                                  candidate_fingerprint: &str,
                                  execution: &PromptWorkflowExecution,
                                  outcome: &AgentEvaluationVerifierOutcome| {
                        AgentEvaluationCaseScore {
                            suite_id: "core-agent-quality".to_string(),
                            suite_version: 2,
                            case_id: case_id.clone(),
                            category: category.clone(),
                            split: AgentEvaluationSplit::Test,
                            run_id: format!("{candidate_id}-{case_index}-{seed}"),
                            seed,
                            candidate_id: candidate_id.to_string(),
                            candidate_fingerprint: candidate_fingerprint.to_string(),
                            evidence_source: AgentEvaluationEvidenceSource::Deterministic,
                            score: if execution.succeeded {
                                outcome.score
                            } else {
                                0.0
                            },
                            verified_success: execution.succeeded && outcome.passed,
                            latency_ms: execution.latency_ms,
                            total_tokens: execution.total_tokens,
                            safety_violations: 0,
                        }
                    };
                    let mut records = records.lock().expect("hidden record lock");
                    records.0.push(record(
                        &baseline_id,
                        &baseline_fingerprint,
                        &baseline.execution,
                        &baseline_outcome,
                    ));
                    records.1.push(record(
                        &candidate_id,
                        &candidate_fingerprint,
                        &candidate.execution,
                        &candidate_outcome,
                    ));
                })
            })
            .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("provider hidden worker should join");
    }
    let (mut baseline_records, mut candidate_records) = Arc::try_unwrap(records)
        .expect("hidden records should have one owner")
        .into_inner()
        .expect("hidden record lock should unwrap");
    let sort_records = |records: &mut Vec<AgentEvaluationCaseScore>| {
        records.sort_by(|left, right| {
            left.case_id
                .cmp(&right.case_id)
                .then(left.seed.cmp(&right.seed))
        });
    };
    sort_records(&mut baseline_records);
    sort_records(&mut candidate_records);
    let baseline_scores = orchestrator::AgentEvaluationScoreSet {
        schema: orchestrator::AGENT_EVALUATION_SCORE_SET_SCHEMA.to_string(),
        suite_id: "core-agent-quality".to_string(),
        suite_version: 2,
        split: AgentEvaluationSplit::Test,
        candidate_id: baseline_id,
        candidate_fingerprint: baseline_fingerprint,
        dataset_sha256: dataset_sha256.clone(),
        provenance: orchestrator::AgentEvaluationRunProvenance::ProviderBacked,
        records: baseline_records,
    };
    let candidate_scores = orchestrator::AgentEvaluationScoreSet {
        schema: orchestrator::AGENT_EVALUATION_SCORE_SET_SCHEMA.to_string(),
        suite_id: "core-agent-quality".to_string(),
        suite_version: 2,
        split: AgentEvaluationSplit::Test,
        candidate_id,
        candidate_fingerprint,
        dataset_sha256,
        provenance: orchestrator::AgentEvaluationRunProvenance::ProviderBacked,
        records: candidate_records,
    };
    let baseline = orchestrator::parse_agent_evaluation_baseline(
        &fs::read_to_string(repository_root.join("benchmarks/agent/evaluation-v2-baseline.json"))
            .expect("frozen evaluation baseline should load"),
    )
    .expect("frozen evaluation baseline should parse");
    let report = orchestrator::build_agent_evaluation_promotion_report(
        &baseline,
        &baseline_scores,
        &candidate_scores,
    )
    .expect("provider hidden report should build");
    let hidden_root = repository_root.join("benchmarks/agent/hidden");
    fs::create_dir_all(&hidden_root).expect("ignored hidden report root should exist");
    fs::write(hidden_root.join("provider-test.json"), dataset_json)
        .expect("provider hidden dataset should persist");
    fs::write(
        hidden_root.join("provider-baseline-scores.json"),
        serde_json::to_string_pretty(&baseline_scores).expect("baseline scores should serialize"),
    )
    .expect("baseline scores should persist");
    fs::write(
        hidden_root.join("provider-candidate-scores.json"),
        serde_json::to_string_pretty(&candidate_scores).expect("candidate scores should serialize"),
    )
    .expect("candidate scores should persist");
    fs::write(
        repository_root.join("target/evaluation-v2-provider-promotion.json"),
        serde_json::to_string_pretty(&report).expect("promotion report should serialize"),
    )
    .expect("promotion report should persist");
    fs::remove_dir_all(workspace_root).expect("provider hidden workspace should be removed");

    assert!(
        report.promotion_eligible,
        "provider hidden gate: {report:#?}"
    );
    assert_eq!(report.recommended_canary_percent, Some(10));
}

#[test]
fn evaluation_arena_applies_retry_and_alternate_model_genes() {
    let mut profile = ConductorPromptGenome::seed_for_effort("auto");
    profile.max_step_attempts = 2;
    profile.retry_policy = PromptRetryPolicy::AlternateModel;
    let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
        "retry-candidate",
        "Investigate and summarize",
        "auto",
        "best_of_n",
        "planner",
        profile.id.clone(),
        &AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "investigate".to_string(),
                    role: "worker".to_string(),
                    model: "worker-a".to_string(),
                    subtask: "investigate evidence".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "worker-b".to_string(),
                    subtask: "synthesize".to_string(),
                    access: vec!["investigate".to_string()],
                },
            ],
        },
        WorkflowBudget {
            max_steps: 4,
            max_models: 2,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
    );
    plan.steps[0].tool_policy = WorkflowToolPolicy::ReadOnlyEvidence;
    let requests = Arc::new(Mutex::new(Vec::<PromptEvaluationWorkerRequest>::new()));
    let captured = Arc::clone(&requests);
    let runner: PromptEvaluationRunner = Arc::new(move |request, _| {
        let should_fail = request.model == "worker-a";
        captured.lock().expect("request capture lock").push(request);
        CollaborationCompletion {
            content: (!should_fail).then(|| "recovered output".to_string()),
            partial_content: None,
            error: should_fail.then(|| "worker-a failed".to_string()),
            failure: should_fail.then(|| {
                AgentFailure::new(
                    "provider_unavailable",
                    "worker-a failed",
                    AgentFailureClass::ProviderTransient,
                    true,
                )
            }),
            latency_ms: 10,
            usage: [("total_tokens".to_string(), "20".to_string())]
                .into_iter()
                .collect(),
            evidence: Vec::new(),
        }
    });

    let candidate = execute_prompt_workflow_candidate_with_runner(
        "Investigate and summarize",
        PromptPlanCandidate {
            genome: profile,
            plan: Some(plan),
            raw_output: String::new(),
            latency_ms: 0,
            total_tokens: 0,
        },
        runner,
    );

    assert!(candidate.execution.succeeded);
    assert_eq!(candidate.execution.steps[0].attempts, 2);
    assert_eq!(candidate.execution.steps[0].model, "worker-b");
    assert_eq!(candidate.execution.steps[0].errors, vec!["worker-a failed"]);
    let requests = requests.lock().expect("request capture lock");
    assert_eq!(
        requests[0].tool_policy,
        WorkflowToolPolicy::ReadOnlyEvidence
    );
    assert_eq!(requests[0].max_model_turns, 2);
    assert_eq!(requests[0].max_tool_calls, 4);
    assert!(requests[1]
        .prompt
        .contains("Retry the same authorized evaluation node"));
}

#[test]
fn timeline_exposes_durable_workflow_progress() {
    let event = Event {
        id: EventId("checkpoint-progress".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 100,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow step checkpointed".to_string(),
        metadata: [
            (
                "workflow_checkpoint_schema".to_string(),
                WORKFLOW_CHECKPOINT_SCHEMA.to_string(),
            ),
            ("workflow_steps".to_string(), "5".to_string()),
            ("completed_steps".to_string(), "2".to_string()),
            ("step_id".to_string(), "review".to_string()),
            ("step_status".to_string(), "completed".to_string()),
            ("workflow_continuations".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    let progress = timeline_workflow_progress(&event).unwrap();

    assert_eq!(progress.completed_steps, 2);
    assert_eq!(progress.total_steps, 5);
    assert_eq!(progress.current_step_id.as_deref(), Some("review"));
    assert_eq!(progress.continuations, 1);
    assert!(progress.recoverable);
}

#[test]
fn prompt_evolution_waits_for_the_final_agent_outcome() {
    let seed = ConductorPromptGenome::seed_for_effort("pro");
    let context = [
        ("collaboration_id".to_string(), "collab-final".to_string()),
        ("agent_run_id".to_string(), "run-final".to_string()),
        ("prompt_profile".to_string(), seed.id.clone()),
        ("prompt_effort".to_string(), "pro".to_string()),
        (
            "prompt_genome".to_string(),
            serde_json::to_string(&seed).unwrap(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut events = vec![
        Event {
            id: EventId("profile".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        },
        Event {
            id: EventId("collaboration-complete".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: context.clone(),
        },
    ];

    assert!(prompt_evolution_observations_from_events(&events).is_empty());
    events.push(Event {
        id: EventId("agent-failed".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 300,
        kind: EventKind::Error,
        summary: "Agent task failed".to_string(),
        metadata: context,
    });
    assert!(prompt_evolution_observations_from_events(&events).is_empty());
}

#[test]
fn prompt_evolution_censors_an_untrusted_anchor_delivery() {
    let seed = ConductorPromptGenome::seed_for_effort("pro");
    let context = [
        ("collaboration_id".to_string(), "collab-anchor".to_string()),
        ("agent_run_id".to_string(), "run-anchor".to_string()),
        ("prompt_profile".to_string(), seed.id.clone()),
        ("prompt_effort".to_string(), "pro".to_string()),
        (
            "prompt_genome".to_string(),
            serde_json::to_string(&seed).unwrap(),
        ),
        ("collaboration_profile".to_string(), "bounded".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = vec![
        Event {
            id: EventId("profile-anchor".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        },
        Event {
            id: EventId("workflow-anchor".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: metadata_with_context(
                [
                    (
                        "anytime_selected_candidate".to_string(),
                        DIRECT_ANCHOR_CANDIDATE_ID.to_string(),
                    ),
                    (
                        "anytime_prompt_learning_eligible".to_string(),
                        "false".to_string(),
                    ),
                    ("anytime_team_uplift_bps".to_string(), "-800".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("agent-anchor".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 300,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task completed".to_string(),
            metadata: metadata_with_context(
                [("routing_learning_eligible".to_string(), "true".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
    ];

    assert!(prompt_evolution_observations_from_events(&events).is_empty());
}

#[test]
fn prompt_evolution_censors_user_cancellation() {
    let seed = ConductorPromptGenome::seed_for_effort("pro");
    let context = [
        (
            "collaboration_id".to_string(),
            "collab-cancelled".to_string(),
        ),
        ("agent_run_id".to_string(), "run-cancelled".to_string()),
        ("prompt_profile".to_string(), seed.id.clone()),
        ("prompt_effort".to_string(), "pro".to_string()),
        (
            "prompt_genome".to_string(),
            serde_json::to_string(&seed).unwrap(),
        ),
        ("collaboration_profile".to_string(), "bounded".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = vec![
        Event {
            id: EventId("profile-cancelled".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        },
        Event {
            id: EventId("workflow-cancelled".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow failed".to_string(),
            metadata: metadata_with_context(
                [(
                    "anytime_native_effort_success".to_string(),
                    "false".to_string(),
                )]
                .into_iter()
                .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("agent-cancelled".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 300,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task cancelled".to_string(),
            metadata: metadata_with_context(
                [("reason".to_string(), "user_cancelled".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
    ];

    assert!(prompt_evolution_observations_from_events(&events).is_empty());
}

#[test]
fn scheduled_queue_progress_tracks_permission_and_completion() {
    let queue_id = "schedule-queue";
    let event = |sequence: u64, summary: &str, queue_action: Option<&str>| Event {
        id: EventId(format!("schedule-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 100,
        kind: EventKind::TaskStatusChanged,
        summary: summary.to_string(),
        metadata: [
            ("queue_id".to_string(), queue_id.to_string()),
            ("session_id".to_string(), "session-a".to_string()),
        ]
        .into_iter()
        .chain(queue_action.map(|action| ("queue_action".to_string(), action.to_string())))
        .collect(),
    };
    let mut events = vec![
        event(1, "Agent message queued", Some("enqueue")),
        event(2, "Queued agent message started", Some("start")),
        event(3, "Agent task started", None),
        event(4, "Agent task waiting for permission", None),
    ];

    let waiting = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(waiting.status, "waiting_for_permission");
    assert_eq!(waiting.started_at_ms, Some(200));
    assert_eq!(
        latest_unfinished_agent_queue_id(&events).as_deref(),
        Some(queue_id)
    );

    events.push(event(5, "Agent task paused", None));
    let paused = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(paused.status, "paused");
    assert_eq!(paused.finished_at_ms, Some(500));
    assert!(latest_unfinished_agent_queue_id(&events).is_none());

    events.push(event(6, "Agent task retry started", None));
    events.push(event(7, "Agent task completed", None));
    let completed = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.finished_at_ms, Some(700));
    assert!(latest_unfinished_agent_queue_id(&events).is_none());
}

#[test]
fn restored_scheduled_queue_is_retryable_instead_of_running() {
    let queue_id = "schedule-queue";
    let events = vec![
        Event {
            id: EventId("schedule-enqueue".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent message queued".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "enqueue".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("schedule-start".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Queued agent message started".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "start".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("schedule-restore".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 300,
            kind: EventKind::TaskStatusChanged,
            summary: "Queued agent message restored".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "restore".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ];

    let progress = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(progress.status, "queued");
    assert_eq!(progress.started_at_ms, None);
    assert_eq!(progress.finished_at_ms, None);
}

#[test]
fn coding_retrieval_mode_skips_graph_channels() {
    let root = temp_test_root("phase7-selective-retrieval");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("notes.md"), "Cindx selective retrieval source")
        .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    replace_lancedb_index(lancedb_database_path_for(&root), adapter.index())
        .expect("LanceDB index should persist");
    let cancellation = Arc::new(AgentRunControl::new("auto"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "selective retrieval source",
        4,
        "semantic_literal_parallel",
        None,
        &cancellation,
        None,
    )
    .expect("retrieval should run");

    assert_eq!(
        retrieval
            .trace
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>(),
        vec!["semantic_rag", "file_search"]
    );
}

#[test]
fn retrieval_keeps_file_evidence_when_semantic_channel_is_unavailable() {
    let root = temp_test_root("phase7-independent-channel-failure");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "independent lexical fallback evidence",
    )
    .expect("fixture should write");
    let mut index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    for chunk in &mut index.chunks {
        chunk.embedding_provider = "cloud".to_string();
        chunk.embedding_model = "cloud-embedding".to_string();
    }
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let cancellation = Arc::new(AgentRunControl::new("auto"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "independent lexical fallback evidence",
        4,
        "semantic_literal_parallel",
        None,
        &cancellation,
        None,
    )
    .expect("independent channels should degrade without failing the retrieval");

    let semantic = retrieval
        .trace
        .channels
        .iter()
        .find(|channel| channel.name == "semantic_rag")
        .expect("semantic channel");
    let file = retrieval
        .trace
        .channels
        .iter()
        .find(|channel| channel.name == "file_search")
        .expect("file channel");
    assert!(semantic.error.is_some());
    assert!(file.error.is_none());
    assert!(file.result_count > 0);
    assert!(!retrieval.results.is_empty());
}

#[test]
fn workspace_cache_ttl_advances_only_after_validation_or_index_change() {
    assert!(!workspace_knowledge_cache_needs_refresh(true, false));
    assert!(workspace_knowledge_cache_needs_refresh(false, false));
    assert!(workspace_knowledge_cache_needs_refresh(true, true));
}

#[test]
fn workspace_knowledge_snapshot_reuses_one_graph_parse_and_borrowed_projection() {
    let root = temp_test_root("phase7-shared-graph-snapshot");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "SharedGraphMarker uses file.read with docs/reference.md",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("graph should build");
    let mut adapter = open_rag_adapter_for(&root).expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");

    reset_graph_store_open_count();
    let entry = workspace_knowledge_cache_entry(&adapter).expect("cache entry should build");
    assert_eq!(graph_store_open_count(), 1);
    let first = entry
        .snapshot_if_current(adapter.path())
        .expect("current cache entry should produce a snapshot");
    let second = entry
        .snapshot_if_current(adapter.path())
        .expect("repeated cache hit should produce a snapshot");
    let first_graph = first
        .graph_store
        .as_ref()
        .expect("snapshot should include graph");
    let second_graph = second
        .graph_store
        .as_ref()
        .expect("snapshot should include graph");
    assert!(Arc::ptr_eq(first_graph, second_graph));
    assert_eq!(
        first_graph.path(),
        crate::knowledge_generation_runtime::knowledge_paths_for_rag_index(first.adapter.path())
            .graph_store
    );

    let focus_paths = vec!["notes.md".to_string()];
    let borrowed = graph_state_for_snapshot(&first, &focus_paths);
    assert_eq!(graph_store_open_count(), 1);
    assert!(borrowed.nodes.len() <= 80);
    assert!(borrowed.edges.len() <= 140);
    let from_disk = graph_state_at_path(first_graph.path(), &focus_paths)
        .expect("disk projection should remain available");
    assert_eq!(graph_store_open_count(), 2);
    assert_eq!(
        serde_json::to_value(&borrowed).expect("borrowed graph should serialize"),
        serde_json::to_value(&from_disk).expect("disk graph should serialize")
    );

    fs::remove_file(first_graph.path()).expect("graph fixture should be removable");
    let after_removal = graph_state_for_snapshot(&second, &focus_paths);
    assert_eq!(graph_store_open_count(), 2);
    assert_eq!(
        serde_json::to_value(&after_removal).expect("leased graph should serialize"),
        serde_json::to_value(&borrowed).expect("original graph should serialize")
    );
    println!(
        "{{\"schema\":\"cindx.workspace-graph-cache-scaling.v1\",\"cache_hit_graph_opens\":1,\"final_graph_opens\":{},\"shared_graph_store\":true,\"borrowed_nodes\":{},\"borrowed_edges\":{}}}",
        graph_store_open_count(),
        borrowed.nodes.len(),
        borrowed.edges.len()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn workspace_knowledge_snapshot_preserves_ttl_and_generation_validation() {
    let root = temp_test_root("phase7-shared-graph-validation");
    fs::create_dir_all(&root).expect("temp root should exist");
    let adapter = open_rag_adapter_for(&root).expect("adapter should open");
    let mut entry = workspace_knowledge_cache_entry(&adapter).expect("cache entry should build");

    assert!(entry.snapshot_if_current(adapter.path()).is_some());
    assert!(entry
        .snapshot_if_current(&root.join(".cindx").join("different-generation.tsv"))
        .is_none());
    entry.validated_at = Instant::now() - WORKSPACE_KNOWLEDGE_CACHE_TTL - Duration::from_millis(1);
    assert!(entry.snapshot_if_current(adapter.path()).is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cold_knowledge_state_reads_stats_without_opening_the_adapter_or_graph() {
    let root = temp_test_root("phase7-lightweight-cold-state");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("notes.md"), "LightweightColdStateMarker").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let expected_stats = index.stats.clone();
    index_graph_chunks(&root, &index.chunks).expect("graph should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let cache = Mutex::new(BTreeMap::new());

    reset_rag_adapter_open_count();
    reset_graph_store_open_count();
    let cold = active_workspace_knowledge_state_snapshot_in(&cache, &root)
        .expect("cold state should load lightweight stats");

    assert!(cold.full.is_none());
    assert_eq!(cold.active_index_path, adapter.path());
    assert_eq!(cold.stats, expected_stats);
    assert_eq!(rag_adapter_open_count(), 0);
    assert_eq!(graph_store_open_count(), 0);

    let entry = workspace_knowledge_cache_entry(&adapter).expect("full cache entry should build");
    cache
        .lock()
        .expect("cache should lock")
        .insert(workspace_knowledge_cache_key(&root), entry);
    reset_rag_adapter_open_count();
    reset_graph_store_open_count();
    let warm = active_workspace_knowledge_state_snapshot_in(&cache, &root)
        .expect("warm state should reuse the exact active generation");

    assert_eq!(warm.active_index_path, adapter.path());
    assert_eq!(
        warm.full
            .as_ref()
            .expect("warm state should retain the full snapshot")
            .adapter
            .path(),
        adapter.path()
    );
    assert_eq!(warm.stats, expected_stats);
    assert_eq!(rag_adapter_open_count(), 0);
    assert_eq!(graph_store_open_count(), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn malformed_lightweight_header_falls_back_to_full_adapter_stats_without_graph_open() {
    let root = temp_test_root("phase7-lightweight-stats-fallback");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("notes.md"), "LegacyStatsFallbackMarker").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let expected_chunks = index.chunks.len();
    let index_path = root.join(".cindx").join("rag-index.tsv");
    let mut adapter = FileRagAdapter::open(&index_path).expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let persisted = fs::read_to_string(&index_path).expect("index fixture should read");
    let (_, tail) = persisted
        .split_once('\n')
        .expect("non-empty index should have a chunk tail");
    fs::write(&index_path, format!("legacy-stats-header\n{tail}"))
        .expect("legacy header fixture should write");
    let cache = Mutex::new(BTreeMap::new());

    reset_rag_adapter_open_count();
    reset_graph_store_open_count();
    let state = active_workspace_knowledge_state_snapshot_in(&cache, &root)
        .expect("legacy stats should fall back to the full adapter");

    assert!(state.full.is_none());
    assert_eq!(state.stats.chunks_indexed, expected_chunks);
    assert_eq!(rag_adapter_open_count(), 1);
    assert_eq!(graph_store_open_count(), 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_generation_cannot_overwrite_the_active_knowledge_cache() {
    let root = temp_test_root("phase7-stale-generation-cache");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("first.md"), "first generation cache evidence")
        .expect("first fixture should write");
    let first_index =
        index_workspace(&root, IndexOptions::default()).expect("first index should build");
    let first =
        crate::knowledge_generation_runtime::build_and_publish_knowledge_generation_cancellable(
            &root,
            first_index,
            || false,
        )
        .expect("first generation should publish");

    let cache = Arc::new(Mutex::new(BTreeMap::new()));
    let old_ready = Arc::new(std::sync::Barrier::new(2));
    let release_old = Arc::new(std::sync::Barrier::new(2));
    let old_cache = Arc::clone(&cache);
    let old_root = root.clone();
    let old_adapter = first.adapter.clone();
    let old_ready_worker = Arc::clone(&old_ready);
    let release_old_worker = Arc::clone(&release_old);
    let old_writer = std::thread::spawn(move || {
        old_ready_worker.wait();
        release_old_worker.wait();
        cache_rag_adapter_in(&old_cache, &old_root, &old_adapter)
    });
    old_ready.wait();

    fs::write(
        root.join("second.rs"),
        "struct SecondGenerationCacheMarker; fn second_generation_cache_marker() {}",
    )
    .expect("second fixture should write");
    let second_index =
        index_workspace(&root, IndexOptions::default()).expect("second index should build");
    let second =
        crate::knowledge_generation_runtime::build_and_publish_knowledge_generation_cancellable(
            &root,
            second_index,
            || false,
        )
        .expect("second generation should publish");

    reset_graph_store_open_count();
    assert!(cache_rag_adapter_in(&cache, &root, &second.adapter)
        .expect("active generation should enter the cache"));
    assert_eq!(graph_store_open_count(), 1);
    release_old.wait();
    assert!(!old_writer
        .join()
        .expect("old cache writer should join")
        .expect("old cache writer should remain non-fatal"));

    let key = workspace_knowledge_cache_key(&root);
    let cache = cache.lock().expect("knowledge cache should lock");
    let entry = cache.get(&key).expect("active cache entry should remain");
    assert_eq!(entry.adapter.path(), second.adapter.path());
    let first_snapshot = entry
        .snapshot_if_current(second.adapter.path())
        .expect("active cache entry should produce a snapshot");
    let second_snapshot = entry
        .snapshot_if_current(second.adapter.path())
        .expect("repeated cache hit should produce a snapshot");
    assert!(Arc::ptr_eq(
        first_snapshot
            .graph_store
            .as_ref()
            .expect("first snapshot should include graph"),
        second_snapshot
            .graph_store
            .as_ref()
            .expect("second snapshot should include graph"),
    ));
    assert_eq!(graph_store_open_count(), 1);
    drop(cache);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn empty_workspace_knowledge_generation_is_reused() {
    let root = temp_test_root("phase7-empty-generation");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("reference.png"), b"not indexable text").expect("fixture should write");
    let mut adapter = open_rag_adapter_for(&root).expect("adapter should open");
    let initial_index_path = adapter.path().to_path_buf();
    let cancellation = Arc::new(AgentRunControl::new("auto"));
    let expected_epoch = cancellation.steer_epoch();

    let first = ensure_workspace_knowledge_index(WorkspaceKnowledgeIndexRequest {
        workspace_root: &root,
        adapter: &mut adapter,
        cache_hit: false,
        config: &ProviderConfig::default(),
        cancellation: &cancellation,
        expected_epoch,
        resource_checkpoint: None,
        rag_operation: None,
    })
    .expect("empty generation should publish");
    assert_eq!(
        first
            .expect("first ensure should index")
            .stats
            .chunks_indexed,
        0
    );
    let first_index_path = adapter.path().to_path_buf();
    assert_ne!(first_index_path, initial_index_path);

    let second = ensure_workspace_knowledge_index(WorkspaceKnowledgeIndexRequest {
        workspace_root: &root,
        adapter: &mut adapter,
        cache_hit: true,
        config: &ProviderConfig::default(),
        cancellation: &cancellation,
        expected_epoch,
        resource_checkpoint: None,
        rag_operation: None,
    })
    .expect("fresh empty generation should validate");
    assert!(second.is_none());
    assert_eq!(adapter.path(), first_index_path);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn interactive_observation_tools_do_not_invalidate_workspace_knowledge() {
    assert!(!tool_may_mutate_workspace(
        "browser.click",
        &ToolRisk::UsesNetwork
    ));
    assert!(!tool_may_mutate_workspace(
        "computer.key",
        &ToolRisk::Destructive
    ));
    assert!(!tool_may_mutate_workspace("file.read", &ToolRisk::ReadOnly));
    assert!(tool_may_mutate_workspace(
        "file.write",
        &ToolRisk::WritesWorkspace
    ));
    assert!(tool_may_mutate_workspace(
        "shell.run",
        &ToolRisk::ExecutesProcess
    ));
}

#[test]
fn graph_walk_seed_fusion_includes_semantic_and_file_evidence() {
    let root = temp_test_root("phase7-graph-seeds");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "semantic source alpha").expect("fixture should write");
    fs::write(root.join("b.md"), "direct source beta").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let semantic = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.9,
    };
    let file = RagSearchResult {
        chunk: index.chunks[1].clone(),
        score: 0.8,
    };
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![semantic.clone()],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: Vec::new(),
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![file.clone()],
            error: None,
        },
    ];

    let seeds = graph_walk_seed_results(&channels, 8);

    assert_eq!(seeds.len(), 2);
    assert_eq!(seeds[0].chunk.id, semantic.chunk.id);
    assert_eq!(seeds[1].chunk.id, file.chunk.id);
}

#[test]
fn complex_retrieval_runs_parallel_seed_channels_before_graph_walk() {
    let root = temp_test_root("phase7-four-way-retrieval");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "Cindx graph retrieval connects workspace evidence",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("graph should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    replace_lancedb_index(lancedb_database_path_for(&root), adapter.index())
        .expect("LanceDB index should persist");
    let cancellation = Arc::new(AgentRunControl::new("pro"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "graph retrieval workspace evidence",
        4,
        "four_way_parallel",
        None,
        &cancellation,
        None,
    )
    .expect("retrieval should run");

    let channel_names = retrieval
        .trace
        .channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        channel_names.iter().copied().collect::<BTreeSet<_>>(),
        ["semantic_rag", "graph_recall", "file_search", "graph_walk"]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(channel_names.last().copied(), Some("graph_walk"));
    assert!(retrieval
        .trace
        .channels
        .iter()
        .all(|channel| channel.error.is_none()));
    assert!(!retrieval.results.is_empty());
}

#[test]
fn graph_index_cancellation_preserves_previous_cache() {
    let root = temp_test_root("phase7-graph-cancel");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "graph source").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("initial graph should build");
    let graph_path = graph_store_path_for(&root);
    let before = fs::read(&graph_path).expect("initial graph should persist");

    fs::write(root.join("b.md"), "replacement graph source")
        .expect("replacement fixture should write");
    let replacement =
        index_workspace(&root, IndexOptions::default()).expect("replacement index should build");

    let error = index_graph_chunks_cancellable(&root, &replacement.chunks, || true)
        .expect_err("graph indexing should cancel");

    assert_eq!(error, MODEL_REQUEST_CANCELLED);
    assert_eq!(
        fs::read(&graph_path).expect("previous graph should remain available"),
        before
    );
    assert!(
        fs::read_dir(graph_path.parent().expect("graph parent should exist"))
            .expect("graph directory should list")
            .all(|entry| !entry
                .expect("graph entry should load")
                .file_name()
                .to_string_lossy()
                .contains("graph-index"))
    );
}

#[test]
fn retrieval_fusion_deduplicates_and_preserves_channel_reasons() {
    let root = temp_test_root("phase7-fusion");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "fusion source alpha").expect("fixture should write");
    fs::write(root.join("b.md"), "fusion source beta").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let first = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.9,
    };
    let second = RagSearchResult {
        chunk: index.chunks[1].clone(),
        score: 0.8,
    };
    let mut overlapping = first.clone();
    overlapping.chunk.id = "direct-file-evidence".to_string();
    overlapping.score = 1.2;
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 2,
            results: vec![first.clone(), second],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![overlapping],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, "", 8);

    assert_eq!(results.len(), 2);
    assert_eq!(sources.len(), 2);
    assert!(sources[0].reason.contains("semantic_rag"));
    assert!(sources[0].reason.contains("file_search"));
}

#[test]
fn retrieval_fusion_prefers_independent_consensus_over_one_channel_outlier() {
    let root = temp_test_root("phase7-calibrated-consensus");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "single channel outlier").expect("fixture should write");
    fs::write(root.join("b.md"), "independently corroborated evidence")
        .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let outlier = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "a.md")
        .expect("outlier chunk should exist")
        .clone();
    let corroborated = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .expect("corroborated chunk should exist")
        .clone();
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![
                RagSearchResult {
                    chunk: outlier,
                    score: 100.0,
                },
                RagSearchResult {
                    chunk: corroborated.clone(),
                    score: 0.4,
                },
            ],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: corroborated,
                score: 0.5,
            }],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, "", 4);

    assert_eq!(results[0].chunk.path, "b.md");
    assert!(sources[0].reason.contains("consensus:2"));
    assert!(results.iter().all(|result| result.score.is_finite()));
}

#[test]
fn retrieval_fusion_does_not_double_count_correlated_graph_routes() {
    let root = temp_test_root("phase7-calibrated-graph-family");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "graph-only evidence").expect("fixture should write");
    fs::write(root.join("b.md"), "semantic evidence").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let graph = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "a.md")
        .expect("graph chunk should exist")
        .clone();
    let semantic = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .expect("semantic chunk should exist")
        .clone();
    let channels = vec![
        RetrievalChannelOutcome {
            name: "graph_recall".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: graph.clone(),
                score: 1.0,
            }],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: graph,
                score: 1.0,
            }],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: semantic,
                score: 1.0,
            }],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, "", 4);

    assert_eq!(results[0].chunk.path, "b.md");
    let graph_source = sources
        .iter()
        .find(|source| source.path == "a.md")
        .expect("graph source should remain available");
    assert!(!graph_source.reason.contains("consensus:"));
}

#[test]
fn phase8_state_lists_browser_observations() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase8_task_id(),
        EventKind::ToolCallFinished,
        "Browser text extracted",
        [
            ("tool_call_id".to_string(), "browser-1".to_string()),
            ("tool".to_string(), "browser.extract_text".to_string()),
            ("status".to_string(), "succeeded".to_string()),
            ("output".to_string(), "Example Domain".to_string()),
            ("result_url".to_string(), "https://example.com".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase8_state(&store, None).expect("state should load");

    assert_eq!(state.observations.len(), 1);
    assert_eq!(state.observations[0].tool_name, "browser.extract_text");
    assert_eq!(
        state.observations[0].url.as_deref(),
        Some("https://example.com")
    );
}

#[test]
fn phase8_state_lists_pending_browser_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("browser-2".to_string()),
        task_id: phase8_task_id(),
        tool_name: "browser.capture".to_string(),
        input_json: encode_input(&[("url", "https://example.com")]),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("browser.capture")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("browser capture should request permission");
    request.id = PermissionRequestId("perm-phase8".to_string());
    request
        .metadata
        .insert("phase".to_string(), "8".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    store
        .save_permission_request(request, 456)
        .expect("request should save");

    let state = phase8_state(&store, None).expect("state should load");

    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "browser.capture");
}

#[test]
fn agent_state_lists_pending_agent_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "write a file".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("agent-tool-1".to_string()),
        task_id: phase16_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", ".cindx/agent-loop.txt"), ("content", "ok")]),
        proposed_by_model: "agent-loop".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("file.write")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("write should request permission");
    request.id = PermissionRequestId("perm-agent".to_string());
    request
        .metadata
        .insert("phase".to_string(), "16".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    request
        .metadata
        .insert("agent_prompt".to_string(), "write a file".to_string());
    store
        .save_permission_request(request, current_time_millis())
        .expect("request should save");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "waiting_for_permission");
    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "file.write");

    store
        .resolve_permission(PermissionResolution {
            request_id: PermissionRequestId("perm-agent".to_string()),
            decision: PermissionDecision::AllowOnce,
            resolved_at_ms: current_time_millis(),
            resolved_by: "local-user".to_string(),
        })
        .expect("permission should resolve");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task resumed after permission",
        Metadata::new(),
    )
    .expect("resume should append");

    let resumed = agent_state(&store, None).expect("resumed state should load");
    assert_eq!(resumed.status, "running");
    assert!(resumed.pending_approvals.is_empty());
}

#[test]
fn agent_transcript_restores_assistant_tool_and_tool_messages() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "read README".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "read README",
    )
    .expect("user message should append");
    append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "",
            [
                (
                    "raw_tool_calls_json".to_string(),
                    r#"[{"id":"call-1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]"#.to_string(),
                ),
                ("tool_call_count".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("assistant tool call should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Tool,
        "tool=file.read\nstatus=succeeded\noutput=hello",
        [
            ("kind".to_string(), "tool_observation".to_string()),
            ("tool_call_id".to_string(), "call-1".to_string()),
            ("tool".to_string(), "file.read".to_string()),
            ("status".to_string(), "succeeded".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("tool message should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let transcript = agent_transcript_from_active_events(&active_agent_events(&events));

    assert_eq!(transcript.len(), 3);
    assert!(matches!(transcript[1].role, MessageRole::Assistant));
    assert!(transcript[1].metadata.contains_key("raw_tool_calls_json"));
    assert!(matches!(transcript[2].role, MessageRole::Tool));
    assert_eq!(
        transcript[2]
            .metadata
            .get("tool_call_id")
            .map(String::as_str),
        Some("call-1")
    );

    let state = agent_state(&store, None).expect("agent state should load");
    assert!(state.run_started_at_ms > 0);
    assert_eq!(state.messages.len(), 3);
    assert_eq!(state.messages[0].role, "user");
    assert_eq!(state.messages[2].role, "tool");
}

#[test]
fn agent_state_and_trace_are_isolated_by_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, prompt, answer) in [
        ("session-a", "inspect alpha", "alpha answer"),
        ("session-b", "inspect beta", "beta answer"),
        ("session-a", "follow up alpha", "second alpha answer"),
    ] {
        let context = [
            ("project_id".to_string(), "project-cindx".to_string()),
            ("project_name".to_string(), "Cindx".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("session_name".to_string(), session_id.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut start_metadata = context.clone();
        start_metadata.insert("prompt".to_string(), prompt.to_string());
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            start_metadata,
        )
        .expect("start should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            prompt,
            context.clone(),
        )
        .expect("user message should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            answer,
            context,
        )
        .expect("assistant message should append");
    }

    let alpha =
        agent_state_for_session(&store, None, Some("session-a")).expect("alpha state should load");
    let beta =
        agent_state_for_session(&store, None, Some("session-b")).expect("beta state should load");
    let alpha_trace = agent_trace_state_for_session(&store, None, None, Some("session-a"))
        .expect("alpha trace should load");

    assert_eq!(alpha.session_id.as_deref(), Some("session-a"));
    assert_eq!(alpha.messages.len(), 4);
    assert_eq!(alpha.messages[1].content, "alpha answer");
    assert_eq!(alpha.messages[3].content, "second alpha answer");
    assert_eq!(beta.session_id.as_deref(), Some("session-b"));
    assert_eq!(beta.messages.len(), 2);
    assert_eq!(beta.messages[1].content, "beta answer");
    assert_eq!(alpha_trace.session_id.as_deref(), Some("session-a"));
    assert!(alpha_trace
        .turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .all(|step| !step.detail.contains("beta")));
}

#[test]
fn interleaved_agent_runs_remain_isolated_by_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let alpha = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let beta = [("session_id".to_string(), "session-b".to_string())]
        .into_iter()
        .collect::<Metadata>();

    for (summary, context) in [
        ("Agent task started", alpha.clone()),
        ("Agent task started", beta.clone()),
    ] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            summary,
            context,
        )
        .expect("run start should append");
    }
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "alpha finished after beta started",
        alpha.clone(),
    )
    .expect("alpha answer should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "beta answer",
        beta.clone(),
    )
    .expect("beta answer should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        alpha,
    )
    .expect("alpha completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        beta,
    )
    .expect("beta completion should append");

    let alpha_state =
        agent_state_for_session(&store, None, Some("session-a")).expect("alpha state should load");
    let beta_state =
        agent_state_for_session(&store, None, Some("session-b")).expect("beta state should load");

    assert_eq!(alpha_state.status, "completed");
    assert_eq!(alpha_state.messages.len(), 1);
    assert_eq!(
        alpha_state.messages[0].content,
        "alpha finished after beta started"
    );
    assert_eq!(beta_state.status, "completed");
    assert_eq!(beta_state.messages.len(), 1);
    assert_eq!(beta_state.messages[0].content, "beta answer");
}

#[test]
fn cancelling_one_session_does_not_cancel_another() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for session_id in ["session-a", "session-b"] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("session_id".to_string(), session_id.to_string())]
                .into_iter()
                .collect(),
        )
        .expect("run start should append");
    }
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("cancellation should append");

    assert!(agent_task_is_cancelled(&mut store, Some("session-a"))
        .expect("alpha cancellation should load"));
    assert!(!agent_task_is_cancelled(&mut store, Some("session-b"))
        .expect("beta cancellation should load"));
}

#[test]
fn startup_recovery_preserves_unfinished_agent_runs_as_continuations() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, run_id) in [("session-a", "run-a"), ("session-b", "run-b")] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [
                ("session_id".to_string(), session_id.to_string()),
                ("agent_run_id".to_string(), run_id.to_string()),
                ("prompt".to_string(), "finish the task".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("run start should append");
    }
    let recovery_control = AgentRunControl::new("fast");
    let _attempt = recovery_control
        .begin_physical_model_attempt("model-a", 12, 8, RunStageClass::Worker)
        .expect("resource attempt should reserve");
    persist_agent_resource_snapshot(
        &mut store,
        &[
            ("session_id".to_string(), "session-b".to_string()),
            ("agent_run_id".to_string(), "run-b".to_string()),
        ]
        .into_iter()
        .collect(),
        &recovery_control.resource_usage(),
    )
    .expect("resource checkpoint should persist");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("completion should append");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let completed = agent_state_for_session(&store, None, Some("session-a"))
        .expect("completed state should load");
    let interrupted = agent_state_for_session(&store, None, Some("session-b"))
        .expect("interrupted state should load");

    assert_eq!(completed.status, "completed");
    assert_eq!(interrupted.status, "paused");
    assert!(interrupted.can_retry);
    assert!(interrupted.can_continue);
    assert!(!interrupted.can_cancel);
    assert!(interrupted.last_error.is_none());
    let interrupted_events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-b")
        .expect("interrupted events should load");
    let envelope = latest_agent_recovery_envelope(&interrupted_events)
        .expect("recovery envelope should persist");
    assert_eq!(envelope.schema, AGENT_RECOVERY_SCHEMA);
    assert_eq!(envelope.state, AgentRecoveryState::Paused);
    assert_eq!(envelope.reason, AgentRecoveryReason::AppRestarted);
    assert_eq!(envelope.identity.source_run_id, "run-b");
    let resources = envelope
        .resource_snapshot
        .expect("resource checkpoint should transfer to recovery envelope");
    assert_eq!(resources.segment.physical_attempts, 1);
    assert_eq!(resources.segment.reserved_tokens, 20);
    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should be idempotent"),
        0
    );
}

#[test]
fn goal2_startup_recovery_preserves_the_latest_durable_steer_epoch() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let base = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-steered".to_string()),
        ("agent_run_id".to_string(), "run-steered".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [
                ("prompt".to_string(), "Keep every safety check".to_string()),
                (
                    "initial_prompt_objective".to_string(),
                    "Keep every safety check".to_string(),
                ),
                ("steer_epoch".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Keep every safety check",
        metadata_with_context(
            [("steer_epoch".to_string(), "0".to_string())]
                .into_iter()
                .collect(),
            &base,
        ),
    )
    .expect("initial prompt should persist");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Continue after fixing the race",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "1".to_string()),
                ("queue_mode".to_string(), "steer".to_string()),
                ("queue_id".to_string(), "queue-steer-1".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("steer should persist");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-steered")
        .expect("events should load");
    let pause = events
        .iter()
        .rev()
        .find(|event| event.summary == "Agent task paused")
        .expect("restart should persist a pause");
    assert_eq!(
        pause.metadata.get("steer_epoch").map(String::as_str),
        Some("1")
    );
    assert_eq!(latest_applied_agent_steer_epoch(&events), 1);
    assert_eq!(
        pause
            .metadata
            .get("initial_prompt_objective")
            .map(String::as_str),
        Some("Keep every safety check")
    );

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task retry started",
        metadata_with_context(
            [
                (
                    "prompt".to_string(),
                    "Continue after fixing the race".to_string(),
                ),
                ("steer_epoch".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("retry should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Continue after fixing the race",
        metadata_with_context(
            [
                ("steer_epoch".to_string(), "1".to_string()),
                ("continuation_replay".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &base,
        ),
    )
    .expect("retry continuation should persist");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("second recovery should succeed"),
        1
    );
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-steered")
        .expect("recovered retry events should load");
    let active_events = active_agent_events_for_session(&events, Some("session-steered"));
    assert_eq!(latest_applied_agent_steer_epoch(&active_events), 1);
    let second_pause = active_events
        .iter()
        .rev()
        .find(|event| event.summary == "Agent task paused")
        .expect("second restart should persist a pause");
    assert_eq!(
        second_pause.metadata.get("steer_epoch").map(String::as_str),
        Some("1")
    );
}

#[test]
fn startup_recovery_preserves_pending_permission_as_blocked() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "pro".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut start = context.clone();
    start.insert("prompt".to_string(), "write the report".to_string());
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        start,
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "write the report",
        context.clone(),
    )
    .expect("user message should append");

    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("call-write".to_string()),
        task_id: phase16_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", "report.md"), ("content", "draft")]),
        proposed_by_model: "agent-loop".to_string(),
        metadata: context.clone(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("file.write")
        .expect("write tool should exist")
        .permission_request(&invocation)
        .expect("write should require permission");
    request.id = PermissionRequestId("permission-write".to_string());
    request.task_id = phase16_task_id();
    request.metadata.extend(context.clone());
    request
        .metadata
        .insert("tool_call_id".to_string(), "call-write".to_string());
    request
        .metadata
        .insert("tool_name".to_string(), "file.write".to_string());
    store
        .save_permission_request(request, current_time_millis())
        .expect("permission should persist");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("blocked state should load");
    assert_eq!(state.status, "waiting_for_permission");
    assert_eq!(state.pending_approvals.len(), 1);
    assert!(!state.can_continue);
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should load");
    let envelope =
        latest_agent_recovery_envelope(&events).expect("blocked recovery envelope should persist");
    assert_eq!(envelope.state, AgentRecoveryState::Blocked);
    assert_eq!(envelope.effort, "pro");
    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should be idempotent"),
        0
    );
    let claimed = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Blocked],
        AgentRecoveryReason::PermissionResolved,
    )
    .expect("blocked recovery claim should succeed")
    .expect("blocked checkpoint should exist");
    assert_eq!(claimed.attempts, 1);
    let duplicate = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Blocked],
        AgentRecoveryReason::PermissionResolved,
    )
    .expect_err("a blocked recovery must not be claimed twice");
    assert!(duplicate.contains("already claimed"));
}

#[test]
fn recovery_envelope_is_bound_to_the_latest_external_user_turn() {
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "auto".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut events = vec![
        Event {
            id: EventId("start".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata_with_context(
                [("prompt".to_string(), "finish alpha".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("user-alpha".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: metadata_with_context(
                [
                    ("role".to_string(), "user".to_string()),
                    ("content".to_string(), "finish alpha".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    ];
    let envelope = build_agent_recovery_envelope_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        30,
        None,
        None,
    )
    .expect("envelope should build");
    assert!(recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));

    events.push(Event {
        id: EventId("replay".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 30,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "finish alpha".to_string()),
                ("continuation_replay".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    });
    assert!(recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));

    events.push(Event {
        id: EventId("user-beta".to_string()),
        task_id: phase16_task_id(),
        sequence: 4,
        timestamp_ms: 40,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "start beta".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    });
    assert!(!recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));
}

#[test]
fn recovery_envelope_round_trips_the_kernel_task_checkpoint() {
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = vec![
        Event {
            id: EventId("start".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata_with_context(
                [("prompt".to_string(), "finish alpha".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("user-alpha".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: metadata_with_context(
                [
                    ("role".to_string(), "user".to_string()),
                    ("content".to_string(), "finish alpha".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    ];
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "finish alpha",
        AgentRuntimeConfig::default(),
    );
    runtime.turn = 3;
    record_tool_outcome_with_risk(
        &mut runtime,
        "file.write",
        r#"{"path":"report.md"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::WritesWorkspace),
    );
    let checkpoint = AgentTaskStateSnapshot::capture(&runtime);
    let envelope = build_agent_recovery_envelope_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        30,
        Some(&checkpoint),
        None,
    )
    .expect("envelope should build");
    let encoded = serde_json::to_string(&envelope).expect("envelope encodes");
    let encoded_value =
        serde_json::from_str::<serde_json::Value>(&encoded).expect("envelope JSON decodes");
    assert_eq!(encoded_value["resumeKey"], envelope.identity.resume_key);
    assert_eq!(encoded_value["sessionId"], "session-a");
    assert_eq!(encoded_value["state"], "paused");
    assert_eq!(encoded_value["reason"], "deadline_exceeded");
    assert!(encoded_value.get("identity").is_none());
    let decoded =
        serde_json::from_str::<AgentRecoveryEnvelope>(&encoded).expect("envelope decodes");
    let restored = decoded
        .task_state
        .expect("task checkpoint persists")
        .restore("finish alpha", runtime.messages.clone())
        .expect("checkpoint restores");

    assert_eq!(restored.turn, 3);
    assert_eq!(restored.successful_mutations(), 1);

    let mut invalid = encoded_value;
    invalid["taskState"]["schema"] = "cindx.agent.task-state.v999".into();
    assert!(serde_json::from_value::<AgentRecoveryEnvelope>(invalid).is_err());
}

#[test]
fn recovery_claim_is_single_use_and_recovered_if_restart_interrupts_claim() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "pro".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "finish alpha".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "finish alpha",
        context.clone(),
    )
    .expect("user message should append");
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should load");
    let recovery_metadata = agent_recovery_metadata_with_task_state(
        &events,
        &context,
        AgentRecoveryState::Paused,
        AgentRecoveryReason::DeadlineExceeded,
        [("completion".to_string(), "partial".to_string())]
            .into_iter()
            .collect(),
        None,
        None,
    )
    .expect("recovery metadata should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        recovery_metadata,
    )
    .expect("pause should persist");

    let claimed = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Paused],
        AgentRecoveryReason::UserContinued,
    )
    .expect("claim should succeed")
    .expect("checkpoint should exist");
    assert_eq!(claimed.attempts, 1);
    let duplicate = claim_agent_recovery_envelope(
        &mut store,
        &context,
        &[AgentRecoveryState::Paused],
        AgentRecoveryReason::UserContinued,
    )
    .expect_err("a claimed recovery must not be claimed twice");
    assert!(duplicate.contains("already claimed"));

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store)
            .expect("a restart should pause an interrupted claim"),
        1
    );
    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("recovered state should load");
    assert_eq!(state.status, "paused");
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should reload");
    let recovered =
        latest_agent_recovery_envelope(&events).expect("recovered checkpoint should persist");
    assert_eq!(recovered.state, AgentRecoveryState::Paused);
    assert_eq!(recovered.attempts, 1);
}

#[test]
fn recovery_transcript_marks_unfinished_tool_calls_unknown() {
    let mut events = vec![
        Event {
            id: EventId("user".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "update report".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("assistant-tool".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "assistant message".to_string(),
            metadata: [
                ("role".to_string(), "assistant".to_string()),
                ("content".to_string(), String::new()),
                ("tool_call_ids".to_string(), "call-write".to_string()),
                ("raw_tool_calls_json".to_string(), "[]".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ];
    let interrupted = recovery_safe_transcript(&events);
    assert_eq!(interrupted.len(), 3);
    assert_eq!(interrupted[2].role, MessageRole::Tool);
    assert_eq!(
        interrupted[2].metadata.get("status").map(String::as_str),
        Some("interrupted")
    );
    assert!(interrupted[2].content.contains("outcome as unknown"));

    events.push(Event {
        id: EventId("tool-result".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 30,
        kind: EventKind::MessageAdded,
        summary: "tool message".to_string(),
        metadata: [
            ("role".to_string(), "tool".to_string()),
            ("content".to_string(), "write completed".to_string()),
            ("tool_call_id".to_string(), "call-write".to_string()),
            ("status".to_string(), "succeeded".to_string()),
        ]
        .into_iter()
        .collect(),
    });
    let resolved = recovery_safe_transcript(&events);
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved[2].content, "write completed");
}

#[test]
fn queued_agent_message_ids_accept_only_bounded_client_ids() {
    assert_eq!(
        queued_agent_message_id(Some(" agent-queue-client-abc-123 ")),
        "agent-queue-client-abc-123"
    );
    assert!(!queued_agent_message_id(Some("queue-client-abc")).starts_with("queue-client-"));
    assert!(!queued_agent_message_id(Some("agent-queue-client-bad/id")).contains("bad/id"));
}

#[test]
fn queued_agent_messages_are_durable_ordered_and_session_scoped() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context_a = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let context_b = [("session_id".to_string(), "session-b".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = |prompt: &str| QueuedAgentMessagePayload {
        prompt: prompt.to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    let first = payload("first");
    let second = payload("second");
    let other = payload("other session");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "enqueue",
        "queue-a-1",
        "queue",
        10,
        Some(&first),
    )
    .expect("first message should queue");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "enqueue",
        "queue-a-2",
        "queue",
        20,
        Some(&second),
    )
    .expect("second message should queue");
    append_agent_queue_event(
        &mut store,
        &context_b,
        "enqueue",
        "queue-b-1",
        "queue",
        5,
        Some(&other),
    )
    .expect("other session message should queue");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "steer",
        "queue-a-2",
        "steer",
        20,
        None,
    )
    .expect("second message should steer");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    let pending = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].view.id, "queue-a-2");
    assert_eq!(pending[0].view.mode, "steer");
    assert_eq!(pending[1].view.id, "queue-a-1");

    let state =
        agent_state_for_session(&store, None, Some("session-a")).expect("queued state should load");
    assert_eq!(state.session_id.as_deref(), Some("session-a"));
    assert_eq!(state.status, "idle");
    assert_eq!(state.queued_messages.len(), 2);
    assert!(state.timeline.is_empty());
    let edited = payload("first edited");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "edit",
        "queue-a-1",
        "queue",
        10,
        Some(&edited),
    )
    .expect("first message should edit");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "delete",
        "queue-a-2",
        "steer",
        20,
        None,
    )
    .expect("steered message should delete");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("edited queue events should load");
    let remaining = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].view.prompt, "first edited");
    assert_eq!(
        agent_state_for_session(&store, None, Some("session-b"))
            .expect("other queued state should load")
            .queued_messages
            .len(),
        1
    );
}

#[test]
fn queue_events_do_not_change_a_terminal_agent_status() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "first task".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context.clone(),
    )
    .expect("run should complete");
    let queued = QueuedAgentMessagePayload {
        prompt: "next task".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-next",
        "queue",
        20,
        Some(&queued),
    )
    .expect("next task should queue");

    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("terminal queue state should load");
    assert_eq!(state.status, "completed");
    assert_eq!(state.queued_messages.len(), 1);
    assert_eq!(state.timeline.len(), 2);
}

#[test]
fn queued_messages_preserve_a_permission_waiting_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "inspect files".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task waiting for permission",
        context.clone(),
    )
    .expect("run should wait");
    let queued = QueuedAgentMessagePayload {
        prompt: "follow-up".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-next",
        "queue",
        20,
        Some(&queued),
    )
    .expect("follow-up should queue");

    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("waiting queue state should load");
    assert_eq!(state.status, "waiting_for_permission");
    assert!(state.can_cancel);
    assert_eq!(state.queued_messages.len(), 1);
}

#[test]
fn queued_agent_message_start_and_restore_are_replay_safe() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = QueuedAgentMessagePayload {
        prompt: "continue safely".to_string(),
        attachments: Vec::new(),
        effort: "pro".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    append_agent_queue_event(&mut store, &context, "start", "queue-a", "queue", 10, None)
        .expect("message should start");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    assert!(pending_queued_agent_messages(&events, "session-a").is_empty());

    append_agent_queue_event(
        &mut store,
        &context,
        "restore",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should restore");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("restored queue events should load");
    let restored = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].view.prompt, "continue safely");
    assert_eq!(restored[0].view.effort, "pro");
}

#[test]
fn queued_run_failure_restores_only_before_agent_start() {
    let session_id = "session-queue-failure";
    let context = [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = QueuedAgentMessagePayload {
        prompt: "continue safely".to_string(),
        attachments: Vec::new(),
        effort: "pro".to_string(),
        current_time: "now".to_string(),
    };
    let pending = PendingQueuedAgentMessage {
        view: QueuedAgentMessageView {
            id: "queue-a".to_string(),
            session_id: session_id.to_string(),
            prompt: payload.prompt.clone(),
            attachments: Vec::new(),
            effort: payload.effort.clone(),
            mode: "queue".to_string(),
            created_at_ms: 10,
            updated_at_ms: 10,
        },
        payload: payload.clone(),
        priority_sequence: 1,
    };

    let mut pre_start_store = SqliteStore::in_memory().expect("store should open");
    append_agent_queue_event(
        &mut pre_start_store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    append_agent_queue_event(
        &mut pre_start_store,
        &context,
        "start",
        "queue-a",
        "queue",
        10,
        None,
    )
    .expect("dispatch should reserve the message");
    assert!(restore_queued_agent_message_before_run_start(
        &mut pre_start_store,
        &context,
        session_id,
        &pending,
    )
    .expect("a pre-start failure should restore"));
    let pre_start_events = pre_start_store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    assert_eq!(
        pending_queued_agent_messages(&pre_start_events, session_id).len(),
        1
    );

    let mut post_start_store = SqliteStore::in_memory().expect("store should open");
    append_agent_queue_event(
        &mut post_start_store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    append_agent_queue_event(
        &mut post_start_store,
        &context,
        "start",
        "queue-a",
        "queue",
        10,
        None,
    )
    .expect("dispatch should reserve the message");
    let run_context = [
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("queue_id".to_string(), "queue-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut post_start_store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("agent start should persist");
    append_message_event_with_metadata(
        &mut post_start_store,
        &phase16_task_id(),
        MessageRole::User,
        &payload.prompt,
        run_context,
    )
    .expect("user message should persist");

    assert!(!restore_queued_agent_message_before_run_start(
        &mut post_start_store,
        &context,
        session_id,
        &pending,
    )
    .expect("a post-start failure should not restore"));
    let post_start_events = post_start_store
        .list_by_task(&phase16_task_id())
        .expect("run events should load");
    assert!(pending_queued_agent_messages(&post_start_events, session_id).is_empty());
    assert!(!post_start_events.iter().any(|event| {
        event.metadata.get("queue_action").map(String::as_str) == Some("restore")
    }));
}

#[test]
fn queued_steer_commit_revalidates_the_current_queue_item() {
    let session_id = "session-steer-commit";
    let context = [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = QueuedAgentMessagePayload {
        prompt: "apply this guidance".to_string(),
        attachments: Vec::new(),
        effort: "pro".to_string(),
        current_time: "now".to_string(),
    };
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-live",
        "queue",
        10,
        Some(&payload),
    )
    .expect("live message should queue");

    let committed = commit_queued_agent_steer(&mut store, &context, session_id, "queue-live")
        .expect("a current queue item should commit");
    assert_eq!(committed.mode, "steer");

    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-deleted",
        "queue",
        20,
        Some(&payload),
    )
    .expect("second message should queue");
    append_agent_queue_event(
        &mut store,
        &context,
        "delete",
        "queue-deleted",
        "queue",
        20,
        None,
    )
    .expect("second message should delete");

    assert_eq!(
        commit_queued_agent_steer(&mut store, &context, session_id, "queue-deleted")
            .expect_err("a stale queue item must not commit"),
        "queued message not found"
    );
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.metadata.get("queue_action").map(String::as_str) == Some("steer")
            })
            .count(),
        1
    );
}

#[test]
fn queued_agent_messages_update_the_incremental_session_read_model() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-queue-read-model";
    let context = [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect::<Metadata>();
    let mut payload = QueuedAgentMessagePayload {
        prompt: "first version".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    let initial = load_agent_session_read_model(&mut store, session_id)
        .expect("initial queue read model should build");
    assert_eq!(initial.state.queued_messages.len(), 1);
    assert_eq!(
        initial
            .queued_payloads
            .get("queue-a")
            .map(|payload| payload.prompt.as_str()),
        Some("first version")
    );

    payload.prompt = "edited version".to_string();
    append_agent_queue_event(
        &mut store,
        &context,
        "edit",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should edit");
    let edited = load_agent_session_read_model(&mut store, session_id)
        .expect("queue edit should apply incrementally");
    assert_eq!(edited.state.queued_messages[0].prompt, "edited version");
    let next = next_queued_agent_message_from_read_model(&mut store, session_id)
        .expect("next queue item should use the read model")
        .expect("next queue item should exist");
    assert_eq!(next.payload.prompt, "edited version");
    let (queued, can_cancel) =
        queued_agent_message_from_read_model(&mut store, session_id, "queue-a")
            .expect("queue action lookup should use the read model");
    let receipt = queued_agent_message_action_receipt(
        &store, session_id, "queue-a", queued, can_cancel, false,
    )
    .expect("queue action receipt should use the compact revision");
    assert_eq!(
        receipt
            .message
            .as_ref()
            .map(|message| message.prompt.as_str()),
        Some("edited version")
    );
    assert!(!receipt.cancelled_active_run);
    assert!(!receipt.steer_committed);
    assert_eq!(
        serde_json::to_value(&receipt)
            .expect("receipt should serialize")
            .get("steerCommitted")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );

    append_agent_queue_event(&mut store, &context, "start", "queue-a", "queue", 10, None)
        .expect("message should start");
    let started = load_agent_session_read_model(&mut store, session_id)
        .expect("queue start should apply incrementally");
    assert!(started.state.queued_messages.is_empty());
    assert!(started.queued_payloads.is_empty());

    let run_context = [
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("queue_id".to_string(), "queue-a".to_string()),
    ]
    .into_iter()
    .collect();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context,
    )
    .expect("queued run should start");
    let running = load_agent_session_read_model(&mut store, session_id)
        .expect("run identity should apply incrementally");
    assert_eq!(running.latest_run_queue_id.as_deref(), Some("queue-a"));
}

#[test]
fn partial_budget_completion_exposes_a_continuation() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("prompt".to_string(), "finish the long task".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        context.clone(),
    )
    .expect("run start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [
                ("completion".to_string(), "partial".to_string()),
                ("continuation_available".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("partial completion should append");

    let state =
        agent_state_for_session(&store, None, Some("session-a")).expect("agent state should load");
    assert_eq!(state.status, "completed");
    assert!(state.can_retry);
    assert!(state.can_continue);
}

#[test]
fn session_permission_grant_only_covers_the_same_capability() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let granted = PermissionRequest {
        id: PermissionRequestId("session-grant".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "run a command".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
            ("command".to_string(), "cargo test".to_string()),
            ("session_reusable".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    store
        .save_permission_request(granted.clone(), 1)
        .expect("grant request should save");
    store
        .resolve_permission(PermissionResolution {
            request_id: granted.id.clone(),
            decision: PermissionDecision::AllowForSession,
            resolved_at_ms: 2,
            resolved_by: "local-user".to_string(),
        })
        .expect("grant should resolve");

    let mut next = granted.clone();
    next.id = PermissionRequestId("next-request".to_string());
    assert!(
        agent_session_permission_granted(&store, &phase16_task_id(), &next, Some("session-a"),)
            .expect("matching grant should load")
    );
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-b"),
    )
    .expect("other session should load"));
    next.action = "file.write".to_string();
    next.scope = "crates/tools".to_string();
    next.risk = PermissionRisk::Write;
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("execute grant must not cover a write request"));
    next.action = "shell.run".to_string();
    next.risk = PermissionRisk::Execute;
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("shell grants must remain bound to their working directory"));
    next.scope = ".".to_string();
    assert!(
        agent_session_permission_granted(&store, &phase16_task_id(), &next, Some("session-a"),)
            .expect("the exact shell capability should reuse the session grant")
    );
    next.metadata
        .insert("command".to_string(), "cargo build".to_string());
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("another shell command should not reuse the grant"));
    next.metadata.remove("command");
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("legacy shell grants without a command should fail closed"));
    next.metadata
        .insert("command".to_string(), "cargo test".to_string());
    next.risk = PermissionRisk::Destructive;
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("destructive grant should not persist"));
}

#[test]
fn pending_permissions_are_isolated_by_agent_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (id, run_id) in [("pending-old", "run-old"), ("pending-new", "run-new")] {
        store
            .save_permission_request(
                PermissionRequest {
                    id: PermissionRequestId(id.to_string()),
                    task_id: phase16_task_id(),
                    risk: PermissionRisk::Execute,
                    action: "shell.run".to_string(),
                    reason: "run a command".to_string(),
                    scope: ".".to_string(),
                    metadata: [
                        ("session_id".to_string(), "session-a".to_string()),
                        ("agent_run_id".to_string(), run_id.to_string()),
                    ]
                    .into_iter()
                    .collect(),
                },
                if run_id == "run-old" { 1 } else { 2 },
            )
            .expect("pending request should save");
    }

    let pending = pending_agent_permissions_for_run(
        &store,
        &phase16_task_id(),
        Some("session-a"),
        Some("run-new"),
    )
    .expect("pending requests should load");

    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id.0, "pending-new");
}

#[test]
fn agent_state_ignores_old_pending_approvals_after_new_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let mut request = PermissionRequest {
        id: PermissionRequestId("old-agent-perm".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Write,
        action: "file.write".to_string(),
        reason: "old run".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("tool_call_id".to_string(), "old-call".to_string()),
            ("tool_name".to_string(), "file.write".to_string()),
            ("tool_input".to_string(), "path=old.txt".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    request
        .metadata
        .insert("phase".to_string(), "16".to_string());
    store
        .save_permission_request(request, 1)
        .expect("old request should save");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "new task".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "new task",
    )
    .expect("user message should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "running");
    assert!(state.pending_approvals.is_empty());
    assert_eq!(state.transcript_messages, 1);
}

#[test]
fn agent_state_reports_cancelled_and_retryable() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "inspect".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        Metadata::new(),
    )
    .expect("cancel should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "cancelled");
    assert!(state.can_retry);
    assert!(!state.can_cancel);
}

#[test]
fn agent_trace_groups_steps_by_turn_and_exposes_details() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "read README".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "read README",
    )
    .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestStarted,
        "Agent model turn started",
        [
            ("request_id".to_string(), "agent-model-1".to_string()),
            ("turn".to_string(), "0".to_string()),
            ("model".to_string(), "model-a".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("model start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        [
            ("request_id".to_string(), "agent-model-1".to_string()),
            ("latency_ms".to_string(), "42".to_string()),
            ("tool_calls".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("model finish should append");
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-1",
        "file.write",
        "succeeded",
        "written ok",
        [("path".to_string(), "notes/result.md".to_string())]
            .into_iter()
            .collect(),
        None,
    )
    .expect("tool finish should append");

    let trace =
        agent_trace_state_for_session(&store, None, None, None).expect("trace should build");

    assert_eq!(trace.turn_count, 1);
    assert!(trace.step_count >= 5);
    assert!(trace.tool_call_count >= 1);
    assert!(trace.turns.iter().any(|turn| turn.index == 1
        && turn
            .steps
            .iter()
            .any(|step| { step.kind == "model" && step.latency_ms == Some(42) })));
    assert!(trace
        .turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .any(|step| step.tool_name.as_deref() == Some("file.write")
            && step.output_preview.as_deref() == Some("written ok")
            && step.artifact_path.as_deref() == Some("notes/result.md")));
}

#[test]
fn agent_trace_reports_actual_collaboration_role_activity() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-role-trace".to_string()),
        ("project_id".to_string(), "project-role-trace".to_string()),
        ("agent_run_id".to_string(), "run-role-trace".to_string()),
        (
            "collaboration_id".to_string(),
            "collab-role-trace".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "compare approaches".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("start should append");
    for (role, model, status, latency, first_token, tokens, evidence) in [
        (
            "planner",
            "model-planner",
            "completed",
            "120",
            "30",
            "80",
            "2",
        ),
        (
            "reviewer",
            "model-reviewer",
            "degraded",
            "90",
            "25",
            "40",
            "1",
        ),
        (
            "planner",
            "model-planner",
            "interrupted",
            "31",
            "",
            "0",
            "0",
        ),
    ] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            format!("Collaboration {role} finished"),
            metadata_with_context(
                [
                    ("role".to_string(), role.to_string()),
                    ("model".to_string(), model.to_string()),
                    ("status".to_string(), status.to_string()),
                    ("latency_ms".to_string(), latency.to_string()),
                    (
                        "first_token_latency_ms".to_string(),
                        first_token.to_string(),
                    ),
                    ("total_tokens".to_string(), tokens.to_string()),
                    ("evidence_count".to_string(), evidence.to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        )
        .expect("role event should append");
    }
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("completion should append");

    let trace = agent_trace_state_for_session(&store, None, None, Some("session-role-trace"))
        .expect("trace should build");

    assert_eq!(trace.role_summaries.len(), 2);
    assert_eq!(trace.role_summaries[0].role, "planner");
    assert_eq!(trace.role_summaries[0].models, vec!["model-planner"]);
    assert_eq!(trace.role_summaries[0].calls, 2);
    assert_eq!(trace.role_summaries[0].completed, 1);
    assert_eq!(trace.role_summaries[0].interrupted, 1);
    assert_eq!(trace.role_summaries[0].degraded, 0);
    assert_eq!(trace.role_summaries[0].latency_ms, 151);
    assert_eq!(trace.role_summaries[0].first_token_latency_ms, Some(30));
    assert_eq!(trace.role_summaries[1].role, "reviewer");
    assert_eq!(trace.role_summaries[1].degraded, 1);
    assert_eq!(trace.role_summaries[1].evidence_count, 1);
}

#[test]
fn agent_outputs_accumulate_versioned_files_across_session_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (index, run_id) in ["run-one", "run-two"].into_iter().enumerate() {
        let context = [
            ("session_id".to_string(), "session-alpha".to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect();
        append_tool_finished_event(
            &mut store,
            &phase16_task_id(),
            &format!("call-{index}"),
            "file.write",
            "succeeded",
            "file written",
            [
                ("path".to_string(), "notes/result.md".to_string()),
                (
                    "source_path".to_string(),
                    "/workspace/notes/result.md".to_string(),
                ),
                (
                    "artifact_path".to_string(),
                    format!("/workspace/.cindx/output-history/{run_id}/result.md"),
                ),
            ]
            .into_iter()
            .collect(),
            Some(&context),
        )
        .expect("tool output should append");
    }
    let context = [
        ("session_id".to_string(), "session-alpha".to_string()),
        ("agent_run_id".to_string(), "run-two".to_string()),
    ]
    .into_iter()
    .collect();
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-list",
        "file.list",
        "succeeded",
        "notes",
        [("path".to_string(), "notes".to_string())]
            .into_iter()
            .collect(),
        Some(&context),
    )
    .expect("read-only tool output should append");

    let events = agent_events_for_session(&store, &phase16_task_id(), Some("session-alpha"))
        .expect("session events should load");
    let outputs = agent_output_artifacts_from_events(&events);
    let manifest = artifact_manifest_message(&events).expect("manifest should exist");

    assert_eq!(outputs.len(), 2);
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.version)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2])
    );
    assert!(outputs
        .iter()
        .all(|output| { output.source_path.as_deref() == Some("/workspace/notes/result.md") }));
    assert!(outputs
        .iter()
        .any(|output| output.run_id.as_deref() == Some("run-one")));
    assert!(outputs
        .iter()
        .any(|output| output.run_id.as_deref() == Some("run-two")));
    assert_eq!(
        manifest.metadata.get("kind").map(String::as_str),
        Some("artifact_manifest")
    );
    assert!(manifest.content.contains("/workspace/notes/result.md"));
    assert!(manifest.content.contains("run-one"));
    assert!(manifest.content.contains("run-two"));
    assert!(manifest.content.contains("latest_version\": 2"));
}

#[test]
fn agent_trace_export_writes_jsonl() {
    let root = temp_test_root("phase18-trace");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "inspect".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");

    let path = write_agent_trace_jsonl(&root, &store, None).expect("trace should export");
    let text = fs::read_to_string(path).expect("trace export should be readable");

    assert!(text.contains("\"trace_id\""));
    assert!(text.contains("\"task_id\":\"phase-16-agent-loop\""));
    assert!(text.contains("\"kind\":\"message\""));
}

#[test]
fn project_session_state_defaults_to_active_workspace() {
    let root = temp_test_root("phase20-projects");
    let config = ProjectSessionConfig::default_for_root(&root);
    let session_id = config.active_session_id.clone();
    let state = project_session_state(&config, None);

    assert_eq!(state.projects.len(), 1);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.projects[0].root, root.display().to_string());
    assert!(state.projects[0].active);
    assert!(state.sessions[0].active);
    assert_eq!(state.sessions[0].effort, "auto");
    assert_eq!(state.active_project_id, "project-cindx");
    assert_eq!(state.active_session_id, session_id);
    assert!(state.active_session_id.starts_with("sess_"));
}

#[test]
fn new_session_ids_are_unique_uuid_v7_values() {
    let first = new_session_id();
    let second = new_session_id();

    assert_ne!(first, second);
    for session_id in [first, second] {
        let value = session_id
            .strip_prefix("sess_")
            .expect("session id should use the opaque prefix");
        let uuid = uuid::Uuid::parse_str(value).expect("session id should contain a UUID");
        assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
    }
}

#[test]
fn schedule_execution_sessions_stay_out_of_the_task_sidebar() {
    let root = temp_test_root("hidden-schedule-session");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.sessions.push(SessionRecord {
        id: "schedule-session-review".to_string(),
        project_id: "project-cindx".to_string(),
        name: "Review · Schedule".to_string(),
        detail: SCHEDULE_EXECUTION_SESSION_DETAIL.to_string(),
        effort: "auto".to_string(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    let state = project_session_state(&config, None);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].id, initial_session_id);

    config.sessions.retain(is_schedule_execution_session);
    let replacement = ensure_open_session_for_project(&mut config, "project-cindx");
    assert_ne!(replacement, "schedule-session-review");
    assert!(config
        .sessions
        .iter()
        .any(|session| { session.id == replacement && !is_schedule_execution_session(session) }));
}

#[test]
fn session_effort_updates_only_the_selected_session() {
    let root = temp_test_root("session-effort");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.sessions.push(SessionRecord {
        id: "session-second".to_string(),
        project_id: config.projects[0].id.clone(),
        name: "Second".to_string(),
        detail: "timeline + chat".to_string(),
        effort: default_agent_effort(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    assert!(update_session_effort(
        &mut config,
        &initial_session_id,
        "pro"
    ));
    assert_eq!(config.sessions[0].effort, "pro");
    assert_eq!(config.sessions[1].effort, "auto");
    assert!(!update_session_effort(&mut config, "missing", "high"));
}

#[test]
fn deleting_a_project_removes_its_sessions_and_selects_a_neighbor() {
    let root = temp_test_root("delete-project");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.projects.push(ProjectRecord {
        id: "project-next".to_string(),
        name: "Next".to_string(),
        root: root.join("next").display().to_string(),
        detail: "workspace project".to_string(),
        created_at_ms: 2,
        updated_at_ms: 2,
    });
    config.sessions.push(SessionRecord {
        id: "session-next".to_string(),
        project_id: "project-next".to_string(),
        name: "Next Session".to_string(),
        detail: "timeline + chat".to_string(),
        effort: "pro".to_string(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    let (_, deleted_session_ids) = remove_project_from_config(&mut config, "project-cindx")
        .expect("project should be removed");

    assert_eq!(deleted_session_ids, vec![initial_session_id]);
    assert_eq!(config.projects.len(), 1);
    assert_eq!(config.sessions.len(), 1);
    assert_eq!(config.active_project_id, "project-next");
    assert_eq!(config.active_session_id, "session-next");
    assert_eq!(config.sessions[0].effort, "pro");
}

#[test]
fn deleting_the_last_project_leaves_an_empty_workspace() {
    let root = temp_test_root("delete-last-project");
    let mut config = ProjectSessionConfig::default_for_root(&root);

    remove_project_from_config(&mut config, "project-cindx").expect("project should be removed");

    assert!(config.projects.is_empty());
    assert!(config.sessions.is_empty());
    assert!(config.active_project_id.is_empty());
    assert!(config.active_session_id.is_empty());
}

#[test]
fn automatic_session_names_use_the_first_prompt() {
    assert!(is_automatic_session_name("Runtime Session"));
    assert!(is_automatic_session_name("New Session"));
    assert!(!is_automatic_session_name("Release planning"));
    assert_eq!(
        automatic_session_title("  Review   the project architecture and risks  "),
        "Review the project architecture and risks"
    );
    assert_eq!(
        automatic_session_title("请帮我修复 session 自动命名"),
        "修复 session 自动命名"
    );
    assert_eq!(
        automatic_session_title("请帮我分析 MBTI，重点区分 N/S？"),
        "分析 MBTI 重点区分 N/S"
    );
    assert_eq!(
        automatic_session_title("用脑图表示一下 transformer 的原理"),
        "Transformer 原理思维导图"
    );
    assert_eq!(
        automatic_session_title("用 mindmap 描述下 transformer 架构"),
        "Transformer 架构思维导图"
    );
}

#[test]
fn semantic_session_titles_reject_raw_conversation_sentences() {
    let turns = vec![
        SessionTitleTurn {
            prompt: "你帮我画一个超时空要塞的三段变形机器人".to_string(),
            answer: "我会生成一张三段变形机器人设定图。".to_string(),
        },
        SessionTitleTurn {
            prompt: "这是高达，不是马克罗士，你重新画".to_string(),
            answer: "我会按超时空要塞 VF-1 的特征重新绘制。".to_string(),
        },
    ];

    assert!(generated_session_title_copies_conversation(
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
    assert_eq!(
        validated_generated_session_title("这是高达 不是马克罗士 你重新画", &turns),
        None
    );
    assert_eq!(
        validated_generated_session_title("重绘超时空要塞变形机器人", &turns),
        Some("重绘超时空要塞变形机器人".to_string())
    );
}

#[test]
fn generated_session_titles_never_copy_even_concise_user_topics() {
    let turns = vec![SessionTitleTurn {
        prompt: "Rust agent loop review".to_string(),
        answer: "I found two lifecycle races.".to_string(),
    }];

    assert!(generated_session_title_copies_conversation(
        "Rust agent loop review",
        &turns
    ));
    assert_eq!(
        validated_generated_session_title("Rust agent loop review", &turns),
        None
    );
    assert_eq!(
        fallback_session_title(&turns),
        Some("Rust agent loop review Overview".to_string())
    );
}

#[test]
fn session_title_fallback_recovers_a_pending_conversation_without_copying_a_request() {
    let title_like_turns = vec![SessionTitleTurn {
        prompt: "用 mindmap 描述下 transformer 架构".to_string(),
        answer: "下面用思维导图展示 Transformer 的结构。".to_string(),
    }];
    assert_eq!(
        fallback_session_title(&title_like_turns),
        Some("Transformer 架构思维导图".to_string())
    );

    let request_turns = vec![SessionTitleTurn {
        prompt: "请帮我修复 session 自动命名".to_string(),
        answer: "我会检查标题生成与持久化链路。".to_string(),
    }];
    assert_eq!(
        fallback_session_title(&request_turns),
        Some("修复 session 自动命名".to_string())
    );
}

#[test]
fn session_title_state_retries_pending_and_repairs_legacy_prompt_copies() {
    let turns = vec![SessionTitleTurn {
        prompt: "这是高达，不是马克罗士，你重新画".to_string(),
        answer: "我会按超时空要塞的设定重新绘制。".to_string(),
    }];

    assert!(session_title_refinement_needed(
        SessionTitleState::Pending,
        "New Session",
        &turns
    ));
    assert!(session_title_refinement_needed(
        SessionTitleState::Automatic,
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
    let copied_diagram_turns = vec![SessionTitleTurn {
        prompt: "用脑图表示一下 transformer 的原理".to_string(),
        answer: "下面用思维导图介绍 Transformer 原理。".to_string(),
    }];
    assert!(session_title_refinement_needed(
        SessionTitleState::Automatic,
        "用脑图表示一下 transformer 的原理",
        &copied_diagram_turns
    ));
    assert!(!session_title_refinement_needed(
        SessionTitleState::Automatic,
        "重绘超时空要塞变形机器人",
        &turns
    ));
    assert!(!session_title_refinement_needed(
        SessionTitleState::Manual,
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
}

#[test]
fn generated_session_titles_are_clean_and_bounded() {
    assert_eq!(
        cleaned_generated_session_title("**标题：桌面宠物开发。**\nextra"),
        Some("桌面宠物开发".to_string())
    );
    assert_eq!(
        cleaned_generated_session_title("Title: Review repository architecture"),
        Some("Review repository architecture".to_string())
    );
    assert_eq!(cleaned_generated_session_title("New Session"), None);
    assert_eq!(cleaned_generated_session_title("你好，Dale！我是"), None);
    assert_eq!(cleaned_generated_session_title("我是 Cindx"), None);
}

#[test]
fn session_title_context_skips_greetings_and_uses_two_meaningful_turns() {
    let message = |sequence, role: &str, content: &str| ChatMessageView {
        sequence,
        role: role.to_string(),
        content: content.to_string(),
        timestamp_ms: sequence,
        run_id: None,
        queue_id: None,
        attachments: Vec::new(),
    };
    let messages = vec![
        message(1, "user", "你好"),
        message(2, "assistant", "你好，Dale！我是 Cindx。"),
        message(3, "user", "分析我们对话里体现出的 MBTI 倾向"),
        message(4, "assistant", "我会根据具体措辞分析倾向。"),
        message(5, "user", "重点区分 N/S，并给出直接证据"),
        message(6, "assistant", "N/S 的证据主要来自抽象与细节偏好。"),
    ];

    let turns = meaningful_session_title_turns(&messages);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].prompt, "分析我们对话里体现出的 MBTI 倾向");
    assert_eq!(turns[1].prompt, "重点区分 N/S，并给出直接证据");
    assert_eq!(turns[1].answer, "N/S 的证据主要来自抽象与细节偏好。");
}

#[test]
fn greeting_only_turns_do_not_claim_a_session_title() {
    for greeting in ["你好", "您好！", "hello", "Hi there"] {
        assert!(!is_meaningful_session_title_prompt(greeting));
    }
    assert!(is_meaningful_session_title_prompt(
        "你好，帮我审查 Rust agent loop"
    ));
    assert!(is_meaningful_session_title_prompt(
        "Hello World app architecture"
    ));
}

#[test]
fn generated_session_titles_do_not_overwrite_later_edits() {
    assert!(can_apply_generated_session_title(
        "Initial request title",
        42,
        "Initial request title",
        42
    ));
    assert!(!can_apply_generated_session_title(
        "My custom title",
        43,
        "Initial request title",
        42
    ));
    assert!(!can_apply_generated_session_title(
        "Initial request title",
        43,
        "Initial request title",
        42
    ));
}

#[test]
fn archived_active_session_gets_a_visible_replacement_and_can_be_listed() {
    let root = temp_test_root("archived-session");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    config.sessions[0].archived_at_ms = Some(42);
    config.ensure_consistent(&root);

    let state = project_session_state(&config, None);
    let archived = state
        .sessions
        .iter()
        .find(|session| session.archived)
        .expect("archived session should remain recoverable");
    let active = state
        .sessions
        .iter()
        .find(|session| session.active)
        .expect("replacement session should be active");

    assert_eq!(archived.archived_at_ms, Some(42));
    assert_ne!(archived.id, active.id);
    assert!(!active.archived);
}

#[test]
fn session_activity_acknowledgement_stops_at_the_last_visible_sequence() {
    assert_eq!(acknowledged_event_sequence(12, None), 12);
    assert_eq!(acknowledged_event_sequence(12, Some(8)), 8);
    assert_eq!(acknowledged_event_sequence(12, Some(20)), 12);
}

#[test]
fn archived_session_restore_does_not_revive_seen_activity() {
    let root = temp_test_root("archived-session-seen-activity");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let session_id = config.sessions[0].id.clone();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let metadata = [("session_id".to_string(), session_id.clone())]
        .into_iter()
        .collect();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata,
    )
    .expect("completion should append");
    let latest_sequence = load_agent_session_read_model_snapshot(&store, &session_id)
        .expect("session model should load")
        .revision;

    config.sessions[0].seen_event_sequence = latest_sequence;
    config.sessions[0].archived_at_ms = Some(42);
    let archived = project_session_state_from_store(&config, &store, None)
        .expect("archived state should project");
    assert_eq!(archived.sessions[0].activity, "idle");
    assert!(!archived.sessions[0].unseen_result);

    config.sessions[0].archived_at_ms = None;
    let restored = project_session_state_from_store(&config, &store, None)
        .expect("restored state should project");
    assert_eq!(restored.sessions[0].activity, "idle");
    assert!(!restored.sessions[0].unseen_result);
}

#[test]
fn fork_names_are_unique_within_a_project() {
    let root = temp_test_root("fork-name");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let source = config.sessions[0].clone();
    assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork");
    config.sessions.push(SessionRecord {
        id: "fork-one".to_string(),
        project_id: source.project_id.clone(),
        name: "Runtime Session Fork".to_string(),
        detail: "fork".to_string(),
        effort: default_agent_effort(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 1,
        updated_at_ms: 1,
        archived_at_ms: None,
    });

    assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork 2");
}

#[test]
fn agent_state_and_trace_expose_project_session_context() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("prompt".to_string(), "inspect".to_string()),
            ("project_id".to_string(), "project-alpha".to_string()),
            ("project_name".to_string(), "Alpha".to_string()),
            ("session_id".to_string(), "session-alpha".to_string()),
            ("session_name".to_string(), "Alpha Session".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");

    let state = agent_state(&store, None).expect("state should load");
    let trace =
        agent_trace_state_for_session(&store, None, None, None).expect("trace should build");

    assert_eq!(state.project_id.as_deref(), Some("project-alpha"));
    assert_eq!(state.session_name.as_deref(), Some("Alpha Session"));
    assert_eq!(trace.project_name.as_deref(), Some("Alpha"));
    assert_eq!(trace.session_id.as_deref(), Some("session-alpha"));
}

#[test]
fn default_sidecar_state_reports_runtime_capabilities() {
    let config = SidecarConfig::default();
    let state = sidecar_state(&config, None);

    assert!(state.auto_configure);
    assert!(state.browser.exists);
    assert!(state.browser.healthy);
    assert!(state.computer.exists);
    assert!(state.computer.executable);
    if !state.computer.healthy {
        assert!(!state.computer.health_output.trim().is_empty());
    }
}

fn temp_test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", unique_id("test")))
}
