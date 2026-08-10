use super::*;
use crate::collaboration_learning_policy::{
    CollaborationRepairV1, CollaborationVerificationV1, COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
    COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
};
use crate::collaboration_learning_projection::test_support::{
    exercise_for_outcome, outcome_for_policy,
};
use crate::{AgentOutcomeTerminalStatusV1, ExternallyVerifiedOutcomeV1};

fn digest(value: char) -> String {
    value.to_string().repeat(64)
}

fn direct_policy() -> CollaborationLearningPolicyV1 {
    CollaborationLearningPolicyV1::seed(
        CollaborationSpecialistInvocationV1::DirectOwnerOnly,
        0,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .unwrap()
}

fn workflow_parent() -> CollaborationLearningPolicyV1 {
    CollaborationLearningPolicyV1::seed(
        CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
        COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .unwrap()
}

fn candidate_policy() -> CollaborationLearningPolicyV1 {
    let parent = workflow_parent();
    CollaborationLearningPolicyV1::candidate(
        &parent,
        CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
        COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .unwrap()
}

fn candidate(
    holdout: &[CollaborationLearningComparisonBindingV1],
    config: &CollaborationLearningConfigV1,
) -> CollaborationLearningCandidateV1 {
    CollaborationLearningCandidateV1::snapshot(
        candidate_policy(),
        Some(workflow_parent()),
        CollaborationLearningCandidateV1::freeze_holdout_manifest(holdout).unwrap(),
        config.digest().to_string(),
        digest('9'),
        1,
    )
    .unwrap()
}

fn comparison(
    split: CollaborationLearningSplitV1,
    replicate: u16,
    order: CollaborationLearningArmOrderV1,
) -> CollaborationLearningComparisonBindingV1 {
    let index = usize::from((replicate - 1) % 6);
    let case = match split {
        CollaborationLearningSplitV1::Train => ['3', '4', '5', '6', '7', '8'][index],
        CollaborationLearningSplitV1::Holdout => ['a', 'b', 'c', 'd', 'e', 'f'][index],
    };
    comparison_with_case(split, replicate, order, case)
}

fn comparison_with_case(
    split: CollaborationLearningSplitV1,
    replicate: u16,
    order: CollaborationLearningArmOrderV1,
    case: char,
) -> CollaborationLearningComparisonBindingV1 {
    CollaborationLearningComparisonBindingV1::freeze(
        CollaborationLearningComparisonHashesV1 {
            source_commit_sha256: digest('1'),
            suite_sha256: digest('2'),
            case_sha256: digest(case),
            prestate_sha256: digest('4'),
            provider_sha256: digest('5'),
            model_pool_sha256: digest('6'),
            route_profile_sha256: digest('7'),
            prompt_profile_sha256: digest('d'),
            conductor_candidate_sha256: digest('e'),
            workflow_proposal_sha256: digest('f'),
            shared_conductor_anchor_sha256: digest('8'),
            direct_execution_plan_semantic_sha256: digest('0'),
            workflow_execution_plan_semantic_sha256: digest('9'),
            budget_sha256: digest('a'),
            cohort_sha256: digest('c'),
        },
        split,
        replicate,
        order,
    )
    .unwrap()
}

fn pair(
    binding: CollaborationLearningComparisonBindingV1,
    workflow_policy: &CollaborationLearningPolicyV1,
    direct_passed: bool,
    workflow_status: AgentOutcomeTerminalStatusV1,
    workflow_passed: bool,
    lane_succeeded: bool,
    workflow_owner_calls: usize,
) -> CollaborationLearningPairV1 {
    let direct_policy = direct_policy();
    let direct = with_owner_calls(
        with_run_identity(
            outcome_for_policy(
                &direct_policy,
                AgentOutcomeTerminalStatusV1::Completed,
                direct_passed,
                &binding.hashes.direct_execution_plan_semantic_sha256,
                &binding.hashes.budget_sha256,
                &binding.hashes.case_sha256,
            ),
            &binding,
            CollaborationLearningArmV1::Direct,
        ),
        4,
    );
    let workflow = with_owner_calls(
        with_run_identity(
            outcome_for_policy(
                workflow_policy,
                workflow_status,
                workflow_passed,
                &binding.hashes.workflow_execution_plan_semantic_sha256,
                &binding.hashes.budget_sha256,
                &binding.hashes.case_sha256,
            ),
            &binding,
            CollaborationLearningArmV1::Workflow,
        ),
        workflow_owner_calls,
    );
    let direct_exercise = exercise_for_outcome(&direct_policy, &direct, true);
    let workflow_exercise = exercise_for_outcome(workflow_policy, &workflow, lane_succeeded);
    CollaborationLearningPairV1::new(
        CollaborationLearningTrialV1::new(
            binding.clone(),
            CollaborationLearningArmV1::Direct,
            direct,
            direct_exercise,
        )
        .unwrap(),
        CollaborationLearningTrialV1::new(
            binding,
            CollaborationLearningArmV1::Workflow,
            workflow,
            workflow_exercise,
        )
        .unwrap(),
    )
    .unwrap()
}

fn baseline() -> CollaborationLearningPairV1 {
    pair(
        comparison(
            CollaborationLearningSplitV1::Train,
            1,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        &workflow_parent(),
        false,
        AgentOutcomeTerminalStatusV1::Completed,
        true,
        true,
        4,
    )
}

fn evidence(
    holdout: CollaborationLearningComparisonBindingV1,
    min_train_pairs: u16,
) -> CollaborationLearningEvidenceSetV1 {
    let config = config(min_train_pairs);
    let candidate = candidate(std::slice::from_ref(&holdout), &config);
    CollaborationLearningEvidenceSetV1::new(candidate, baseline(), config, vec![holdout]).unwrap()
}

fn candidate_pair(
    binding: CollaborationLearningComparisonBindingV1,
    direct_passed: bool,
    workflow_status: AgentOutcomeTerminalStatusV1,
    workflow_passed: bool,
    lane_succeeded: bool,
) -> CollaborationLearningPairV1 {
    pair(
        binding,
        &candidate_policy(),
        direct_passed,
        workflow_status,
        workflow_passed,
        lane_succeeded,
        4,
    )
}

fn config(train: u16) -> CollaborationLearningConfigV1 {
    CollaborationLearningConfigV1::freeze(train, 1, 1, 2_500, 1, 4).unwrap()
}

fn reseal(outcome: ExternallyVerifiedOutcomeV1) -> ExternallyVerifiedOutcomeV1 {
    ExternallyVerifiedOutcomeV1::new(
        outcome.lifecycle,
        outcome.exposure,
        outcome.verifier,
        outcome.postconditions,
        outcome.resources,
    )
    .unwrap()
}

fn with_run_identity(
    mut outcome: ExternallyVerifiedOutcomeV1,
    binding: &CollaborationLearningComparisonBindingV1,
    arm: CollaborationLearningArmV1,
) -> ExternallyVerifiedOutcomeV1 {
    outcome.lifecycle.agent_run_id = format!("fixture-{arm:?}-{}", binding.digest());
    outcome.lifecycle.strategy_receipt_key = collaboration_learning_sha256(
        b"cindx.test.collaboration-learning-strategy\0",
        &(binding.digest(), arm),
        "fixture strategy",
    )
    .unwrap();
    outcome.lifecycle.terminal_commit_key = collaboration_learning_sha256(
        b"cindx.test.collaboration-learning-terminal\0",
        &(binding.digest(), arm),
        "fixture terminal",
    )
    .unwrap();
    reseal(outcome)
}

fn with_owner_calls(
    mut outcome: ExternallyVerifiedOutcomeV1,
    owner_calls: usize,
) -> ExternallyVerifiedOutcomeV1 {
    let added = owner_calls - outcome.exposure.successful_owner_model_calls;
    outcome.exposure.successful_owner_model_calls = owner_calls;
    outcome.exposure.logical_model_calls += added;
    outcome.resources.logical_model_calls += added;
    for usage in [
        &mut outcome.resources.terminal.segment,
        &mut outcome.resources.terminal.lineage,
    ] {
        usage.physical_model_attempts += u64::try_from(added).unwrap();
        usage.provider_usage_attempts += u64::try_from(added).unwrap();
    }
    reseal(outcome)
}

fn with_partial_usage(mut outcome: ExternallyVerifiedOutcomeV1) -> ExternallyVerifiedOutcomeV1 {
    for usage in [
        &mut outcome.resources.terminal.segment,
        &mut outcome.resources.terminal.lineage,
    ] {
        usage.partial_usage_attempts = usage.provider_usage_attempts;
        usage.provider_usage_attempts = 0;
    }
    reseal(outcome)
}

#[test]
fn agent_collaboration_learning_contract_pair_uses_external_reward_and_retains_lane_failure() {
    let pair = candidate_pair(
        comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        true,
        AgentOutcomeTerminalStatusV1::Failed,
        true,
        false,
    );
    assert_eq!(pair.direct_reward_bps().unwrap(), 10_000);
    assert_eq!(pair.workflow_reward_bps().unwrap(), 0);
    assert_eq!(pair.workflow.outcome.exposure.worker_model_calls, 1);
    assert!(!pair.workflow.exercise.lanes[0].final_success);
    assert_ne!(
        pair.binding.hashes.direct_execution_plan_semantic_sha256,
        pair.binding.hashes.workflow_execution_plan_semantic_sha256
    );

    let mut identical_treatment = pair.binding.hashes.clone();
    identical_treatment.workflow_execution_plan_semantic_sha256 = identical_treatment
        .direct_execution_plan_semantic_sha256
        .clone();
    assert!(CollaborationLearningComparisonBindingV1::freeze(
        identical_treatment,
        CollaborationLearningSplitV1::Train,
        2,
        CollaborationLearningArmOrderV1::DirectFirst,
    )
    .is_err());

    let mut tampered = pair.binding.clone();
    tampered.hashes.workflow_proposal_sha256 = digest('e');
    assert!(CollaborationLearningTrialV1::new(
        tampered,
        CollaborationLearningArmV1::Direct,
        pair.direct.outcome.clone(),
        pair.direct.exercise.clone(),
    )
    .is_err());

    let direct_policy = direct_policy();
    let wrong_plan = outcome_for_policy(
        &direct_policy,
        AgentOutcomeTerminalStatusV1::Completed,
        true,
        &pair.binding.hashes.workflow_execution_plan_semantic_sha256,
        &pair.binding.hashes.budget_sha256,
        &pair.binding.hashes.case_sha256,
    );
    assert!(CollaborationLearningTrialV1::new(
        pair.binding.clone(),
        CollaborationLearningArmV1::Direct,
        wrong_plan.clone(),
        exercise_for_outcome(&direct_policy, &wrong_plan, true),
    )
    .is_err());

    let policy = candidate_policy();
    let mut changed_postcondition = pair.workflow.outcome.clone();
    changed_postcondition.postconditions[0].expected_sha256 = digest('f');
    let changed_postcondition = reseal(changed_postcondition);
    let changed_exercise = exercise_for_outcome(&policy, &changed_postcondition, false);
    let changed_trial = CollaborationLearningTrialV1::new(
        pair.binding.clone(),
        CollaborationLearningArmV1::Workflow,
        changed_postcondition,
        changed_exercise,
    )
    .unwrap();
    assert!(CollaborationLearningPairV1::new(pair.direct.clone(), changed_trial).is_err());
}

#[test]
fn agent_collaboration_learning_contract_dedupes_censors_and_seals_terminal_evidence() {
    let holdout = comparison(
        CollaborationLearningSplitV1::Holdout,
        2,
        CollaborationLearningArmOrderV1::WorkflowFirst,
    );
    let frozen_config = config(1);
    let wrong_candidate = CollaborationLearningCandidateV1::snapshot(
        candidate_policy(),
        Some(workflow_parent()),
        digest('0'),
        frozen_config.digest().to_string(),
        digest('9'),
        1,
    )
    .unwrap();
    assert!(CollaborationLearningEvidenceSetV1::new(
        wrong_candidate,
        baseline(),
        frozen_config.clone(),
        vec![holdout.clone()]
    )
    .is_err());
    let wrong_config_candidate = CollaborationLearningCandidateV1::snapshot(
        candidate_policy(),
        Some(workflow_parent()),
        CollaborationLearningCandidateV1::freeze_holdout_manifest(std::slice::from_ref(&holdout))
            .unwrap(),
        digest('0'),
        digest('9'),
        1,
    )
    .unwrap();
    assert!(CollaborationLearningEvidenceSetV1::new(
        wrong_config_candidate,
        baseline(),
        frozen_config,
        vec![holdout.clone()]
    )
    .is_err());

    let parent = workflow_parent();
    let repair_policy = CollaborationLearningPolicyV1::candidate(
        &parent,
        CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
        COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::SameModelOnce,
    )
    .unwrap();
    let repair_config = config(1);
    let repair_candidate = CollaborationLearningCandidateV1::snapshot(
        repair_policy.clone(),
        Some(parent),
        CollaborationLearningCandidateV1::freeze_holdout_manifest(std::slice::from_ref(&holdout))
            .unwrap(),
        repair_config.digest().to_string(),
        digest('9'),
        1,
    )
    .unwrap();
    let mut repair_evidence = CollaborationLearningEvidenceSetV1::new(
        repair_candidate,
        baseline(),
        repair_config,
        vec![holdout.clone()],
    )
    .unwrap();
    assert!(repair_evidence
        .append_pair(pair(
            comparison(
                CollaborationLearningSplitV1::Train,
                4,
                CollaborationLearningArmOrderV1::DirectFirst,
            ),
            &repair_policy,
            false,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            true,
            4,
        ))
        .is_err());

    let mut evidence = evidence(holdout.clone(), 1);
    assert!(evidence
        .append_pair(candidate_pair(
            comparison_with_case(
                CollaborationLearningSplitV1::Train,
                6,
                CollaborationLearningArmOrderV1::DirectFirst,
                'b',
            ),
            false,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            true,
        ))
        .is_err());
    let mut foreign_scope = comparison(
        CollaborationLearningSplitV1::Train,
        4,
        CollaborationLearningArmOrderV1::DirectFirst,
    );
    foreign_scope.hashes.provider_sha256 = digest('6');
    foreign_scope.binding_sha256 = foreign_scope.payload_sha256().unwrap();
    assert!(evidence
        .append_pair(candidate_pair(
            foreign_scope,
            false,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            true,
        ))
        .is_err());
    let train = comparison(
        CollaborationLearningSplitV1::Train,
        2,
        CollaborationLearningArmOrderV1::DirectFirst,
    );
    let pair = candidate_pair(
        train,
        false,
        AgentOutcomeTerminalStatusV1::Completed,
        true,
        true,
    );
    let replay_binding = comparison_with_case(
        CollaborationLearningSplitV1::Train,
        3,
        CollaborationLearningArmOrderV1::WorkflowFirst,
        '4',
    );
    let replayed_run = CollaborationLearningPairV1::new(
        CollaborationLearningTrialV1::new(
            replay_binding.clone(),
            CollaborationLearningArmV1::Direct,
            pair.direct.outcome.clone(),
            pair.direct.exercise.clone(),
        )
        .unwrap(),
        CollaborationLearningTrialV1::new(
            replay_binding,
            CollaborationLearningArmV1::Workflow,
            pair.workflow.outcome.clone(),
            pair.workflow.exercise.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    evidence.append_pair(pair.clone()).unwrap();
    assert!(evidence.append_pair(replayed_run).is_err());
    assert!(evidence.append_pair(pair).is_err());
    evidence
        .append_censor(
            CollaborationLearningCensorReceiptV1::new(
                comparison(
                    CollaborationLearningSplitV1::Train,
                    3,
                    CollaborationLearningArmOrderV1::WorkflowFirst,
                ),
                digest('5'),
                CollaborationLearningCensorReasonV1::IncompleteInstrumentation,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(evidence.training_pairs().count(), 1);
    assert_eq!(evidence.censored_count(), 1);
    let aggregate = evidence.aggregate().unwrap();
    assert_eq!(
        aggregate.freeze_reason(),
        Some(CollaborationLearningFreezeReasonV1::InvalidInstrumentation)
    );
    assert!(evidence
        .append_censor(
            CollaborationLearningCensorReceiptV1::new(
                comparison(
                    CollaborationLearningSplitV1::Train,
                    4,
                    CollaborationLearningArmOrderV1::DirectFirst,
                ),
                digest('6'),
                CollaborationLearningCensorReasonV1::InvalidBinding,
            )
            .unwrap()
        )
        .is_err());
}

#[test]
fn agent_collaboration_learning_contract_partial_usage_or_any_regression_freezes() {
    let holdout = comparison(
        CollaborationLearningSplitV1::Holdout,
        5,
        CollaborationLearningArmOrderV1::WorkflowFirst,
    );
    let mut partial = evidence(holdout, 1);
    let binding = comparison(
        CollaborationLearningSplitV1::Train,
        5,
        CollaborationLearningArmOrderV1::DirectFirst,
    );
    let policy = candidate_policy();
    let direct_policy = direct_policy();
    let direct = with_owner_calls(
        with_run_identity(
            outcome_for_policy(
                &direct_policy,
                AgentOutcomeTerminalStatusV1::Completed,
                false,
                &binding.hashes.direct_execution_plan_semantic_sha256,
                &binding.hashes.budget_sha256,
                &binding.hashes.case_sha256,
            ),
            &binding,
            CollaborationLearningArmV1::Direct,
        ),
        4,
    );
    let workflow = with_partial_usage(with_owner_calls(
        with_run_identity(
            outcome_for_policy(
                &policy,
                AgentOutcomeTerminalStatusV1::Completed,
                true,
                &binding.hashes.workflow_execution_plan_semantic_sha256,
                &binding.hashes.budget_sha256,
                &binding.hashes.case_sha256,
            ),
            &binding,
            CollaborationLearningArmV1::Workflow,
        ),
        4,
    ));
    partial
        .append_pair(
            CollaborationLearningPairV1::new(
                CollaborationLearningTrialV1::new(
                    binding.clone(),
                    CollaborationLearningArmV1::Direct,
                    direct.clone(),
                    exercise_for_outcome(&direct_policy, &direct, true),
                )
                .unwrap(),
                CollaborationLearningTrialV1::new(
                    binding,
                    CollaborationLearningArmV1::Workflow,
                    workflow.clone(),
                    exercise_for_outcome(&policy, &workflow, true),
                )
                .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        partial.aggregate().unwrap().freeze_reason(),
        Some(CollaborationLearningFreezeReasonV1::IncompleteResourceReceipts)
    );

    let mut regression = evidence(
        comparison(
            CollaborationLearningSplitV1::Holdout,
            6,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        3,
    );
    regression
        .append_pair(candidate_pair(
            comparison(
                CollaborationLearningSplitV1::Train,
                2,
                CollaborationLearningArmOrderV1::DirectFirst,
            ),
            true,
            AgentOutcomeTerminalStatusV1::Completed,
            false,
            true,
        ))
        .unwrap();
    assert_eq!(
        regression.aggregate().unwrap().freeze_reason(),
        Some(CollaborationLearningFreezeReasonV1::NoUplift)
    );

    let mut resources = evidence(
        comparison(
            CollaborationLearningSplitV1::Holdout,
            4,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        3,
    );
    resources
        .append_pair(pair(
            comparison(
                CollaborationLearningSplitV1::Train,
                2,
                CollaborationLearningArmOrderV1::DirectFirst,
            ),
            &candidate_policy(),
            false,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            true,
            8,
        ))
        .unwrap();
    assert_eq!(
        resources.aggregate().unwrap().freeze_reason(),
        Some(CollaborationLearningFreezeReasonV1::ResourceRegression)
    );
}

#[test]
fn agent_collaboration_learning_contract_only_independent_review_approves_offline_catalog() {
    let incomplete_holdout = vec![
        comparison(
            CollaborationLearningSplitV1::Holdout,
            2,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        comparison(
            CollaborationLearningSplitV1::Holdout,
            3,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
    ];
    let incomplete_config = config(1);
    let mut incomplete = CollaborationLearningEvidenceSetV1::new(
        candidate(&incomplete_holdout, &incomplete_config),
        baseline(),
        incomplete_config,
        incomplete_holdout.clone(),
    )
    .unwrap();
    for binding in [
        comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        incomplete_holdout[0].clone(),
    ] {
        incomplete
            .append_pair(candidate_pair(
                binding,
                false,
                AgentOutcomeTerminalStatusV1::Completed,
                true,
                true,
            ))
            .unwrap();
    }
    assert_eq!(
        incomplete.aggregate().unwrap().status(),
        CollaborationLearningAggregateStatusV1::Collecting
    );

    let holdout = comparison(
        CollaborationLearningSplitV1::Holdout,
        2,
        CollaborationLearningArmOrderV1::WorkflowFirst,
    );
    let mut evidence = evidence(holdout.clone(), 1);
    for binding in [
        comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        holdout,
    ] {
        evidence
            .append_pair(candidate_pair(
                binding,
                false,
                AgentOutcomeTerminalStatusV1::Completed,
                true,
                true,
            ))
            .unwrap();
    }
    let aggregate = evidence.aggregate().unwrap();
    assert_eq!(
        aggregate.status(),
        CollaborationLearningAggregateStatusV1::ReadyForReview
    );

    let review = |reviewer| {
        CollaborationLearningReviewReceiptV1::new(
            digest(reviewer),
            aggregate.review_evidence_sha256().into(),
            evidence.candidate.digest().into(),
            true,
        )
        .unwrap()
    };
    assert!(evidence
        .approve_for_narrow_validation(&aggregate, &review('9'))
        .is_err());
    let admission = evidence
        .approve_for_narrow_validation(&aggregate, &review('8'))
        .unwrap();
    assert_eq!(
        admission.status(),
        CollaborationLearningOfflineAdmissionStatusV1::ApprovedForNarrowValidation
    );
    assert!(admission.offline_catalog_only());
    assert!(!admission.production_eligible());
}
