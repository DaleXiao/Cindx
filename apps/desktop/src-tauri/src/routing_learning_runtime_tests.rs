use super::*;
use agent_core::{EventTypeV1, EVENT_TYPE_METADATA_KEY};
use orchestrator::{
    IndependentQualitySource, LearningAttribution, LearningDisposition, LearningEvidenceV1,
    LearningTermination, LearningUsageCompleteness, LearningVerification,
};

fn context(run_id: &str) -> Metadata {
    let mut metadata = [
        ("agent_run_id".to_string(), run_id.to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        ("collaboration_policy".to_string(), "single".to_string()),
        ("agent_model".to_string(), "test-model".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("routing_signature".to_string(), "coding:test".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    for (index, key) in LEARNING_BUDGET_KEYS.into_iter().enumerate() {
        metadata.insert(key.to_string(), (index + 1).to_string());
    }
    metadata
}

fn event(sequence: u64, kind: EventKind, summary: &str, metadata: Metadata) -> Event {
    Event {
        id: EventId(format!("event-{sequence}")),
        task_id: TaskId("learning-evidence-test".to_string()),
        sequence,
        timestamp_ms: sequence * 100,
        kind,
        summary: summary.to_string(),
        metadata,
    }
}

fn resource_usage_metadata(
    prefix: &str,
    total_tokens: u64,
    provider: u64,
    partial: u64,
    estimated: u64,
    unknown: u64,
) -> Metadata {
    [
        (
            format!("{prefix}_physical_model_attempts"),
            provider
                .saturating_add(partial)
                .saturating_add(estimated)
                .saturating_add(unknown)
                .to_string(),
        ),
        (format!("{prefix}_prompt_tokens"), total_tokens.to_string()),
        (format!("{prefix}_completion_tokens"), "0".to_string()),
        (format!("{prefix}_total_tokens"), total_tokens.to_string()),
        (
            format!("{prefix}_provider_usage_attempts"),
            provider.to_string(),
        ),
        (
            format!("{prefix}_partial_usage_attempts"),
            partial.to_string(),
        ),
        (
            format!("{prefix}_estimated_usage_attempts"),
            estimated.to_string(),
        ),
        (
            format!("{prefix}_unknown_usage_attempts"),
            unknown.to_string(),
        ),
    ]
    .into_iter()
    .collect()
}

fn routing_events(run_id: &str, terminal_summary: &str, terminal_extra: Metadata) -> Vec<Event> {
    let context = context(run_id);
    let mut terminal = context.clone();
    terminal.extend(resource_usage_metadata("run_segment", 120, 1, 0, 0, 0));
    terminal.extend(resource_usage_metadata("run_lineage", 120, 1, 0, 0, 0));
    terminal.extend(terminal_extra);
    let mut usage = context.clone();
    usage.insert("total_tokens".to_string(), "120".to_string());
    usage.insert("usage_source".to_string(), "provider".to_string());
    vec![
        event(
            1,
            EventKind::TaskStatusChanged,
            "Agent task started",
            context.clone(),
        ),
        event(
            2,
            EventKind::TaskStatusChanged,
            "Agent run decision selected",
            context,
        ),
        event(
            3,
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            usage,
        ),
        event(4, EventKind::TaskStatusChanged, terminal_summary, terminal),
    ]
}

#[test]
fn invalid_lifecycle_tags_do_not_create_routing_telemetry() {
    let mut future_start = routing_events("future-start", "Agent task completed", Metadata::new());
    future_start[0].metadata.insert(
        EVENT_TYPE_METADATA_KEY.to_string(),
        "cindx.event.v2/agent.run.started".to_string(),
    );
    assert!(routing_telemetry_from_events(&future_start).is_empty());

    let mut mismatched_terminal = routing_events(
        "mismatched-terminal",
        "Agent task completed",
        Metadata::new(),
    );
    mismatched_terminal
        .last_mut()
        .expect("terminal event")
        .metadata
        .insert(
            EVENT_TYPE_METADATA_KEY.to_string(),
            EventTypeV1::AgentRunFailed.id().to_string(),
        );
    assert!(routing_telemetry_from_events(&mismatched_terminal).is_empty());
}

#[test]
fn trusted_routing_cost_uses_lineage_aggregate_across_restarts_and_embeddings() {
    let run_id = "physical-aggregate";
    let evidence = LearningEvidenceV1::verified_postcondition(
        LearningUsageCompleteness::Partial,
        0,
        learning_budget_fingerprint(&context(run_id)).expect("budget fingerprint"),
    )
    .to_metadata_value()
    .expect("evidence encodes");
    let telemetry =
        routing_telemetry_from_events(&routing_events(run_id, "Agent task completed", {
            let mut metadata = resource_usage_metadata("run_lineage", 185, 1, 0, 1, 0);
            metadata.insert(
                orchestrator::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                evidence,
            );
            metadata.insert("routing_learning_eligible".to_string(), "true".to_string());
            metadata
        }));

    assert_eq!(telemetry.len(), 1);
    assert!(telemetry[0].learning_evidence.is_learnable());
    assert_eq!(
        telemetry[0].learning_evidence.usage_completeness,
        LearningUsageCompleteness::Partial
    );
    assert_eq!(telemetry[0].cost_proxy, 185);
}

#[test]
fn malformed_resource_source_counts_censor_typed_evidence_and_keep_legacy_cost_only() {
    let run_id = "malformed-aggregate";
    let evidence = LearningEvidenceV1::verified_postcondition(
        LearningUsageCompleteness::Complete,
        0,
        learning_budget_fingerprint(&context(run_id)).expect("budget fingerprint"),
    )
    .to_metadata_value()
    .expect("evidence encodes");
    let mut events = routing_events(
        run_id,
        "Agent task completed",
        [
            (
                orchestrator::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                evidence,
            ),
            ("routing_learning_eligible".to_string(), "true".to_string()),
            ("run_lineage_total_tokens".to_string(), "999".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    events.last_mut().expect("terminal exists").metadata.insert(
        "run_lineage_physical_model_attempts".to_string(),
        "2".to_string(),
    );
    let telemetry = routing_telemetry_from_events(&events);

    assert_eq!(telemetry.len(), 1);
    assert!(!telemetry[0].learning_evidence.is_learnable());
    assert_eq!(
        telemetry[0].learning_evidence.disposition,
        LearningDisposition::Censored
    );
    assert_eq!(telemetry[0].cost_proxy, 120);
}

#[test]
fn plain_completion_is_censored_even_with_complete_usage() {
    let telemetry = routing_telemetry_from_events(&routing_events(
        "plain",
        "Agent task completed",
        Metadata::new(),
    ));

    assert_eq!(telemetry.len(), 1);
    assert_eq!(
        telemetry[0].learning_evidence.disposition,
        LearningDisposition::Censored
    );
    assert!(!telemetry[0].learning_evidence.is_learnable());
}

#[test]
fn only_verified_postcondition_or_explicit_quality_gate_is_learnable() {
    let verified_fingerprint =
        learning_budget_fingerprint(&context("verified")).expect("budget fingerprint");
    let verified_evidence = LearningEvidenceV1::verified_postcondition(
        LearningUsageCompleteness::Complete,
        0,
        verified_fingerprint,
    )
    .to_metadata_value()
    .expect("evidence encodes");
    let verified = routing_telemetry_from_events(&routing_events(
        "verified",
        "Agent task completed",
        [
            (
                orchestrator::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                verified_evidence,
            ),
            (
                "completion_evidence".to_string(),
                "verified_mutation".to_string(),
            ),
            ("routing_learning_eligible".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect(),
    ));
    assert_eq!(
        verified[0].learning_evidence.disposition,
        LearningDisposition::Positive
    );
    assert_eq!(
        verified[0].learning_evidence.verification,
        LearningVerification::Passed
    );

    let mut gated_events = routing_events(
        "gated",
        "Agent task completed",
        [
            ("routing_learning_eligible".to_string(), "true".to_string()),
            (
                orchestrator::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                LearningEvidenceV1::independent_quality(
                    LearningTermination::Completed,
                    LearningAttribution::Workflow,
                    LearningUsageCompleteness::Complete,
                    0,
                    learning_budget_fingerprint(&context("gated")).expect("budget fingerprint"),
                    IndependentQualitySource::CollaborationQualityGate,
                    4_000,
                    false,
                )
                .to_metadata_value()
                .expect("evidence encodes"),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let gate_context = context("gated");
    gated_events.insert(
        3,
        event(
            4,
            EventKind::TaskStatusChanged,
            "Collaboration quality gate evaluated",
            {
                let mut metadata = gate_context;
                metadata.insert("quality_pass".to_string(), "false".to_string());
                metadata.insert("quality_score".to_string(), "0.40".to_string());
                metadata.insert("safety_violations".to_string(), "0".to_string());
                metadata
            },
        ),
    );
    gated_events.last_mut().expect("terminal exists").sequence = 5;
    gated_events
        .last_mut()
        .expect("terminal exists")
        .timestamp_ms = 500;
    let gated = routing_telemetry_from_events(&gated_events);
    assert_eq!(
        gated[0].learning_evidence.disposition,
        LearningDisposition::Negative
    );
    assert_eq!(gated[0].learning_evidence.quality_bps, Some(4_000));
}

#[test]
fn typed_censored_and_legacy_only_runs_cannot_be_upgraded() {
    let fingerprint =
        learning_budget_fingerprint(&context("typed-censored")).expect("budget fingerprint");
    let censored = LearningEvidenceV1::censored(
        LearningTermination::Completed,
        LearningAttribution::Unknown,
        LearningUsageCompleteness::Complete,
        Some(0),
        Some(fingerprint),
    )
    .to_metadata_value()
    .expect("evidence encodes");
    for (run_id, typed) in [("typed-censored", Some(censored)), ("legacy-only", None)] {
        let mut metadata = [
            (
                "completion_evidence".to_string(),
                "verified_mutation".to_string(),
            ),
            ("routing_learning_eligible".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        if let Some(typed) = typed {
            metadata.insert(
                orchestrator::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                typed,
            );
        }
        let telemetry = routing_telemetry_from_events(&routing_events(
            run_id,
            "Agent task completed",
            metadata,
        ));
        assert_eq!(telemetry.len(), 1);
        assert_eq!(
            telemetry[0].learning_evidence.disposition,
            LearningDisposition::Censored
        );
    }
}

#[test]
fn cancel_and_unknown_failure_are_censored() {
    for (run_id, summary) in [
        ("cancelled", "Agent task cancelled"),
        ("failed", "Agent task failed"),
    ] {
        let telemetry =
            routing_telemetry_from_events(&routing_events(run_id, summary, Metadata::new()));
        assert_eq!(telemetry.len(), 1);
        assert_eq!(
            telemetry[0].learning_evidence.disposition,
            LearningDisposition::Censored
        );
    }
}

#[test]
fn unmeasured_workflow_completion_is_censored() {
    let models = vec!["planner".to_string()];
    let workflow = orchestrator::AdaptiveWorkflow {
        steps: vec![orchestrator::AdaptiveWorkflowStep {
            id: "final".to_string(),
            role: "synthesizer".to_string(),
            model: "planner".to_string(),
            subtask: "produce final guidance".to_string(),
            access: Vec::new(),
        }],
    };
    let plan = WorkflowPlanIr::from_adaptive(
        "workflow",
        "review",
        "pro",
        "best_of_n",
        "planner",
        &workflow,
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: 1,
            max_tool_calls_per_step: 1,
            max_output_tokens_per_step: 1_024,
        },
    );
    let mut workflow_context = context("workflow-run");
    workflow_context.insert("collaboration_id".to_string(), "workflow".to_string());
    workflow_context.insert("task_class".to_string(), "research".to_string());
    let mut planned = workflow_context.clone();
    planned.insert(
        "workflow_ir".to_string(),
        plan.to_json().expect("plan should serialize"),
    );
    let mut usage = workflow_context.clone();
    usage.insert("total_tokens".to_string(), "80".to_string());
    usage.insert("usage_source".to_string(), "provider".to_string());
    let events = vec![
        event(
            1,
            EventKind::TaskStatusChanged,
            "Collaboration workflow planned",
            planned,
        ),
        event(
            2,
            EventKind::ModelRequestFinished,
            "Collaboration worker completed",
            usage,
        ),
        event(
            3,
            EventKind::TaskStatusChanged,
            "Collaboration workflow completed",
            workflow_context,
        ),
    ];

    let telemetry = workflow_execution_telemetry_from_events(&events, &models);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(
        telemetry[0].learning_evidence.disposition,
        LearningDisposition::Censored
    );
}
