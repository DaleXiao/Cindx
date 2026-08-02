use super::*;
use agent_core::{EventTypeV1, EVENT_TYPE_METADATA_KEY};
use orchestrator::{LearningAttribution, LearningTermination};

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

#[test]
fn verified_postcondition_does_not_train_a_workflow_prompt() {
    let evidence = LearningEvidenceV1::verified_postcondition(
        LearningUsageCompleteness::Complete,
        0,
        "b".repeat(64),
    )
    .to_metadata_value()
    .expect("valid learning evidence");
    let observations = prompt_evolution_observations_from_events(&live_events(
        "Agent task completed",
        Some(evidence),
        Some("8"),
        false,
        Some("2500"),
    ));

    assert!(observations.is_empty());
}

#[test]
fn live_observations_fail_closed_for_untrusted_or_incomplete_runs() {
    assert!(prompt_evolution_observations_from_events(&live_events(
        "Agent task completed",
        None,
        Some("20"),
        false,
        None,
    ))
    .is_empty());

    let cancelled = LearningEvidenceV1::censored(
        LearningTermination::Cancelled,
        LearningAttribution::User,
        LearningUsageCompleteness::Complete,
        Some(0),
        Some("c".repeat(64)),
    )
    .to_metadata_value()
    .expect("valid censored evidence");
    assert!(prompt_evolution_observations_from_events(&live_events(
        "Agent task cancelled",
        Some(cancelled),
        Some("20"),
        false,
        None,
    ))
    .is_empty());

    let trusted = quality_evidence(
        IndependentQualitySource::CollaborationQualityGate,
        8_000,
        true,
    );
    assert!(prompt_evolution_observations_from_events(&live_events(
        "Agent task completed",
        Some(trusted.clone()),
        None,
        false,
        None,
    ))
    .is_empty());
    assert!(prompt_evolution_observations_from_events(&live_events(
        "Agent task completed",
        Some(trusted),
        Some("20"),
        true,
        None,
    ))
    .is_empty());

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
    assert!(prompt_evolution_observations_from_events(&inconsistent_usage).is_empty());

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
    assert!(prompt_evolution_observations_from_events(&live_events(
        "Agent task completed",
        Some(direct_evidence),
        Some("20"),
        false,
        None,
    ))
    .is_empty());
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
    assert!(prompt_evolution_observations_from_events(&mismatched_evidence).is_empty());
}

#[test]
fn explicit_pairwise_evaluation_observations_remain_unchanged() {
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

    assert_eq!(
        prompt_evolution_observations_from_events(&[event]),
        vec![("pro".to_string(), observation)]
    );
}

#[test]
fn auto_transfer_event_enters_only_its_scoped_read_model() {
    let project_id = "project-a";
    let evaluation_id = scoped_prompt_evaluation_id(project_id, "auto-transfer-1");
    let pro_profile = "pro-candidate";
    let auto_profile = "auto-stable";
    let pro_sha256 = sha256_hex(pro_profile.as_bytes());
    let auto_sha256 = sha256_hex(auto_profile.as_bytes());
    let transfer = PromptTransferProvenance::auto_to_pro(
        "auto-source-run",
        2,
        auto_profile,
        auto_sha256.clone(),
        sha256_hex(b"verified Auto output"),
    );
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
                "d".repeat(64),
                candidate_sha256,
                opponent_sha256,
            )
            .with_transfer(transfer.clone()),
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
        observation(auto_profile, pro_profile, auto_sha256, pro_sha256, -0.2),
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

    let model = build_prompt_evolution_read_model(std::slice::from_ref(&transfer_event), 1, 1);
    let project_a = prompt_evolution_read_model_for_scope(&model, project_id);
    let project_b = prompt_evolution_read_model_for_scope(&model, "project-b");
    assert_eq!(project_a.observations.len(), 2);
    assert!(project_a
        .observations
        .iter()
        .all(|(_, observation)| observation.is_scientific_transfer_evidence()));
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
        observations: Vec::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    };

    assert!(!prompt_genome_scopes_are_valid(&model));
}
