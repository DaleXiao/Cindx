use super::*;

pub(crate) mod test_support {
    use super::*;
    use crate::outcome_evidence::{
        AgentExternalPostconditionV1, AgentExternalVerifierV1, AgentOutcomeExposureV1,
        AgentOutcomeLifecycleBindingV1, AgentOutcomeResourcesV1, AgentOutcomeTerminalResourcesV1,
        AgentOutcomeTerminalStatusV1, AgentOutcomeUsageV1,
    };

    fn d(value: char) -> String {
        value.to_string().repeat(64)
    }

    pub(crate) fn outcome_for_policy(
        policy: &CollaborationLearningPolicyV1,
        terminal_status: AgentOutcomeTerminalStatusV1,
        behavior_passed: bool,
        plan_sha256: &str,
        budget_sha256: &str,
        case_sha256: &str,
    ) -> ExternallyVerifiedOutcomeV1 {
        let workflow = policy.specialist_invocation
            == CollaborationSpecialistInvocationV1::OneReadOnlySpecialist;
        let verifier =
            workflow && policy.verification == CollaborationVerificationV1::AlwaysIndependent;
        let worker_calls = usize::from(workflow) + usize::from(verifier);
        let mut exposure = AgentOutcomeExposureV1 {
            logical_model_calls: worker_calls + 1,
            worker_model_calls: worker_calls,
            successful_owner_model_calls: 1,
            successful_specialist_model_calls: usize::from(workflow),
            successful_independent_verifier_model_calls: usize::from(verifier),
            successful_workflow_specialist_model_calls: usize::from(workflow),
            successful_workflow_verifier_model_calls: usize::from(verifier),
            workflow_planned: workflow,
            workflow_completed: workflow
                && terminal_status == AgentOutcomeTerminalStatusV1::Completed,
            ..AgentOutcomeExposureV1::default()
        };
        if workflow {
            exposure.worker_models.insert("specialist-model".into());
            exposure
                .successful_workflow_specialist_models
                .insert("specialist-model".into());
        }
        if verifier {
            exposure.worker_models.insert("verifier-model".into());
            exposure
                .successful_workflow_verifier_models
                .insert("verifier-model".into());
        }
        let calls = u64::try_from(worker_calls + 1).unwrap();
        let usage = AgentOutcomeUsageV1 {
            physical_model_attempts: calls,
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            reserved_tokens: 0,
            provider_usage_attempts: calls,
            partial_usage_attempts: 0,
            estimated_usage_attempts: 0,
            unknown_usage_attempts: 0,
        };
        ExternallyVerifiedOutcomeV1::new(
            AgentOutcomeLifecycleBindingV1 {
                agent_run_id: "fixture-run".into(),
                steer_epoch: 0,
                strategy_receipt_key: d('1'),
                strategy_plan_sha256: plan_sha256.into(),
                execution_plan_semantic_sha256: plan_sha256.into(),
                terminal_commit_key: d('2'),
                terminal_status,
                decision_sequence: 1,
                terminal_sequence: 10,
            },
            exposure,
            AgentExternalVerifierV1 {
                kind: "external".into(),
                protocol_sha256: d('3'),
                subject_sha256: case_sha256.into(),
                safety_violations: 0,
            },
            vec![AgentExternalPostconditionV1 {
                kind: "behavior".into(),
                subject_sha256: case_sha256.into(),
                expected_sha256: d('4'),
                observed_sha256: Some(d('5')),
                artifact_sha256: None,
                bytes: None,
                passed: behavior_passed,
                preservation: false,
            }],
            AgentOutcomeResourcesV1::new(
                budget_sha256.into(),
                d('6'),
                d('7'),
                10,
                worker_calls + 1,
                0,
                AgentOutcomeTerminalResourcesV1 {
                    segment: usage.clone(),
                    lineage: usage,
                },
            )
            .unwrap(),
        )
        .unwrap()
    }

    pub(crate) fn exercise_for_outcome(
        policy: &CollaborationLearningPolicyV1,
        outcome: &ExternallyVerifiedOutcomeV1,
        lane_succeeded: bool,
    ) -> CollaborationLearningExerciseV1 {
        let workflow = policy.specialist_invocation
            == CollaborationSpecialistInvocationV1::OneReadOnlySpecialist;
        let verifier =
            workflow && policy.verification == CollaborationVerificationV1::AlwaysIndependent;
        let repair_model = |initial: &str, alternate: &str| match policy.repair {
            CollaborationRepairV1::FailFast => None,
            CollaborationRepairV1::SameModelOnce => Some(initial.to_string()),
            CollaborationRepairV1::AlternateModelOnce => Some(alternate.to_string()),
        };
        let lane = |actor, step: &str, model: &str, start| CollaborationLearningLaneExerciseV1 {
            actor,
            workflow_step_id: step.into(),
            output_kind: if actor == CollaborationLearningLaneActorV1::Specialist {
                CollaborationLearningOutputKindV1::Analysis
            } else {
                CollaborationLearningOutputKindV1::Verification
            },
            attempts: vec![CollaborationLearningWorkerAttemptV1 {
                request_id: format!("{step}-request"),
                model: model.into(),
                started_sequence: start,
                finished_sequence: start + 1,
                recovery_attempt: None,
                context: CollaborationLearningContextReceiptV1 {
                    budget_bps: policy.context_budget_bps,
                    payload_sha256: d('8'),
                    bytes: 64,
                },
                provider_response_observed: true,
                succeeded: lane_succeeded,
            }],
            final_success: lane_succeeded,
        };
        let specialist_assignment = workflow.then(|| CollaborationLearningLaneAssignmentV1 {
            actor: CollaborationLearningLaneActorV1::Specialist,
            workflow_step_id: "specialist".into(),
            output_kind: CollaborationLearningOutputKindV1::Analysis,
            initial_model: "specialist-model".into(),
            repair_model: repair_model("specialist-model", "specialist-repair-model"),
        });
        let verifier_assignment = verifier.then(|| CollaborationLearningLaneAssignmentV1 {
            actor: CollaborationLearningLaneActorV1::IndependentVerifier,
            workflow_step_id: "verify".into(),
            output_kind: CollaborationLearningOutputKindV1::Verification,
            initial_model: "verifier-model".into(),
            repair_model: repair_model("verifier-model", "verifier-repair-model"),
        });
        let mut lanes = Vec::new();
        if workflow {
            lanes.push(lane(
                CollaborationLearningLaneActorV1::Specialist,
                "specialist",
                "specialist-model",
                3,
            ));
        }
        if verifier && lane_succeeded {
            lanes.push(lane(
                CollaborationLearningLaneActorV1::IndependentVerifier,
                "verify",
                "verifier-model",
                5,
            ));
        }
        let assignment = CollaborationLearningAssignmentV1 {
            schema: COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA.into(),
            agent_run_id: outcome.lifecycle.agent_run_id.clone(),
            steer_epoch: outcome.lifecycle.steer_epoch,
            execution_plan_semantic_sha256: outcome
                .lifecycle
                .execution_plan_semantic_sha256
                .clone(),
            policy_json: policy.to_json().unwrap(),
            policy_sha256: policy.policy_sha256.clone(),
            case_binding_sha256: outcome.verifier.subject_sha256.clone(),
            assignment_sequence: 2,
            plan_required_independent_verifier: false,
            collaboration_id: workflow.then(|| "fixture-collaboration".into()),
            specialist: specialist_assignment,
            verifier: verifier_assignment,
        };
        let mut exercise = CollaborationLearningExerciseV1 {
            schema: COLLABORATION_LEARNING_EXERCISE_SCHEMA.into(),
            outcome_receipt_sha256: outcome.receipt_sha256.clone(),
            stop_reason: derive_stop(policy, &assignment, &lanes).unwrap(),
            assignment,
            lanes,
            receipt_sha256: String::new(),
        };
        exercise.receipt_sha256 = exercise_digest(&exercise).unwrap();
        exercise.validate_against(policy, outcome).unwrap();
        exercise
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::outcome_evidence::AgentOutcomeTerminalStatusV1;
    use agent_core::{
        insert_event_type_v1, AgentActor, AgentEffectAuthority, AgentModelAttribution,
        AgentModelProfile, AgentStage, EventId, TaskId,
    };
    use test_support::outcome_for_policy;

    fn d(value: char) -> String {
        value.to_string().repeat(64)
    }
    fn policy(repair: CollaborationRepairV1) -> CollaborationLearningPolicyV1 {
        CollaborationLearningPolicyV1::seed(
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            5_000,
            CollaborationVerificationV1::PlanRequiredOnly,
            repair,
        )
        .unwrap()
    }
    fn event(sequence: u64, kind: EventKind) -> Event {
        Event {
            id: EventId(sequence.to_string()),
            task_id: TaskId("task".into()),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: "worker".into(),
            metadata: [
                (RUN.into(), "fixture-run".into()),
                (EPOCH.into(), "0".into()),
            ]
            .into_iter()
            .collect(),
        }
    }
    fn assignment(
        policy: &CollaborationLearningPolicyV1,
        model: &str,
        repair: Option<&str>,
    ) -> Event {
        let mut event = event(2, EventKind::TaskStatusChanged);
        event.summary = COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY.into();
        event.metadata.extend([
            (
                COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY.into(),
                COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA.into(),
            ),
            (PLAN.into(), d('a')),
            (
                COLLABORATION_LEARNING_POLICY_JSON_METADATA_KEY.into(),
                policy.to_json().unwrap(),
            ),
            (
                COLLABORATION_LEARNING_POLICY_SHA256_METADATA_KEY.into(),
                policy.policy_sha256.clone(),
            ),
            (
                COLLABORATION_LEARNING_CASE_BINDING_SHA256_METADATA_KEY.into(),
                d('c'),
            ),
            (
                COLLABORATION_LEARNING_PLAN_REQUIRED_INDEPENDENT_VERIFIER_METADATA_KEY.into(),
                "false".into(),
            ),
            (COLLABORATION.into(), "fixture-collaboration".into()),
            (
                COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY.into(),
                "specialist".into(),
            ),
            (
                COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY.into(),
                "analysis".into(),
            ),
            (
                COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY.into(),
                model.into(),
            ),
        ]);
        if let Some(model) = repair {
            event.metadata.insert(
                COLLABORATION_LEARNING_SPECIALIST_REPAIR_MODEL_METADATA_KEY.into(),
                model.into(),
            );
        }
        event
    }
    fn worker_event(
        sequence: u64,
        start: bool,
        request: &str,
        model: &str,
        recovery: bool,
        success: bool,
    ) -> Event {
        let mut event = event(
            sequence,
            if start {
                EventKind::ModelRequestStarted
            } else {
                EventKind::ModelRequestFinished
            },
        );
        insert_event_type_v1(
            &event.kind,
            &mut event.metadata,
            if start {
                EventTypeV1::AgentModelTurnStarted
            } else {
                EventTypeV1::AgentModelTurnFinished
            },
        )
        .unwrap();
        event.metadata.extend([
            (REQUEST.into(), request.into()),
            (STEP.into(), "specialist".into()),
            (OUTPUT.into(), "analysis".into()),
            (COLLABORATION.into(), "fixture-collaboration".into()),
            ("model".into(), model.into()),
        ]);
        AgentModelAttribution::actor(
            AgentActor::Specialist,
            AgentStage::Plan,
            AgentModelProfile::Primary,
            AgentEffectAuthority::ReadOnly,
        )
        .insert_into(&mut event.metadata, model, "executor", "specialist")
        .unwrap();
        if recovery {
            event.metadata.insert(RECOVERY.into(), "true".into());
            event.metadata.insert(RECOVERY_ATTEMPT.into(), "2".into());
        }
        if start {
            event.metadata.extend([
                (
                    COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY.into(),
                    COLLABORATION_LEARNING_CONTEXT_RECEIPT_SCHEMA.into(),
                ),
                (
                    COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY.into(),
                    "5000".into(),
                ),
                (
                    COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY.into(),
                    d('8'),
                ),
                (
                    COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY.into(),
                    "64".into(),
                ),
            ]);
        } else {
            event.metadata.insert(
                "status".into(),
                if success { "completed" } else { "degraded" }.into(),
            );
            if success {
                event.metadata.insert("output".into(), "result".into());
            }
        }
        event
    }
    fn outcome(
        policy: &CollaborationLearningPolicyV1,
        repair: bool,
    ) -> ExternallyVerifiedOutcomeV1 {
        let mut outcome = outcome_for_policy(
            policy,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            &d('a'),
            &d('b'),
            &d('c'),
        );
        if repair {
            outcome.exposure.worker_model_calls = 2;
            outcome.exposure.logical_model_calls = 3;
            outcome
                .exposure
                .worker_models
                .insert("alternate-model".into());
            outcome
                .exposure
                .successful_workflow_specialist_models
                .clear();
            outcome
                .exposure
                .successful_workflow_specialist_models
                .insert("alternate-model".into());
            outcome.resources.logical_model_calls = 3;
            outcome.receipt_sha256.clear();
            outcome = ExternallyVerifiedOutcomeV1::new(
                outcome.lifecycle,
                outcome.exposure,
                outcome.verifier,
                outcome.postconditions,
                outcome.resources,
            )
            .unwrap();
        }
        outcome
    }

    #[test]
    fn agent_collaboration_learning_contract_projects_direct_workflow_and_repair() {
        let direct = CollaborationLearningPolicyV1::seed(
            CollaborationSpecialistInvocationV1::DirectOwnerOnly,
            0,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .unwrap();
        let direct_outcome = outcome_for_policy(
            &direct,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            &d('a'),
            &d('b'),
            &d('c'),
        );
        let direct_event = {
            let mut event = assignment(&direct, "unused", None);
            for key in [
                COLLABORATION,
                COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY,
                COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY,
                COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY,
            ] {
                event.metadata.remove(key);
            }
            event
        };
        assert_eq!(
            CollaborationLearningExerciseV1::from_events(&[direct_event], &direct_outcome)
                .unwrap()
                .stop_reason,
            CollaborationDerivedStopReasonV1::DirectOwnerTerminal,
        );

        let workflow = policy(CollaborationRepairV1::FailFast);
        let workflow_outcome = outcome(&workflow, false);
        let workflow_events = vec![
            assignment(&workflow, "specialist-model", None),
            worker_event(3, true, "one", "specialist-model", false, true),
            worker_event(4, false, "one", "specialist-model", false, true),
        ];
        let projected =
            CollaborationLearningExerciseV1::from_events(&workflow_events, &workflow_outcome)
                .unwrap();
        assert_eq!(
            CollaborationLearningExerciseV1::from_json(&projected.to_json().unwrap()).unwrap(),
            projected
        );

        let repair_policy = policy(CollaborationRepairV1::AlternateModelOnce);
        let repair_outcome = outcome(&repair_policy, true);
        let repair_events = vec![
            assignment(&repair_policy, "specialist-model", Some("alternate-model")),
            worker_event(3, true, "one", "specialist-model", false, false),
            worker_event(4, false, "one", "specialist-model", false, false),
            worker_event(5, true, "two", "alternate-model", true, true),
            worker_event(6, false, "two", "alternate-model", true, true),
        ];
        assert_eq!(
            CollaborationLearningExerciseV1::from_events(&repair_events, &repair_outcome)
                .unwrap()
                .stop_reason,
            CollaborationDerivedStopReasonV1::RequiredLanesCompletedAfterRepair,
        );
    }

    #[test]
    fn agent_collaboration_learning_contract_censors_missing_context_and_tamper() {
        let policy = policy(CollaborationRepairV1::FailFast);
        let outcome = outcome(&policy, false);
        let mut events = vec![
            assignment(&policy, "specialist-model", None),
            worker_event(3, true, "one", "specialist-model", false, true),
            worker_event(4, false, "one", "specialist-model", false, true),
        ];
        events[1]
            .metadata
            .remove(COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY);
        assert!(
            CollaborationLearningExerciseV1::from_events(&events, &outcome)
                .unwrap_err()
                .to_string()
                .contains("censored")
        );
        events[1].metadata.insert(
            COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY.into(),
            d('8'),
        );
        let mut attribution_tamper = events.clone();
        attribution_tamper[1]
            .metadata
            .insert(AGENT_ACTOR_METADATA_KEY.into(), "owner".into());
        assert!(
            CollaborationLearningExerciseV1::from_events(&attribution_tamper, &outcome)
                .unwrap_err()
                .to_string()
                .contains("censored")
        );
        let mut finish_attribution_tamper = events.clone();
        finish_attribution_tamper[2]
            .metadata
            .insert(AGENT_STAGE_METADATA_KEY.into(), "evidence".into());
        assert!(
            CollaborationLearningExerciseV1::from_events(&finish_attribution_tamper, &outcome)
                .unwrap_err()
                .to_string()
                .contains("censored")
        );
        let mut late_assignment = assignment(&policy, "specialist-model", None);
        late_assignment.sequence = outcome.lifecycle.terminal_sequence + 1;
        let mut duplicate_assignment = events.clone();
        duplicate_assignment.push(late_assignment);
        let duplicate_error =
            CollaborationLearningExerciseV1::from_events(&duplicate_assignment, &outcome)
                .unwrap_err()
                .to_string();
        assert!(duplicate_error.contains("censored") && duplicate_error.contains("duplicated"));
        let mut projected =
            CollaborationLearningExerciseV1::from_events(&events, &outcome).unwrap();
        projected.assignment.case_binding_sha256 = d('9');
        assert!(projected.validate().is_err());
    }

    #[test]
    fn agent_collaboration_learning_contract_censors_declared_actual_mismatch() {
        let policy = policy(CollaborationRepairV1::FailFast);
        let outcome = outcome(&policy, false);
        let events = vec![
            assignment(&policy, "declared-model", None),
            worker_event(3, true, "one", "actual-model", false, true),
            worker_event(4, false, "one", "actual-model", false, true),
        ];
        let error = CollaborationLearningExerciseV1::from_events(&events, &outcome)
            .unwrap_err()
            .to_string();
        assert!(error.contains("censored") && error.contains("declared and actual"));
    }
}
