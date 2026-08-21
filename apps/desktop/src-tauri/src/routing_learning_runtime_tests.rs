use super::*;
use agent_core::{AgentRunIdentity, EventTypeV1, EVENT_TYPE_METADATA_KEY};
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
                agent_core::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
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
fn logical_run_telemetry_aggregates_physical_attempts_once() {
    let logical_run_id = "logical-routing-run";
    let mut events = routing_events("attempt-a", "Agent task completed", Metadata::new());
    events.pop();
    for event in &mut events {
        AgentRunIdentity::new(logical_run_id, "attempt-a")
            .expect("initial identity should be valid")
            .insert_into(&mut event.metadata)
            .expect("initial identity should attach");
    }

    let mut retry_context = context("attempt-b");
    AgentRunIdentity::continuation(logical_run_id, "attempt-b", "attempt-a")
        .expect("continuation identity should be valid")
        .insert_into(&mut retry_context)
        .expect("continuation identity should attach");
    let mut retry_usage = retry_context.clone();
    retry_usage.insert("total_tokens".to_string(), "30".to_string());
    retry_usage.insert("usage_source".to_string(), "provider".to_string());
    events.extend([
        event(
            4,
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            retry_context.clone(),
        ),
        event(
            5,
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            retry_usage,
        ),
        event(
            6,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            retry_context,
        ),
    ]);

    let telemetry = routing_telemetry_from_events(&events);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].cost_proxy, 150);
    assert_eq!(telemetry[0].selected_model, "test-model");
}

#[test]
fn logical_run_latency_sums_active_attempts_without_pause_gap() {
    let logical_run_id = "logical-routing-latency";
    let mut initial = context("attempt-latency-a");
    AgentRunIdentity::new(logical_run_id, "attempt-latency-a")
        .expect("initial identity should be valid")
        .insert_into(&mut initial)
        .expect("initial identity should attach");
    let mut continuation = context("attempt-latency-b");
    AgentRunIdentity::continuation(logical_run_id, "attempt-latency-b", "attempt-latency-a")
        .expect("continuation identity should be valid")
        .insert_into(&mut continuation)
        .expect("continuation identity should attach");
    let at = |mut event: Event, timestamp_ms| {
        event.timestamp_ms = timestamp_ms;
        event
    };
    let events = vec![
        at(
            event(
                1,
                EventKind::TaskStatusChanged,
                "Agent task started",
                initial.clone(),
            ),
            1_000,
        ),
        at(
            event(
                2,
                EventKind::TaskStatusChanged,
                "Agent run decision selected",
                initial.clone(),
            ),
            1_100,
        ),
        at(
            event(
                3,
                EventKind::ModelRequestFinished,
                "Agent model turn finished",
                initial.clone(),
            ),
            1_200,
        ),
        at(
            event(
                4,
                EventKind::TaskStatusChanged,
                "Agent task paused",
                initial,
            ),
            1_300,
        ),
        at(
            event(
                5,
                EventKind::TaskStatusChanged,
                "Agent task retry started",
                continuation.clone(),
            ),
            100_000,
        ),
        at(
            event(
                6,
                EventKind::ModelRequestFinished,
                "Agent model turn finished",
                continuation.clone(),
            ),
            100_100,
        ),
        at(
            event(
                7,
                EventKind::TaskStatusChanged,
                "Agent task completed",
                continuation,
            ),
            100_200,
        ),
    ];

    let telemetry = routing_telemetry_from_events(&events);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].latency_ms, 500);
}

#[test]
fn routing_v5_rebuilds_from_events_without_consuming_v4_cache() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let events = routing_events("cache-rebuild", "Agent task completed", Metadata::new());
    for event in events {
        store
            .append_next_event(
                event.id,
                phase16_task_id(),
                event.timestamp_ms,
                event.kind,
                event.summary,
                event.metadata,
            )
            .expect("routing event should append");
    }
    let revision = store
        .event_revision(&phase16_task_id())
        .expect("routing revision should load");
    let stale = RoutingTelemetryReadModel {
        schema: "routing-telemetry-v4".to_string(),
        revision: revision.latest_sequence,
        event_count: revision.event_count,
        entries: Vec::new(),
    };
    store
        .save_read_model(
            "routing-telemetry-v4",
            ROUTING_TELEMETRY_READ_MODEL_KEY,
            stale.revision,
            &serde_json::to_string(&stale).expect("stale model should encode"),
        )
        .expect("stale routing model should save");

    let telemetry = load_routing_telemetry_read_model(&mut store)
        .expect("current routing model should rebuild");
    assert_eq!(telemetry.len(), 1);
    assert!(store
        .load_read_model("routing-telemetry-v4", ROUTING_TELEMETRY_READ_MODEL_KEY)
        .expect("old routing namespace should remain readable")
        .is_some());
    assert!(store
        .load_read_model(
            ROUTING_TELEMETRY_READ_MODEL_NAMESPACE,
            ROUTING_TELEMETRY_READ_MODEL_KEY,
        )
        .expect("current routing namespace should load")
        .is_some());
}

#[test]
fn incremental_routing_bridges_legacy_predecessors_into_explicit_lineage() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let append = |store: &mut SqliteStore, event: Event| {
        store
            .append_next_event(
                event.id,
                phase16_task_id(),
                event.timestamp_ms,
                event.kind,
                event.summary,
                event.metadata,
            )
            .expect("hybrid routing event should append");
    };
    let initial = context("attempt-a");
    let mut initial_usage = initial.clone();
    initial_usage.insert("total_tokens".to_string(), "10".to_string());
    for event in [
        event(
            1,
            EventKind::TaskStatusChanged,
            "Agent task started",
            initial.clone(),
        ),
        event(
            2,
            EventKind::TaskStatusChanged,
            "Agent run decision selected",
            initial.clone(),
        ),
        event(
            3,
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            initial_usage,
        ),
    ] {
        append(&mut store, event);
    }
    for (sequence, attempt_run_id, source_run_id, tokens) in [
        (4, "attempt-b", "attempt-a", "20"),
        (6, "attempt-c", "attempt-b", "30"),
    ] {
        let mut retry = context(attempt_run_id);
        retry.insert("source_agent_run_id".to_string(), source_run_id.to_string());
        let mut usage = retry.clone();
        usage.insert("total_tokens".to_string(), tokens.to_string());
        append(
            &mut store,
            event(
                sequence,
                EventKind::TaskStatusChanged,
                "Agent task retry started",
                retry.clone(),
            ),
        );
        append(
            &mut store,
            event(
                sequence + 1,
                EventKind::ModelRequestFinished,
                "Agent model turn finished",
                usage,
            ),
        );
        if attempt_run_id == "attempt-b" {
            append(
                &mut store,
                event(
                    20,
                    EventKind::ToolCallFinished,
                    "Agent tool call finished",
                    retry,
                ),
            );
        } else {
            append(
                &mut store,
                event(
                    21,
                    EventKind::RetrievalPerformed,
                    "Agent retrieval performed",
                    retry,
                ),
            );
        }
    }
    assert!(load_routing_telemetry_read_model(&mut store)
        .expect("incomplete hybrid routing model should build")
        .is_empty());

    let mut continuation = context("attempt-d");
    AgentRunIdentity::continuation("attempt-a", "attempt-d", "attempt-c")
        .expect("hybrid continuation identity should be valid")
        .insert_into(&mut continuation)
        .expect("hybrid continuation identity should attach");
    let mut usage = continuation.clone();
    usage.insert("total_tokens".to_string(), "40".to_string());
    append(
        &mut store,
        event(
            8,
            EventKind::TaskStatusChanged,
            "Agent task retry started",
            continuation.clone(),
        ),
    );
    append(
        &mut store,
        event(
            9,
            EventKind::ModelRequestFinished,
            "Agent model turn finished",
            usage,
        ),
    );
    append(
        &mut store,
        event(
            10,
            EventKind::TaskStatusChanged,
            "Agent task completed",
            continuation,
        ),
    );

    let telemetry = load_routing_telemetry_read_model(&mut store)
        .expect("completed hybrid routing model should advance");
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].cost_proxy, 100);
    assert_eq!(telemetry[0].tool_count, 1);
    assert_eq!(telemetry[0].retrieval_count, 1);
}

#[test]
fn incremental_routing_does_not_label_malformed_physical_fallback_as_logical() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    assert!(load_routing_telemetry_read_model(&mut store)
        .expect("empty routing model should build")
        .is_empty());
    let mut malformed = context("attempt-malformed");
    malformed.insert(
        "logical_agent_run_id".to_string(),
        "logical-malformed".to_string(),
    );
    malformed.insert(
        "agent_run_identity_schema".to_string(),
        "cindx.agent-run-identity.v999".to_string(),
    );
    for (sequence, summary) in [
        (1, "Agent task started"),
        (2, "Agent run decision selected"),
        (3, "Agent task completed"),
    ] {
        let event = event(
            sequence,
            EventKind::TaskStatusChanged,
            summary,
            malformed.clone(),
        );
        store
            .append_next_event(
                event.id,
                phase16_task_id(),
                event.timestamp_ms,
                event.kind,
                event.summary,
                event.metadata,
            )
            .expect("malformed routing event should append");
    }

    assert!(load_routing_telemetry_read_model(&mut store)
        .expect("malformed incremental routing update should fail closed")
        .is_empty());
}

#[test]
fn physical_fallback_drops_cross_session_attempt_id_collisions() {
    let mut events = Vec::new();
    for (project_id, session_id) in [
        ("project-routing-scope-a", "session-routing-scope-a"),
        ("project-routing-scope-b", "session-routing-scope-b"),
    ] {
        let mut scoped = routing_events(
            "attempt-routing-collision",
            "Agent task completed",
            Metadata::new(),
        );
        for event in &mut scoped {
            event
                .metadata
                .insert("project_id".to_string(), project_id.to_string());
            event
                .metadata
                .insert("session_id".to_string(), session_id.to_string());
        }
        events.extend(scoped);
    }

    assert!(routing_telemetry_from_events(&events).is_empty());
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
                agent_core::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
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
                agent_core::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
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
                agent_core::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
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
                agent_core::LEARNING_EVIDENCE_METADATA_KEY.to_string(),
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
