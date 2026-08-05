use super::*;
use crate::prompt_attempt_runtime::{
    PROMPT_EVALUATION_ATTEMPT_EVENT, PROMPT_EVALUATION_ATTEMPT_METADATA_KEY,
};
use agent_core::{EventTypeV1, EVENT_TYPE_METADATA_KEY};
use orchestrator::{
    LearningAttribution, LearningTermination, PromptDatasetCaseIdentityV1, PromptDatasetIdentityV1,
    PromptEvaluationAttemptEventV1, PromptEvaluationAttemptStatus, PromptExecutionContextV1,
    PromptLearningCohortV1, PromptMatchedEvaluationIdentityV1, PromptTransferProvenance,
    PromptTreatmentIdentityV1, PROMPT_EXECUTION_CONTEXT_SCHEMA_V1,
};

fn context_metadata() -> Metadata {
    [
        ("collaboration_id".to_string(), "collab-live".to_string()),
        ("agent_run_id".to_string(), "run-live".to_string()),
        ("prompt_profile".to_string(), "profile-live".to_string()),
        ("prompt_effort".to_string(), "pro".to_string()),
        ("collaboration_profile".to_string(), "bounded".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect()
}

fn event(
    sequence: u64,
    kind: EventKind,
    summary: &str,
    extra: impl IntoIterator<Item = (String, String)>,
) -> Event {
    let mut metadata = context_metadata();
    metadata.extend(extra);
    Event {
        id: EventId(format!("event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 100,
        kind,
        summary: summary.to_string(),
        metadata,
    }
}

fn prompt_rollout_event(
    sequence: u64,
    status: &str,
    stable_profile_id: &str,
    canary_profile_id: Option<&str>,
    canary_percent: u8,
    rollback_count: usize,
    quarantined_profile_ids: &[&str],
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
) -> Event {
    event(
        sequence,
        EventKind::TaskStatusChanged,
        "Conductor prompt rollout updated",
        [
            ("project_id".to_string(), "project-a".to_string()),
            ("prompt_effort".to_string(), "auto".to_string()),
            ("prompt_rollout_scope".to_string(), "project-a".to_string()),
            ("stable_profile".to_string(), stable_profile_id.to_string()),
            (
                "canary_profile".to_string(),
                canary_profile_id.unwrap_or_default().to_string(),
            ),
            ("canary_percent".to_string(), canary_percent.to_string()),
            ("rollout_status".to_string(), status.to_string()),
            ("evidence_checkpoint".to_string(), sequence.to_string()),
            ("live_checkpoint".to_string(), sequence.to_string()),
            ("stable_live_checkpoint".to_string(), sequence.to_string()),
            (
                "quarantined_profiles".to_string(),
                serde_json::to_string(quarantined_profile_ids).unwrap(),
            ),
            ("distillation_canary_lease".to_string(), String::new()),
            ("rollback_count".to_string(), rollback_count.to_string()),
            (
                "frozen_prompt_profile".to_string(),
                frozen_profile
                    .map(|snapshot| serde_json::to_string(snapshot).unwrap())
                    .unwrap_or_default(),
            ),
        ],
    )
}

fn with_distillation_lease(
    mut event: Event,
    stable_profile_id: &str,
    candidate_profile_id: &str,
) -> Event {
    let lease = PromptDistillationCanaryLeaseV1 {
        schema: PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1.to_string(),
        candidate_profile_id: candidate_profile_id.to_string(),
        candidate_profile_sha256: "a".repeat(64),
        stable_profile_id: stable_profile_id.to_string(),
        stable_profile_sha256: "b".repeat(64),
        cohort_sha256: "c".repeat(64),
        paired_evidence_sha256: "d".repeat(64),
    };
    event.metadata.insert(
        "distillation_canary_lease".to_string(),
        serde_json::to_string(&lease).unwrap(),
    );
    event
}

fn complete_run_lineage(total_tokens: &str) -> Vec<(String, String)> {
    complete_run_lineage_with_attempts(total_tokens, "1")
}

fn complete_run_lineage_with_attempts(total_tokens: &str, attempts: &str) -> Vec<(String, String)> {
    vec![
        (
            "run_lineage_physical_model_attempts".to_string(),
            attempts.to_string(),
        ),
        (
            "run_lineage_provider_usage_attempts".to_string(),
            attempts.to_string(),
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
        (
            "run_lineage_prompt_tokens".to_string(),
            total_tokens.to_string(),
        ),
        ("run_lineage_completion_tokens".to_string(), "0".to_string()),
        (
            "run_lineage_total_tokens".to_string(),
            total_tokens.to_string(),
        ),
    ]
}

fn live_events(
    terminal_summary: &str,
    learning_evidence: Option<String>,
    total_tokens: Option<&str>,
    permission_denied: bool,
    uplift_bps: Option<&str>,
) -> Vec<Event> {
    let mut events = vec![
        event(
            1,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile selected",
            [],
        ),
        event(
            2,
            EventKind::ModelRequestFinished,
            "Model request finished",
            total_tokens
                .map(|tokens| vec![("total_tokens".to_string(), tokens.to_string())])
                .unwrap_or_default(),
        ),
    ];
    if permission_denied {
        events.push(event(
            3,
            EventKind::PermissionResolved,
            "Permission denied",
            [("decision".to_string(), "deny".to_string())],
        ));
    }
    events.push(event(
        4,
        EventKind::TaskStatusChanged,
        "Collaboration workflow completed",
        uplift_bps
            .map(|uplift| vec![("anytime_team_uplift_bps".to_string(), uplift.to_string())])
            .unwrap_or_default(),
    ));
    let mut terminal_metadata = learning_evidence
        .map(|evidence| vec![("learning_evidence_v1".to_string(), evidence)])
        .unwrap_or_default();
    if let Some(total_tokens) = total_tokens {
        terminal_metadata.extend(complete_run_lineage(total_tokens));
    }
    events.push(event(
        5,
        EventKind::TaskStatusChanged,
        terminal_summary,
        terminal_metadata,
    ));
    events
}

fn quality_evidence(source: IndependentQualitySource, quality_bps: u16, passed: bool) -> String {
    quality_evidence_at(source, quality_bps, passed, 0)
}

fn quality_evidence_at(
    source: IndependentQualitySource,
    quality_bps: u16,
    passed: bool,
    steer_epoch: u64,
) -> String {
    LearningEvidenceV1::independent_quality(
        LearningTermination::Completed,
        LearningAttribution::Workflow,
        LearningUsageCompleteness::Complete,
        steer_epoch,
        "a".repeat(64),
        source,
        quality_bps,
        passed,
    )
    .to_metadata_value()
    .expect("valid learning evidence")
}

#[test]
fn live_observations_map_only_trusted_positive_and_negative_evidence() {
    let mut positive_events = live_events(
        "Agent task completed",
        Some(quality_evidence(
            IndependentQualitySource::AnytimeSelector,
            8_200,
            true,
        )),
        Some("42"),
        false,
        Some("1200"),
    );
    positive_events
        .last_mut()
        .expect("terminal event")
        .metadata
        .insert("run_lineage_total_tokens".to_string(), "43".to_string());
    let positive = prompt_evolution_observations_from_events(&positive_events);
    assert_eq!(positive.len(), 1);
    assert!(positive[0].1.succeeded);
    assert!((positive[0].1.quality_score - 0.82).abs() < 0.000_001);
    assert_eq!(positive[0].1.relative_reward, Some(0.12));
    assert_eq!(positive[0].1.total_tokens, 43);

    let negative = prompt_evolution_observations_from_events(&live_events(
        "Agent task completed",
        Some(quality_evidence(
            IndependentQualitySource::CollaborationQualityGate,
            3_500,
            false,
        )),
        Some("17"),
        false,
        Some("9000"),
    ));
    assert_eq!(negative.len(), 1);
    assert!(!negative[0].1.succeeded);
    assert!((negative[0].1.quality_score - 0.35).abs() < 0.000_001);
    assert_eq!(negative[0].1.relative_reward, None);
    assert_eq!(negative[0].1.total_tokens, 17);
}

#[test]
fn live_observations_reject_invalid_agent_terminal_tags() {
    let evidence = quality_evidence(
        IndependentQualitySource::CollaborationQualityGate,
        8_200,
        true,
    );
    for invalid_tag in [
        "cindx.event.v2/agent.run.completed",
        EventTypeV1::AgentRunFailed.id(),
    ] {
        let mut events = live_events(
            "Agent task completed",
            Some(evidence.clone()),
            Some("42"),
            false,
            Some("1200"),
        );
        events
            .last_mut()
            .expect("terminal event")
            .metadata
            .insert(EVENT_TYPE_METADATA_KEY.to_string(), invalid_tag.to_string());

        assert!(prompt_evolution_observations_from_events(&events).is_empty());
    }
}

fn assert_negative_live_control(events: &[Event]) -> PromptEvolutionObservation {
    let observations = prompt_evolution_observations_from_events(events);
    assert_eq!(observations.len(), 1);
    let observation = observations[0].1.clone();
    assert_eq!(observation.mode, PromptEvaluationMode::Live);
    assert!(!observation.succeeded);
    assert!(!observation.format_valid);
    assert!(!observation.is_scientific_evidence());
    observation
}

#[test]
fn verified_postcondition_is_only_a_negative_live_control() {
    let evidence = LearningEvidenceV1::verified_postcondition(
        LearningUsageCompleteness::Complete,
        0,
        "b".repeat(64),
    )
    .to_metadata_value()
    .expect("valid learning evidence");
    let events = live_events(
        "Agent task completed",
        Some(evidence),
        Some("8"),
        false,
        Some("2500"),
    );

    assert_negative_live_control(&events);
}

#[test]
fn untrusted_or_incomplete_runs_only_enter_the_live_canary_control_plane() {
    assert_negative_live_control(&live_events(
        "Agent task completed",
        None,
        Some("20"),
        false,
        None,
    ));

    let cancelled = LearningEvidenceV1::censored(
        LearningTermination::Cancelled,
        LearningAttribution::User,
        LearningUsageCompleteness::Complete,
        Some(0),
        Some("c".repeat(64)),
    )
    .to_metadata_value()
    .expect("valid censored evidence");
    assert_negative_live_control(&live_events(
        "Agent task cancelled",
        Some(cancelled),
        Some("20"),
        false,
        None,
    ));

    let trusted = quality_evidence(
        IndependentQualitySource::CollaborationQualityGate,
        8_000,
        true,
    );
    assert_negative_live_control(&live_events(
        "Agent task completed",
        Some(trusted.clone()),
        None,
        false,
        None,
    ));
    let denied = assert_negative_live_control(&live_events(
        "Agent task completed",
        Some(trusted),
        Some("20"),
        true,
        None,
    ));
    assert_eq!(denied.safety_violations, 1);

    let trusted = quality_evidence(
        IndependentQualitySource::CollaborationQualityGate,
        8_000,
        true,
    );
    let mut inconsistent_usage = live_events(
        "Agent task completed",
        Some(trusted),
        Some("20"),
        false,
        None,
    );
    inconsistent_usage
        .last_mut()
        .expect("terminal event")
        .metadata
        .insert(
            "run_lineage_provider_usage_attempts".to_string(),
            "2".to_string(),
        );
    assert_negative_live_control(&inconsistent_usage);

    let direct_evidence = LearningEvidenceV1::independent_quality(
        LearningTermination::Completed,
        LearningAttribution::Model,
        LearningUsageCompleteness::Complete,
        0,
        "d".repeat(64),
        IndependentQualitySource::CollaborationQualityGate,
        8_000,
        true,
    )
    .to_metadata_value()
    .expect("valid direct evidence");
    assert_negative_live_control(&live_events(
        "Agent task completed",
        Some(direct_evidence),
        Some("20"),
        false,
        None,
    ));
}

fn completed_denial_metadata() -> Metadata {
    let mut contract = agent_runtime::AgentTaskContract::default();
    contract.begin_action_denial_epoch(0);
    contract.require_tool_success("shell.run");
    let input_fingerprint = agent_runtime::tool_input_fingerprint(
        "shell.run",
        r#"{"command":"SENSITIVE_FAILURE_CURRICULUM_SENTINEL"}"#,
    );
    let denial = contract
        .record_action_denial(
            "shell.run",
            &input_fingerprint,
            &agent_runtime::AgentActionDenialFeedback::runtime_policy(
                "shell_policy_denied",
                agent_runtime::AgentActionRecovery::FinalizeBlocked,
            ),
        )
        .expect("runtime denial should be canonical");
    let ledger = contract.completed_outcome_ledger(agent_runtime::OutcomeTerminalObservation {
        steer_epoch: 0,
        model_turn: 1,
        answer: "The action was denied, so no command was run.",
        selected_stage: "blocked_finalizer",
        selector_quality: ResultQuality::Grounded,
        selector_marked_verified: false,
        selector_marked_deliverable: true,
        selector_evidence_count: 1,
        trusted_evidence_sequences: &[denial.evidence_sequence],
    });
    let mut metadata = Metadata::new();
    assert!(ledger.insert_metadata(&mut metadata));
    metadata.insert("outcome_ledger_status".to_string(), "recorded".to_string());
    metadata
}

fn failed_ledger_metadata(failure: &AgentFailure) -> Metadata {
    let ledger = agent_runtime::AgentTaskContract::default().failed_outcome_ledger(0, failure);
    let mut metadata = Metadata::new();
    assert!(ledger.insert_metadata(&mut metadata));
    metadata.insert("outcome_ledger_status".to_string(), "recorded".to_string());
    metadata
}

#[test]
fn agent_failure_curriculum_contract() {
    let mut denied_events = live_events(
        "Agent task completed",
        Some(quality_evidence(
            IndependentQualitySource::CollaborationQualityGate,
            9_000,
            true,
        )),
        Some("20"),
        false,
        None,
    );
    denied_events[3].metadata.insert(
        "anytime_prompt_learning_eligible".to_string(),
        "true".to_string(),
    );
    denied_events
        .last_mut()
        .expect("terminal event")
        .metadata
        .extend(completed_denial_metadata());

    let denied_terminal = denied_events.last().expect("terminal event");
    assert!(crate::prompt_failure_curriculum_projection::terminal_outcome_ledger_has_blocking_denial(
        denied_terminal
    ));
    let denied_observation = assert_negative_live_control(&denied_events);
    assert_eq!(denied_observation.safety_violations, 1);
    let denied_records = crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&denied_events);
    assert_eq!(denied_records.len(), 1);
    assert_eq!(
        denied_records[0].1.receipt.kind,
        PromptFailureCurriculumKind::Denial
    );
    let encoded_denial = serde_json::to_string(&denied_records[0].1.receipt).unwrap();
    assert!(!encoded_denial.contains("SENSITIVE_FAILURE_CURRICULUM_SENTINEL"));
    assert!(!encoded_denial.contains("shell.run"));

    let objective = crate::prompt_learning_runtime::redact_prompt_learning_text(
        "Run the requested bounded command.",
    );
    let run_events = denied_events.iter().collect::<Vec<_>>();
    let teacher_receipt = crate::prompt_learning_runtime::prompt_learning_receipt(
        orchestrator::PromptLearningPurpose::AutoTeacher,
        "run-live",
        "global",
        "coding",
        &objective,
        &run_events,
        denied_terminal,
        0,
    );
    assert_eq!(
        teacher_receipt.rejection,
        orchestrator::PromptLearningRejection::PermissionDenied
    );
    assert!(!teacher_receipt.is_eligible());

    let mut no_progress_events = vec![event(
        10,
        EventKind::TaskStatusChanged,
        "Conductor prompt profile selected",
        [],
    )];
    no_progress_events.push(event(
        11,
        EventKind::Error,
        "Agent task failed",
        failed_ledger_metadata(&AgentFailure::contract(
            "no_progress",
            "SENSITIVE_NO_PROGRESS_SENTINEL",
        )),
    ));
    let no_progress_records = crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&no_progress_events);
    assert_eq!(no_progress_records.len(), 1);
    assert_eq!(
        no_progress_records[0].1.receipt.kind,
        PromptFailureCurriculumKind::NoProgress
    );
    assert!(!serde_json::to_string(&no_progress_records[0].1.receipt)
        .unwrap()
        .contains("SENSITIVE_NO_PROGRESS_SENTINEL"));

    let mut provider_events = vec![event(
        20,
        EventKind::TaskStatusChanged,
        "Conductor prompt profile selected",
        [],
    )];
    provider_events.push(event(
        21,
        EventKind::Error,
        "Agent task failed",
        failed_ledger_metadata(&AgentFailure::new(
            "provider_timeout",
            "provider noise",
            AgentFailureClass::ProviderTransient,
            true,
        )),
    ));
    assert!(crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&provider_events).is_empty());

    let envelope = AgentRecoveryEnvelope {
        schema: AGENT_RECOVERY_SCHEMA.to_string(),
        identity: AgentRecoveryIdentity {
            project_id: None,
            session_id: "session-live".to_string(),
            resume_key: "resume-live".to_string(),
            source_run_id: "run-live".to_string(),
            user_turn_sequence: 1,
            prompt_fingerprint: "prompt-fingerprint".to_string(),
        },
        effort: "pro".to_string(),
        policy: "pro".to_string(),
        queue_id: None,
        workflow_resume_key: None,
        state: AgentRecoveryState::Paused,
        reason: AgentRecoveryReason::DeadlineExceeded,
        attempts: 0,
        model_calls: 1,
        tool_calls: 0,
        material_checkpoints: 0,
        observations: 0,
        budget_extensions: 0,
        task_state: None,
        resource_snapshot: None,
        created_at_ms: 1,
        updated_at_ms: 2,
    };
    let pause_metadata = [
        ("stop_reason".to_string(), "deadline_exceeded".to_string()),
        (
            "recovery_reason".to_string(),
            "deadline_exceeded".to_string(),
        ),
        (
            "recovery_envelope".to_string(),
            serde_json::to_string(&envelope).unwrap(),
        ),
    ];
    let paused_events = vec![
        event(
            30,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile selected",
            [],
        ),
        event(
            31,
            EventKind::TaskStatusChanged,
            "Agent task paused",
            pause_metadata.clone(),
        ),
        event(
            32,
            EventKind::TaskStatusChanged,
            "Agent task paused",
            pause_metadata,
        ),
    ];
    let timeout_records = crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&paused_events);
    assert_eq!(timeout_records.len(), 1);
    assert_eq!(
        timeout_records[0].1.receipt.kind,
        PromptFailureCurriculumKind::Timeout
    );
    let first_digest = timeout_records[0].1.receipt.digest().unwrap();
    let replay_digest = crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&paused_events)[0]
        .1
        .receipt
        .digest()
        .unwrap();
    assert_eq!(first_digest, replay_digest);

    let mut hot_curricula = vec![timeout_records[0].1.clone()];
    let mut resolved_events = paused_events.clone();
    resolved_events.push(event(
        33,
        EventKind::TaskStatusChanged,
        "Agent task completed",
        [],
    ));
    let resolved_records = crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&resolved_events)
        .into_iter()
        .map(|(_, record)| record)
        .collect();
    replace_prompt_failure_curriculum_for_run(&mut hot_curricula, "run-live", resolved_records);
    assert!(hot_curricula.is_empty());

    let mut stale_events = paused_events;
    stale_events[2]
        .metadata
        .insert("steer_epoch".to_string(), "1".to_string());
    assert!(crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&stale_events).is_empty());

    println!("{}", orchestrator::PROMPT_FAILURE_CURRICULUM_SCHEMA_V1);
}

#[test]
fn live_observations_bind_workflow_and_typed_evidence_to_the_terminal_steer_epoch() {
    let scoped = |sequence: u64,
                  kind: EventKind,
                  summary: &str,
                  collaboration_id: &str,
                  steer_epoch: u64,
                  extra: Vec<(String, String)>| {
        event(
            sequence,
            kind,
            summary,
            [
                ("collaboration_id".to_string(), collaboration_id.to_string()),
                ("agent_run_id".to_string(), "run-shared".to_string()),
                ("steer_epoch".to_string(), steer_epoch.to_string()),
            ]
            .into_iter()
            .chain(extra),
        )
    };
    let events = vec![
        scoped(
            1,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile selected",
            "collab-old",
            0,
            vec![("prompt_profile".to_string(), "profile-old".to_string())],
        ),
        scoped(
            2,
            EventKind::ModelRequestFinished,
            "Model request finished",
            "collab-old",
            0,
            vec![("total_tokens".to_string(), "100".to_string())],
        ),
        scoped(
            3,
            EventKind::TaskStatusChanged,
            "Collaboration workflow completed",
            "collab-old",
            0,
            Vec::new(),
        ),
        scoped(
            4,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile selected",
            "collab-new",
            1,
            vec![("prompt_profile".to_string(), "profile-new".to_string())],
        ),
        scoped(
            5,
            EventKind::PermissionResolved,
            "Permission denied in another collaboration",
            "collab-old",
            1,
            vec![("decision".to_string(), "deny".to_string())],
        ),
        scoped(
            6,
            EventKind::ModelRequestFinished,
            "Model request finished",
            "collab-new",
            1,
            vec![("total_tokens".to_string(), "20".to_string())],
        ),
        scoped(
            7,
            EventKind::TaskStatusChanged,
            "Collaboration workflow completed",
            "collab-new",
            1,
            Vec::new(),
        ),
        scoped(
            8,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            "collab-new",
            1,
            {
                let mut metadata = complete_run_lineage_with_attempts("120", "2");
                metadata.push((
                    "learning_evidence_v1".to_string(),
                    quality_evidence_at(
                        IndependentQualitySource::CollaborationQualityGate,
                        8_000,
                        true,
                        1,
                    ),
                ));
                metadata
            },
        ),
    ];

    let observations = prompt_evolution_observations_from_events(&events);

    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].1.case_id, "collab-new");
    assert_eq!(observations[0].1.profile_id, "profile-new");
    assert_eq!(observations[0].1.total_tokens, 120);

    let mut mismatched_evidence = events;
    mismatched_evidence
        .last_mut()
        .expect("terminal event")
        .metadata
        .insert(
            "learning_evidence_v1".to_string(),
            quality_evidence_at(
                IndependentQualitySource::CollaborationQualityGate,
                8_000,
                true,
                0,
            ),
        );
    let mismatched = prompt_evolution_observations_from_events(&mismatched_evidence);
    assert_eq!(mismatched.len(), 1);
    assert!(!mismatched[0].1.succeeded);
    assert!(!mismatched[0].1.format_valid);
    assert!(!mismatched[0].1.is_scientific_evidence());
}

#[test]
fn malformed_single_pairwise_observation_is_rejected() {
    let observation = PromptEvolutionObservation {
        profile_id: "offline-profile".to_string(),
        evaluation_id: "offline-evaluation".to_string(),
        case_id: "offline-case".to_string(),
        opponent_profile_id: Some("offline-opponent".to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Holdout,
        mode: PromptEvaluationMode::ReplayExecution,
        format_valid: true,
        succeeded: true,
        quality_score: 0.91,
        latency_ms: 50,
        total_tokens: 100,
        estimated_cost_microusd: 4,
        safety_violations: 0,
        relative_reward: Some(0.2),
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance: Default::default(),
    };
    let event = Event {
        id: EventId("offline-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor pairwise evaluation".to_string(),
        metadata: [
            ("prompt_effort".to_string(), "pro".to_string()),
            (
                "prompt_observation".to_string(),
                serde_json::to_string(&observation).expect("serialize observation"),
            ),
        ]
        .into_iter()
        .collect(),
    };

    assert!(prompt_evolution_observations_from_events(&[event]).is_empty());
}

#[test]
fn exact_observation_replay_is_idempotent_and_incremental_projection_preserves_conflicts() {
    let project_id = "project-a";
    let evaluation_id = scoped_prompt_evaluation_id(project_id, "conflicting-replay");
    let candidate_sha256 = sha256_hex(b"candidate-prompt");
    let stable_sha256 = sha256_hex(b"stable-prompt");
    let observations = |candidate_quality: f64| {
        let candidate = PromptEvolutionObservation {
            profile_id: "candidate".to_string(),
            evaluation_id: evaluation_id.clone(),
            case_id: "holdout-case".to_string(),
            opponent_profile_id: Some("stable".to_string()),
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Holdout,
            mode: PromptEvaluationMode::ReplayExecution,
            format_valid: true,
            succeeded: true,
            quality_score: candidate_quality,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.2),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: PromptEvaluationProvenance::blind_pairwise_swap(
                vec!["reviewer".to_string()],
                vec!["worker".to_string()],
                "d".repeat(64),
                candidate_sha256.clone(),
                stable_sha256.clone(),
            ),
        };
        let mut stable = PromptEvolutionObservation {
            profile_id: "stable".to_string(),
            opponent_profile_id: Some("candidate".to_string()),
            quality_score: 0.8,
            relative_reward: Some(-0.2),
            ..candidate.clone()
        };
        stable.provenance = PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["reviewer".to_string()],
            vec!["worker".to_string()],
            "d".repeat(64),
            stable_sha256.clone(),
            candidate_sha256.clone(),
        );
        vec![candidate, stable]
    };
    let pair_event = |sequence: u64, candidate_quality: f64| {
        event(
            sequence,
            EventKind::TaskStatusChanged,
            "Conductor pairwise evaluation",
            [
                ("project_id".to_string(), project_id.to_string()),
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_observations".to_string(),
                    serde_json::to_string(&observations(candidate_quality)).unwrap(),
                ),
            ],
        )
    };
    let original = pair_event(1, 0.9);
    let exact_replay = pair_event(2, 0.9);
    let conflicting_replay = pair_event(3, 0.1);

    let exact = build_prompt_evolution_read_model(&[original.clone(), exact_replay.clone()], 2, 2);
    assert_eq!(exact.observations.len(), 2);

    let rebuilt = build_prompt_evolution_read_model(
        &[
            original.clone(),
            exact_replay.clone(),
            conflicting_replay.clone(),
        ],
        3,
        3,
    );
    assert_eq!(rebuilt.observations.len(), 3);

    let mut incremental = build_prompt_evolution_read_model(&[original], 1, 1);
    for replay in [&exact_replay, &conflicting_replay] {
        for (effort, observation) in
            prompt_observation_records_from_event_with_canonical_teacher(replay, None)
        {
            upsert_prompt_observation(&mut incremental.observations, effort, observation);
        }
    }
    assert_eq!(incremental.observations, rebuilt.observations);
}

#[test]
fn auto_transfer_event_enters_only_its_scoped_read_model() {
    let project_id = "project-a";
    let evaluation_id = scoped_prompt_evaluation_id(project_id, "auto-transfer-1");
    let pro_profile = "pro-candidate";
    let auto_profile = "auto-stable";
    let pro_sha256 = sha256_hex(pro_profile.as_bytes());
    let auto_sha256 = sha256_hex(auto_profile.as_bytes());
    let dataset = PromptDatasetIdentityV1::new(
        project_id,
        0,
        vec![PromptDatasetCaseIdentityV1 {
            case_id: "runtime-coding-transfer".to_string(),
            objective_sha256: "c".repeat(64),
            task_family_sha256: "f".repeat(64),
            split: PromptEvaluationSplit::Train,
        }],
    )
    .unwrap();
    let cohort = PromptLearningCohortV1::new(
        dataset,
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
    let matched_identity = PromptMatchedEvaluationIdentityV1::new(
        evaluation_id.clone(),
        &cohort,
        "runtime-coding-transfer",
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedExecution,
    )
    .unwrap();
    let source_context = orchestrator::AutoTeacherSourceContextV1 {
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
    };
    let transfer = PromptTransferProvenance::auto_to_pro(
        "auto-source-run",
        2,
        auto_profile,
        auto_sha256.clone(),
        sha256_hex(b"verified Auto output"),
    )
    .with_source_context(&source_context)
    .unwrap();
    let observation = |profile_id: &str,
                       opponent_id: &str,
                       candidate_sha256: String,
                       opponent_sha256: String,
                       relative_reward: f64| {
        PromptEvolutionObservation {
            profile_id: profile_id.to_string(),
            evaluation_id: evaluation_id.clone(),
            case_id: "runtime-coding-transfer".to_string(),
            opponent_profile_id: Some(opponent_id.to_string()),
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::PairedExecution,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 50,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(relative_reward),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: PromptEvaluationProvenance::blind_pairwise_swap(
                vec!["independent-judge".to_string()],
                vec!["pro-worker".to_string(), "auto-worker".to_string()],
                cohort.dataset.dataset_sha256.clone(),
                candidate_sha256,
                opponent_sha256,
            )
            .with_transfer(transfer.clone())
            .with_matched_evaluation(matched_identity.clone()),
        }
    };
    let observations = vec![
        observation(
            pro_profile,
            auto_profile,
            pro_sha256.clone(),
            auto_sha256.clone(),
            0.2,
        ),
        observation(
            auto_profile,
            pro_profile,
            auto_sha256.clone(),
            pro_sha256.clone(),
            -0.2,
        ),
    ];
    let transfer_event = Event {
        id: EventId("auto-transfer-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor Auto transfer evaluation".to_string(),
        metadata: [
            ("project_id".to_string(), project_id.to_string()),
            ("prompt_effort".to_string(), "pro".to_string()),
            ("evaluation_id".to_string(), evaluation_id),
            ("auto_teacher_profile".to_string(), auto_profile.to_string()),
            (
                "auto_teacher_run_id".to_string(),
                "auto-source-run".to_string(),
            ),
            (
                "prompt_observations".to_string(),
                serde_json::to_string(&observations).expect("serialize transfer observations"),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let mut model = build_prompt_evolution_read_model(std::slice::from_ref(&transfer_event), 1, 1);
    assert!(
        model.observations.is_empty(),
        "a transfer event cannot attest its own canonical Auto source"
    );
    model.observations = prompt_observation_records_from_event_with_canonical_teacher(
        &transfer_event,
        Some(CanonicalPromptAutoTeacher {
            source_run_id: "auto-source-run",
            steer_epoch: 2,
            profile_id: auto_profile,
            profile_sha256: &auto_sha256,
            output_sha256: &sha256_hex(b"verified Auto output"),
            source_context: &source_context,
        }),
    );
    assert_eq!(model.observations.len(), 2);
    assert!(prompt_evolution_read_model_for_scope(&model, project_id)
        .observations
        .is_empty());
    let started = PromptEvaluationAttemptEventV1::started(
        matched_identity,
        &cohort,
        [
            PromptTreatmentIdentityV1 {
                profile_id: pro_profile.to_string(),
                prompt_sha256: pro_sha256.clone(),
            },
            PromptTreatmentIdentityV1 {
                profile_id: auto_profile.to_string(),
                prompt_sha256: sha256_hex(auto_profile.as_bytes()),
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
    model.cohorts.insert(cohort.cohort_sha256.clone(), cohort);
    model.attempts.insert(
        started.identity.evaluation_id.clone(),
        PromptEvaluationAttemptState {
            started,
            terminal: Some(terminal),
        },
    );
    let project_a = prompt_evolution_read_model_for_scope(&model, project_id);
    let project_b = prompt_evolution_read_model_for_scope(&model, "project-b");
    assert_eq!(project_a.observations.len(), 2);
    assert!(project_a
        .observations
        .iter()
        .all(|(_, observation)| observation.is_strict_matched_transfer_evidence()));
    assert!(project_b.observations.is_empty());

    let mut mismatched = transfer_event;
    mismatched.metadata.insert(
        "auto_teacher_run_id".to_string(),
        "different-run".to_string(),
    );
    assert!(prompt_observation_records_from_event(&mismatched).is_empty());
}

#[test]
fn learned_prompt_genomes_are_isolated_by_project_scope() {
    let mut project_a = ConductorPromptGenome::seed_for_effort("auto")
        .mutations()
        .into_iter()
        .next()
        .expect("seed should provide a candidate");
    project_a.custom_directive = "project-a evidence".to_string();
    let mut project_b = project_a.clone();
    project_b.custom_directive = "project-b evidence".to_string();
    let mutation_event = |sequence: u64, project_id: &str, genome: &ConductorPromptGenome| {
        event(
            sequence,
            EventKind::TaskStatusChanged,
            "Conductor prompt mutation generated",
            [
                ("project_id".to_string(), project_id.to_string()),
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_genome".to_string(),
                    serde_json::to_string(genome).expect("serialize genome"),
                ),
                (
                    "mutation_strategy".to_string(),
                    "gepa_reflection".to_string(),
                ),
            ],
        )
    };
    let model = build_prompt_evolution_read_model(
        &[
            mutation_event(1, "project-a", &project_a),
            mutation_event(2, "project-b", &project_b),
        ],
        2,
        2,
    );

    assert_eq!(model.genomes.len(), 2);
    let scoped_a = prompt_evolution_read_model_for_scope(&model, "project-a");
    let scoped_b = prompt_evolution_read_model_for_scope(&model, "project-b");
    assert_eq!(scoped_a.genomes.len(), 1);
    assert_eq!(scoped_b.genomes.len(), 1);
    assert_eq!(
        scoped_a.genomes[0].genome.custom_directive,
        "project-a evidence"
    );
    assert_eq!(
        scoped_b.genomes[0].genome.custom_directive,
        "project-b evidence"
    );
}

#[test]
fn legacy_unscoped_prompt_genomes_force_a_read_model_rebuild() {
    let model = PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        projection_version: PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
        revision: 1,
        event_count: 1,
        genomes: vec![PromptGenomeRecord {
            scope: String::new(),
            effort: "auto".to_string(),
            genome: ConductorPromptGenome::seed_for_effort("auto"),
            evolution_method: None,
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

    assert!(!prompt_genome_scopes_are_valid(&model));
}

#[test]
fn prior_projection_version_replays_canonical_events_without_a_delta() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let genome = ConductorPromptGenome::seed_for_effort("auto");
    store
        .append(event(
            1,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile indexed",
            [
                ("project_id".to_string(), "project-a".to_string()),
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_genome".to_string(),
                    serde_json::to_string(&genome).expect("serialize genome"),
                ),
            ],
        ))
        .expect("canonical event should append");
    let stale = PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        projection_version: PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION - 1,
        revision: 1,
        event_count: 1,
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
    store
        .save_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
            stale.revision,
            &serde_json::to_string(&stale).expect("serialize stale projection"),
        )
        .expect("stale projection should persist");

    let rebuilt = load_prompt_evolution_read_model(&mut store)
        .expect("old projection version should replay canonical events");

    assert_eq!(
        rebuilt.projection_version,
        PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION
    );
    assert_eq!(rebuilt.genomes.len(), 1);
    assert_eq!(rebuilt.genomes[0].genome.id, genome.id);
}

#[test]
fn prompt_rollout_replay_rejects_forged_stable_canary_and_status() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let candidate = seed
        .mutations()
        .into_iter()
        .next()
        .expect("Auto seed should provide a canary");
    let snapshot_only_canary =
        prompt_rollout_event(2, "canary", &seed.id, Some(&candidate.id), 10, 0, &[], None);
    let events = vec![
        prompt_rollout_event(1, "stable", "forged-stable", None, 0, 0, &[], None),
        snapshot_only_canary,
        prompt_rollout_event(
            3,
            "canary",
            "forged-stable",
            Some(&candidate.id),
            20,
            0,
            &[],
            None,
        ),
        prompt_rollout_event(4, "canary", &seed.id, Some(&candidate.id), 51, 0, &[], None),
        prompt_rollout_event(5, "stable", &seed.id, Some(&candidate.id), 10, 0, &[], None),
        prompt_rollout_event(6, "future", &seed.id, None, 0, 0, &[], None),
    ];

    let model = build_prompt_evolution_read_model(&events, 6, events.len() as u64);
    assert!(model.rollouts.is_empty());
}

#[test]
fn prompt_rollout_transition_accepts_exact_stages_and_atomic_promotion() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let candidate = seed
        .mutations()
        .into_iter()
        .next()
        .expect("Auto seed should provide a canary");
    let frozen = FrozenPromptProfileSnapshot::new_gepa(
        "auto",
        candidate.clone(),
        seed.id.clone(),
        "a".repeat(64),
        "b".repeat(64),
    )
    .expect("promotion snapshot should be valid");
    let events = vec![
        prompt_rollout_event(1, "canary", &seed.id, Some(&candidate.id), 10, 0, &[], None),
        prompt_rollout_event(2, "canary", &seed.id, Some(&candidate.id), 25, 0, &[], None),
        prompt_rollout_event(3, "canary", &seed.id, Some(&candidate.id), 50, 0, &[], None),
        prompt_rollout_event(4, "promoted", &candidate.id, None, 0, 0, &[], Some(&frozen)),
        prompt_rollout_event(5, "stable", &candidate.id, None, 0, 0, &[], Some(&frozen)),
    ];

    let mut previous = None;
    for event in &events {
        let (_, effort, rollout) = prompt_rollout_record_from_event(event).unwrap();
        assert!(prompt_rollout_transition_is_valid(
            &effort,
            previous.as_ref(),
            &rollout
        ));
        previous = Some(rollout);
    }
    let rollout = previous.expect("legal rollout history should remain structurally valid");
    assert_eq!(rollout.status, "stable");
    assert_eq!(rollout.stable_profile_id, candidate.id);
    assert_eq!(rollout.rollback_count, 0);
    assert_eq!(rollout.frozen_profile.as_ref(), Some(&frozen));
}

#[test]
fn prompt_rollout_quarantine_blocks_a_after_a_then_b_roll_back() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let candidates = seed.mutations();
    let candidate_a = &candidates[0].id;
    let candidate_b = &candidates[1].id;
    let events = vec![
        with_distillation_lease(
            prompt_rollout_event(1, "canary", &seed.id, Some(candidate_a), 10, 0, &[], None),
            &seed.id,
            candidate_a,
        ),
        prompt_rollout_event(2, "rolled_back", &seed.id, None, 0, 1, &[candidate_a], None),
        with_distillation_lease(
            prompt_rollout_event(
                3,
                "canary",
                &seed.id,
                Some(candidate_b),
                10,
                1,
                &[candidate_a],
                None,
            ),
            &seed.id,
            candidate_b,
        ),
        prompt_rollout_event(
            4,
            "rolled_back",
            &seed.id,
            None,
            0,
            2,
            &[candidate_a, candidate_b],
            None,
        ),
        with_distillation_lease(
            prompt_rollout_event(
                5,
                "canary",
                &seed.id,
                Some(candidate_a),
                10,
                2,
                &[candidate_a, candidate_b],
                None,
            ),
            &seed.id,
            candidate_a,
        ),
    ];

    let mut previous = None;
    for event in &events[..4] {
        let (_, effort, rollout) = prompt_rollout_record_from_event(event).unwrap();
        assert!(prompt_rollout_transition_is_valid(
            &effort,
            previous.as_ref(),
            &rollout
        ));
        previous = Some(rollout);
    }
    let (_, effort, reentry) = prompt_rollout_record_from_event(&events[4]).unwrap();
    assert!(!prompt_rollout_transition_is_valid(
        &effort,
        previous.as_ref(),
        &reentry
    ));

    let rollout = previous.unwrap();
    assert_eq!(
        rollout.quarantined_profile_ids,
        vec![candidate_a.clone(), candidate_b.clone()]
    );
}

#[test]
fn prompt_rollout_replay_rejects_snapshot_only_initial_promotion() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let candidate = seed
        .mutations()
        .into_iter()
        .next()
        .expect("Auto seed should provide a promoted profile");
    let frozen = FrozenPromptProfileSnapshot::new_gepa(
        "auto",
        candidate.clone(),
        seed.id.clone(),
        "c".repeat(64),
        "d".repeat(64),
    )
    .expect("promotion snapshot should be valid");
    let snapshot_only =
        prompt_rollout_event(1, "promoted", &candidate.id, None, 0, 0, &[], Some(&frozen));
    let mut wrong_anchor = frozen.clone();
    wrong_anchor.stable_profile_id = "forged-previous-stable".to_string();
    let invalid = prompt_rollout_event(
        1,
        "promoted",
        &candidate.id,
        None,
        0,
        0,
        &[],
        Some(&wrong_anchor),
    );

    let rejected_snapshot = build_prompt_evolution_read_model(&[snapshot_only], 1, 1);
    let rejected = build_prompt_evolution_read_model(&[invalid], 1, 1);

    assert!(rejected_snapshot.rollouts.is_empty());
    assert!(rejected.rollouts.is_empty());
}

#[test]
fn matched_attempt_lifecycle_persists_one_terminal_and_keeps_failures_in_the_read_model() {
    let dataset = PromptDatasetIdentityV1::new(
        "project-a",
        1,
        vec![PromptDatasetCaseIdentityV1 {
            case_id: "case-a".to_string(),
            objective_sha256: "a".repeat(64),
            task_family_sha256: "b".repeat(64),
            split: PromptEvaluationSplit::Train,
        }],
    )
    .unwrap();
    let context = PromptExecutionContextV1 {
        schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
        provider_sha256: "1".repeat(64),
        model_pool_sha256: "2".repeat(64),
        harness_sha256: "3".repeat(64),
        system_prompt_sha256: "4".repeat(64),
        policy: AgentPolicy::Auto,
        policy_sha256: "5".repeat(64),
        budget_sha256: "6".repeat(64),
        tool_contract_sha256: "7".repeat(64),
        source_revision_sha256: "8".repeat(64),
        workspace_revision_sha256: "9".repeat(64),
    };
    let cohort = PromptLearningCohortV1::new(dataset, context).unwrap();
    let evaluation_id = scoped_prompt_evaluation_id("project-a", "attempt-a");
    let identity = PromptMatchedEvaluationIdentityV1::new(
        evaluation_id.clone(),
        &cohort,
        "case-a",
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedExecution,
    )
    .unwrap();
    let started = PromptEvaluationAttemptEventV1::started(
        identity,
        &cohort,
        [
            PromptTreatmentIdentityV1 {
                profile_id: "current".to_string(),
                prompt_sha256: "c".repeat(64),
            },
            PromptTreatmentIdentityV1 {
                profile_id: "challenger".to_string(),
                prompt_sha256: "d".repeat(64),
            },
        ],
    )
    .unwrap();
    let failed = PromptEvaluationAttemptEventV1::terminal(
        &started,
        PromptEvaluationAttemptStatus::TreatmentFailure,
        [true, false],
        "treatment_execution_failed",
    )
    .unwrap();
    let invalid_duplicate = PromptEvaluationAttemptEventV1::terminal(
        &started,
        PromptEvaluationAttemptStatus::InfrastructureInvalid,
        [false; 2],
        "late_duplicate",
    )
    .unwrap();
    let attempt_event = |sequence: u64, attempt: &PromptEvaluationAttemptEventV1| Event {
        id: EventId(format!("attempt-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence,
        kind: EventKind::TaskStatusChanged,
        summary: PROMPT_EVALUATION_ATTEMPT_EVENT.to_string(),
        metadata: [
            ("project_id".to_string(), "project-a".to_string()),
            ("prompt_evaluation_id".to_string(), evaluation_id.clone()),
            (
                PROMPT_EVALUATION_ATTEMPT_METADATA_KEY.to_string(),
                serde_json::to_string(attempt).unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let started_event = attempt_event(1, &started);
    let mut model = build_prompt_evolution_read_model(
        &[
            started_event.clone(),
            attempt_event(2, &failed),
            attempt_event(3, &invalid_duplicate),
        ],
        3,
        3,
    );
    model
        .cohorts
        .insert(cohort.cohort_sha256.clone(), cohort.clone());

    let state = model.attempts.get(&evaluation_id).unwrap();
    assert_eq!(state.started.status, PromptEvaluationAttemptStatus::Started);
    assert_eq!(
        state.terminal.as_ref().map(|event| event.status),
        Some(PromptEvaluationAttemptStatus::TreatmentFailure)
    );
    assert!(state
        .terminal
        .as_ref()
        .is_some_and(PromptEvaluationAttemptEventV1::enters_effect_denominator));

    let observation = PromptEvolutionObservation {
        profile_id: "current".to_string(),
        evaluation_id: evaluation_id.clone(),
        case_id: "case-a".to_string(),
        opponent_profile_id: Some("challenger".to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Train,
        mode: PromptEvaluationMode::PairedExecution,
        format_valid: true,
        succeeded: false,
        quality_score: 0.0,
        latency_ms: 1,
        total_tokens: 1,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(-1.0),
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance: PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["reviewer".to_string()],
            vec!["worker".to_string()],
            started.identity.dataset_sha256.clone(),
            "c".repeat(64),
            "d".repeat(64),
        )
        .with_matched_evaluation(started.identity.clone()),
    };
    model
        .observations
        .push(("auto".to_string(), observation.clone()));
    assert!(prompt_evolution_read_model_for_scope(&model, "project-a")
        .observations
        .is_empty());

    let mut missing_terminal = build_prompt_evolution_read_model(&[started_event], 1, 1);
    missing_terminal
        .cohorts
        .insert(cohort.cohort_sha256.clone(), cohort);
    missing_terminal
        .observations
        .push(("auto".to_string(), observation));
    assert!(
        prompt_evolution_read_model_for_scope(&missing_terminal, "project-a")
            .observations
            .is_empty()
    );
}

#[test]
fn scoped_read_model_never_falls_back_to_a_global_rollout() {
    let rollout = |stable_profile_id: &str| PromptRolloutState {
        stable_profile_id: stable_profile_id.to_string(),
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
        promotion_confidence: None,
        frozen_profile: None,
    };
    let mut model = build_prompt_evolution_read_model(&[], 0, 0);
    model
        .rollouts
        .insert("auto".to_string(), rollout("global-stable"));
    model.rollouts.insert(
        prompt_rollout_key("project-a", "auto"),
        rollout("project-a-stable"),
    );

    let project_a = prompt_evolution_read_model_for_scope(&model, "project-a");
    let project_b = prompt_evolution_read_model_for_scope(&model, "project-b");
    let global = prompt_evolution_read_model_for_scope(&model, "global");

    assert_eq!(
        project_a
            .rollouts
            .get("auto")
            .map(|rollout| rollout.stable_profile_id.as_str()),
        Some("project-a-stable")
    );
    assert!(project_b.rollouts.is_empty());
    assert_eq!(
        global
            .rollouts
            .get("auto")
            .map(|rollout| rollout.stable_profile_id.as_str()),
        Some("global-stable")
    );
}

#[test]
fn cold_rebuild_indexes_each_run_event_once_and_limits_teacher_visits_to_its_run() {
    let events = (0..2_048)
        .map(|offset| {
            event(
                offset + 1,
                EventKind::TaskStatusChanged,
                "indexed event",
                [("agent_run_id".to_string(), format!("run-{}", offset % 128))],
            )
        })
        .collect::<Vec<_>>();
    let index = prompt_agent_run_event_index(&events);

    assert_eq!(index.len(), 128);
    assert_eq!(index.values().map(Vec::len).sum::<usize>(), events.len());
    assert!(index.values().all(|run_events| run_events.len() == 16));
    for source_run_id in ["run-0", "run-63", "run-127"] {
        let indexed =
            crate::prompt_evidence_runtime::reconstruct_prompt_auto_teacher_from_indexed_events(
                index
                    .get(source_run_id)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                "project-a",
                source_run_id,
            );
        let scanned =
            crate::prompt_evidence_runtime::reconstruct_prompt_auto_teacher_from_canonical_events(
                &events,
                "project-a",
                source_run_id,
            );
        assert_eq!(indexed, scanned);
    }
}

#[test]
fn hot_state_compaction_preserves_valid_and_unfinished_references() {
    let cohort = |case_id: &str, marker: char| {
        let dataset = PromptDatasetIdentityV1::new(
            "project-a",
            1,
            vec![PromptDatasetCaseIdentityV1 {
                case_id: case_id.to_string(),
                objective_sha256: marker.to_string().repeat(64),
                task_family_sha256: "f".repeat(64),
                split: PromptEvaluationSplit::Train,
            }],
        )
        .unwrap();
        PromptLearningCohortV1::new(
            dataset,
            PromptExecutionContextV1 {
                schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
                provider_sha256: marker.to_string().repeat(64),
                model_pool_sha256: "2".repeat(64),
                harness_sha256: "3".repeat(64),
                system_prompt_sha256: "4".repeat(64),
                policy: AgentPolicy::Auto,
                policy_sha256: "5".repeat(64),
                budget_sha256: "6".repeat(64),
                tool_contract_sha256: "7".repeat(64),
                source_revision_sha256: "8".repeat(64),
                workspace_revision_sha256: "9".repeat(64),
            },
        )
        .unwrap()
    };
    let referenced_cohort = cohort("case-referenced", 'a');
    let second_referenced_cohort = cohort("case-second-referenced", 'f');
    let orphan_attempt_cohort = cohort("case-orphan-attempt", 'b');
    let unfinished_cohort = cohort("case-unfinished", 'c');
    let leased_cohort = cohort("case-leased", 'd');
    let unused_cohort = cohort("case-unused", 'e');
    let attempt = |cohort: &PromptLearningCohortV1,
                   suffix: &str,
                   terminal: Option<PromptEvaluationAttemptStatus>| {
        let identity = PromptMatchedEvaluationIdentityV1::new(
            scoped_prompt_evaluation_id("project-a", suffix),
            cohort,
            cohort.dataset.cases[0].case_id.clone(),
            PromptEvaluationSplit::Train,
            PromptEvaluationMode::PairedExecution,
        )
        .unwrap();
        let started = PromptEvaluationAttemptEventV1::started(
            identity,
            cohort,
            [
                PromptTreatmentIdentityV1 {
                    profile_id: format!("{suffix}-candidate"),
                    prompt_sha256: "a".repeat(64),
                },
                PromptTreatmentIdentityV1 {
                    profile_id: format!("{suffix}-stable"),
                    prompt_sha256: "b".repeat(64),
                },
            ],
        )
        .unwrap();
        let terminal = terminal.map(|status| {
            let (failures, reason) = match status {
                PromptEvaluationAttemptStatus::CompletedPair => ([false; 2], ""),
                PromptEvaluationAttemptStatus::TreatmentFailure => {
                    ([true, false], "treatment_failed")
                }
                _ => ([false; 2], "invalid"),
            };
            PromptEvaluationAttemptEventV1::terminal(&started, status, failures, reason).unwrap()
        });
        PromptEvaluationAttemptState { started, terminal }
    };
    let valid = attempt(
        &referenced_cohort,
        "valid",
        Some(PromptEvaluationAttemptStatus::CompletedPair),
    );
    let second_valid = attempt(
        &second_referenced_cohort,
        "second-valid",
        Some(PromptEvaluationAttemptStatus::CompletedPair),
    );
    let orphan = attempt(
        &orphan_attempt_cohort,
        "orphan",
        Some(PromptEvaluationAttemptStatus::TreatmentFailure),
    );
    let unfinished = attempt(&unfinished_cohort, "unfinished", None);
    let valid_id = valid.started.identity.evaluation_id.clone();
    let second_valid_id = second_valid.started.identity.evaluation_id.clone();
    let orphan_id = orphan.started.identity.evaluation_id.clone();
    let unfinished_id = unfinished.started.identity.evaluation_id.clone();
    let mut model = build_prompt_evolution_read_model(&[], 0, 0);
    model.attempts.insert(valid_id.clone(), valid);
    model.attempts.insert(second_valid_id.clone(), second_valid);
    model.attempts.insert(orphan_id.clone(), orphan);
    model.attempts.insert(unfinished_id.clone(), unfinished);
    for (sequence, cohort) in [
        (4, referenced_cohort.clone()),
        (3, second_referenced_cohort.clone()),
        (2, leased_cohort.clone()),
        (1, unused_cohort.clone()),
    ] {
        model
            .cohort_sequences
            .insert(cohort.cohort_sha256.clone(), sequence);
        model.cohorts.insert(cohort.cohort_sha256.clone(), cohort);
    }
    model.rollouts.insert(
        prompt_rollout_key("project-a", "auto"),
        PromptRolloutState {
            stable_profile_id: "stable".to_string(),
            canary_profile_id: Some("candidate".to_string()),
            canary_percent: 10,
            evidence_checkpoint: 0,
            live_checkpoint: 0,
            stable_live_checkpoint: 0,
            quarantined_profile_ids: Vec::new(),
            distillation_lease: Some(PromptDistillationCanaryLeaseV1 {
                schema: PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1.to_string(),
                candidate_profile_id: "candidate".to_string(),
                candidate_profile_sha256: "1".repeat(64),
                stable_profile_id: "stable".to_string(),
                stable_profile_sha256: "2".repeat(64),
                cohort_sha256: leased_cohort.cohort_sha256.clone(),
                paired_evidence_sha256: "3".repeat(64),
            }),
            rollback_count: 0,
            status: "canary".to_string(),
            last_reason: None,
            promotion_confidence: None,
            frozen_profile: None,
        },
    );

    compact_prompt_learning_hot_state_to_limits(&mut model, 2, 2);

    assert!(model.attempts.contains_key(&valid_id));
    assert!(model.attempts.contains_key(&second_valid_id));
    assert!(model.attempts.contains_key(&unfinished_id));
    assert!(!model.attempts.contains_key(&orphan_id));
    assert!(model.cohorts.contains_key(&referenced_cohort.cohort_sha256));
    assert!(model
        .cohorts
        .contains_key(&second_referenced_cohort.cohort_sha256));
    assert!(model.cohorts.contains_key(&leased_cohort.cohort_sha256));
    assert!(!model.cohorts.contains_key(&unused_cohort.cohort_sha256));
}

#[test]
fn conflicting_genome_identity_tombstone_persists_and_fails_closed() {
    let generation_one = ConductorPromptGenome::seed_for_effort("auto")
        .mutations()
        .into_iter()
        .next()
        .unwrap();
    let mut first = generation_one.mutations().into_iter().next().unwrap();
    first.custom_directive = "first payload".to_string();
    let mut conflicting = first.clone();
    conflicting.custom_directive = "conflicting payload".to_string();
    let mutation = |sequence: u64, genome: &ConductorPromptGenome| {
        event(
            sequence,
            EventKind::TaskStatusChanged,
            "Conductor prompt mutation generated",
            [
                ("project_id".to_string(), "project-a".to_string()),
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_genome".to_string(),
                    serde_json::to_string(genome).unwrap(),
                ),
                (
                    "mutation_strategy".to_string(),
                    "gepa_reflection".to_string(),
                ),
            ],
        )
    };
    let mut model = build_prompt_evolution_read_model(
        &[
            mutation(1, &first),
            mutation(2, &first),
            mutation(3, &conflicting),
        ],
        3,
        3,
    );
    model.rollouts.insert(
        prompt_rollout_key("project-a", "auto"),
        PromptRolloutState {
            stable_profile_id: first.id.clone(),
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
            promotion_confidence: None,
            frozen_profile: None,
        },
    );

    assert!(model.genomes.is_empty());
    assert_eq!(
        model
            .genome_identity_fingerprints
            .values()
            .filter(|fingerprint| fingerprint.as_str() == "conflict")
            .count(),
        1
    );
    let encoded = serde_json::to_string(&model).unwrap();
    let restored = serde_json::from_str::<PromptEvolutionReadModel>(&encoded).unwrap();
    let scoped = prompt_evolution_read_model_for_scope(&restored, "project-a");
    let candidates = prompt_genomes_from_events(&prompt_evolution_profile_events(&scoped), "auto");

    assert!(scoped.genomes.is_empty());
    assert!(scoped.rollouts.is_empty());
    assert!(candidates.iter().all(|genome| genome.id != first.id));
    assert!(visible_prompt_rollout(&restored, "auto").is_none());
}

#[test]
fn evicted_genome_identity_detects_later_conflict_like_cold_replay() {
    let generation_one = ConductorPromptGenome::seed_for_effort("auto")
        .mutations()
        .into_iter()
        .next()
        .unwrap();
    let mut genomes = generation_one.mutations().into_iter();
    let mut first = genomes.next().unwrap();
    first.custom_directive = "first payload".to_string();
    let filler_one = genomes.next().unwrap();
    let filler_two = genomes.next().unwrap();
    let mut conflicting = first.clone();
    conflicting.custom_directive = "conflicting payload".to_string();
    let mutation = |sequence: u64, genome: &ConductorPromptGenome| {
        event(
            sequence,
            EventKind::TaskStatusChanged,
            "Conductor prompt mutation generated",
            [
                ("project_id".to_string(), "project-a".to_string()),
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_genome".to_string(),
                    serde_json::to_string(genome).unwrap(),
                ),
                (
                    "mutation_strategy".to_string(),
                    "gepa_reflection".to_string(),
                ),
            ],
        )
    };
    let events = vec![
        mutation(1, &first),
        mutation(2, &filler_one),
        mutation(3, &filler_two),
        mutation(4, &first),
        mutation(5, &conflicting),
    ];

    let mut incremental = build_prompt_evolution_read_model(&events[..3], 3, 3);
    compact_prompt_evolution_hot_state_to_limits(
        &mut incremental,
        usize::MAX,
        usize::MAX,
        1,
        usize::MAX,
    );
    assert!(incremental
        .genomes
        .iter()
        .all(|record| record.genome.id != first.id));
    for event in &events[3..] {
        for record in prompt_genome_records_from_event(event) {
            upsert_prompt_genome(&mut incremental, record);
        }
        compact_prompt_evolution_hot_state_to_limits(
            &mut incremental,
            usize::MAX,
            usize::MAX,
            1,
            usize::MAX,
        );
    }
    incremental.revision = 5;
    incremental.event_count = 5;

    let mut cold = build_prompt_evolution_read_model(&events, 5, 5);
    compact_prompt_evolution_hot_state_to_limits(
        &mut cold,
        usize::MAX,
        usize::MAX,
        1,
        usize::MAX,
    );

    assert_eq!(
        serde_json::to_string(&incremental.genomes).unwrap(),
        serde_json::to_string(&cold.genomes).unwrap()
    );
    assert_eq!(
        incremental.genome_identity_fingerprints,
        cold.genome_identity_fingerprints
    );
    let scoped = prompt_evolution_read_model_for_scope(&incremental, "project-a");
    let candidates = prompt_genomes_from_events(&prompt_evolution_profile_events(&scoped), "auto");
    assert!(candidates.iter().all(|genome| genome.id != first.id));
}

#[test]
fn hot_state_soft_caps_are_reference_safe_and_incrementally_deterministic() {
    let genomes = ConductorPromptGenome::seed_for_effort("auto")
        .mutations()
        .into_iter()
        .take(6)
        .map(|genome| PromptGenomeRecord {
            scope: "project-a".to_string(),
            effort: "auto".to_string(),
            genome,
            evolution_method: Some(PromptEvolutionMethod::GepaReflectivePaired),
        })
        .collect::<Vec<_>>();
    let active_profile = genomes[0].genome.id.clone();
    let observation = |index: usize, profile_id: &str| PromptEvolutionObservation {
        profile_id: profile_id.to_string(),
        evaluation_id: scoped_prompt_evaluation_id("project-a", &format!("live-{index}")),
        case_id: format!("case-{index}"),
        opponent_profile_id: None,
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Train,
        mode: PromptEvaluationMode::Live,
        format_valid: true,
        succeeded: true,
        quality_score: 0.8,
        latency_ms: 1,
        total_tokens: 1,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: None,
        step_credits: Vec::new(),
        reflection_packet: None,
        provenance: PromptEvaluationProvenance::default(),
    };
    let observations = (0..8)
        .map(|index| {
            let profile = if index % 2 == 0 {
                active_profile.as_str()
            } else {
                "inactive-profile"
            };
            ("auto".to_string(), observation(index, profile))
        })
        .collect::<Vec<_>>();
    let rollout = PromptRolloutState {
        stable_profile_id: active_profile.clone(),
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
        promotion_confidence: None,
        frozen_profile: None,
    };
    let mut rebuilt = build_prompt_evolution_read_model(&[], 0, 0);
    rebuilt.genomes = genomes.clone();
    rebuilt.observations = observations.clone();
    rebuilt
        .rollouts
        .insert(prompt_rollout_key("project-a", "auto"), rollout.clone());
    compact_prompt_evolution_hot_state_to_limits(&mut rebuilt, usize::MAX, usize::MAX, 2, 2);

    let mut incremental = build_prompt_evolution_read_model(&[], 0, 0);
    incremental
        .rollouts
        .insert(prompt_rollout_key("project-a", "auto"), rollout);
    for index in 0..observations.len() {
        if let Some(genome) = genomes.get(index) {
            incremental.genomes.push(genome.clone());
        }
        incremental.observations.push(observations[index].clone());
        compact_prompt_evolution_hot_state_to_limits(
            &mut incremental,
            usize::MAX,
            usize::MAX,
            2,
            2,
        );
    }

    assert_eq!(
        rebuilt
            .observations
            .iter()
            .filter(|(_, observation)| observation.profile_id == active_profile)
            .count(),
        4,
        "active rollout references are allowed to exceed the soft cap"
    );
    assert_eq!(rebuilt.observations.len(), 6);
    assert_eq!(rebuilt.genomes.len(), 3);
    assert!(rebuilt
        .genomes
        .iter()
        .any(|record| record.genome.id == active_profile));
    assert_eq!(rebuilt.observations, incremental.observations);
    assert_eq!(
        serde_json::to_string(&rebuilt.genomes).unwrap(),
        serde_json::to_string(&incremental.genomes).unwrap()
    );
}

#[test]
fn prompt_snapshot_publication_rejects_a_stale_observed_row() {
    let mut store = SqliteStore::in_memory().unwrap();
    let model = build_prompt_evolution_read_model(&[], 0, 0);
    assert!(compare_exchange_prompt_evolution_read_model(&mut store, None, &model).unwrap());
    let observed = store
        .load_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
        )
        .unwrap()
        .unwrap();
    assert!(store
        .compare_exchange_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
            Some(&observed),
            0,
            "competing-payload",
        )
        .unwrap());

    assert!(
        !compare_exchange_prompt_evolution_read_model(&mut store, Some(&observed), &model,)
            .unwrap()
    );
}
