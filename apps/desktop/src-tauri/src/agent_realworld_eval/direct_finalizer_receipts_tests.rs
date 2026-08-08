use super::direct_finalizer_receipts::DirectFinalizerExecutionReceipt;
use super::receipts::strategy_receipt_from_events;
use super::Treatment;
use crate::agent_finalizer_runtime::direct_finalizer_policy::{
    insert_direct_finalizer_receipt_metadata, install_direct_finalizer_policy_metadata,
    selected_direct_finalizer_policy,
};
use crate::prompt_profile_serving::{seed_prompt_profile_selection, PromptProfileFallback};
use agent_core::{
    Event, EventId, EventKind, Metadata, TaskId, AGENT_RUN_ID_METADATA_KEY,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use orchestrator::{
    prompt_genome_sha256, AgentExecutionMode, AgentPolicy, AgentRunDecision, PromptVerification,
};

fn event(summary: &str, kind: EventKind, metadata: Metadata) -> Event {
    Event {
        id: EventId("direct-finalizer-receipt-event".to_string()),
        task_id: TaskId("direct-finalizer-receipt-task".to_string()),
        sequence: 7,
        timestamp_ms: 1,
        kind,
        summary: summary.to_string(),
        metadata,
    }
}

fn strategy_events() -> Vec<Event> {
    let mut context = Metadata::from([
        ("project_id".to_string(), "receipt-project".to_string()),
        ("agent_effort".to_string(), "auto".to_string()),
        ("collaboration_profile".to_string(), "direct".to_string()),
        ("steer_epoch".to_string(), "2".to_string()),
        (
            AGENT_RUN_ID_METADATA_KEY.to_string(),
            "physical-receipt-run".to_string(),
        ),
        (
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            "logical-receipt-run".to_string(),
        ),
    ]);
    let mut selection =
        seed_prompt_profile_selection("auto", PromptProfileFallback::EvolutionDisabled, &context);
    selection.genome.id = "direct-finalizer-receipt-profile".to_string();
    selection.genome.direct_finalizer_verification = PromptVerification::Adversarial;
    let profile_sha256 = prompt_genome_sha256(&selection.genome).unwrap();
    selection.receipt.profile_id = selection.genome.id.clone();
    selection.receipt.profile_sha256 = profile_sha256.clone();
    selection.receipt.stable_profile_sha256 = profile_sha256;
    let (assignment_receipt, assignment_sha256) = selection.receipt_json_and_sha256().unwrap();
    context.insert("prompt_profile".to_string(), selection.genome.id.clone());
    context.insert(
        "prompt_profile_source".to_string(),
        selection.source_label(),
    );
    context.insert(
        "prompt_genome".to_string(),
        serde_json::to_string(&selection.genome).unwrap(),
    );
    context.insert(
        "route_prompt_profile_sha256".to_string(),
        selection
            .genome
            .route_decision_profile_sha256("auto")
            .unwrap(),
    );
    context.insert(
        "prompt_profile_assignment_receipt".to_string(),
        assignment_receipt,
    );
    context.insert(
        "prompt_profile_assignment_sha256".to_string(),
        assignment_sha256,
    );
    context.insert(
        "run_decision".to_string(),
        serde_json::to_string(&AgentRunDecision::direct("configured-model")).unwrap(),
    );
    install_direct_finalizer_policy_metadata(
        &mut context,
        AgentPolicy::Auto,
        AgentExecutionMode::Direct,
        &selection.genome,
    )
    .unwrap();
    let selected =
        selected_direct_finalizer_policy(&context).expect("assigned phenotype should select");

    let mut decision = context.clone();
    decision.insert("requested_policy".to_string(), "auto_router".to_string());
    decision.insert("collaboration_policy".to_string(), "single".to_string());
    decision.insert("decision_source".to_string(), "fixture".to_string());
    decision.insert("routing_signature".to_string(), "frozen-route".to_string());

    let request_id = "private-direct-finalizer-request";
    let mut started = context.clone();
    insert_direct_finalizer_receipt_metadata(&mut started, &selected);
    started.insert("request_id".to_string(), request_id.to_string());
    started.insert("execution_role".to_string(), "finalizer".to_string());
    started.insert("terminal_commit".to_string(), "true".to_string());

    let mut finished = context.clone();
    insert_direct_finalizer_receipt_metadata(&mut finished, &selected);
    finished.insert("request_id".to_string(), request_id.to_string());
    finished.insert("request_payload_sha256".to_string(), "c".repeat(64));

    let mut terminal = context;
    insert_direct_finalizer_receipt_metadata(&mut terminal, &selected);
    terminal.insert(
        "direct_finalizer_profile_exercised".to_string(),
        "true".to_string(),
    );
    terminal.insert(
        "direct_finalizer_delivery_request_id".to_string(),
        request_id.to_string(),
    );
    terminal.insert("finalizer_fallback".to_string(), "false".to_string());

    vec![
        event(
            "Agent run decision selected",
            EventKind::TaskStatusChanged,
            decision,
        ),
        event(
            "Agent model turn started",
            EventKind::ModelRequestStarted,
            started,
        ),
        event(
            "Agent model turn finished",
            EventKind::ModelRequestFinished,
            finished,
        ),
        event(
            "Agent task completed",
            EventKind::TaskStatusChanged,
            terminal,
        ),
    ]
}

fn execution_receipt(events: &[Event]) -> DirectFinalizerExecutionReceipt {
    strategy_receipt_from_events(events, Treatment::Auto, None)
        .expect("strategy receipt")
        .expect("product strategy")
        .direct_finalizer_execution
        .expect("direct phenotype execution receipt")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[test]
fn proves_assignment_request_and_non_fallback_delivery() {
    let direct = execution_receipt(&strategy_events());
    let encoded = serde_json::to_string(&direct).unwrap();

    assert_eq!(direct.verification, "adversarial");
    assert_eq!(
        direct.treatment_specific_request_payload_sha256,
        "c".repeat(64)
    );
    assert!(is_sha256(&direct.prompt_profile_assignment_sha256));
    assert!(is_sha256(&direct.phenotype_receipt_sha256));
    assert!(is_sha256(&direct.phenotype_sha256));
    assert!(is_sha256(&direct.delivery_request_id_sha256));
    assert!(!encoded.contains("private-direct-finalizer-request"));
}

#[test]
fn rejects_a_mismatched_finished_request() {
    let mut events = strategy_events();
    events[2].metadata.insert(
        "request_id".to_string(),
        "different-finalizer-request".to_string(),
    );

    assert!(strategy_receipt_from_events(&events, Treatment::Auto, None)
        .unwrap_err()
        .contains("matching direct-finalizer finished event is missing"));
}

#[test]
fn rejects_a_tampered_assignment_receipt() {
    let mut events = strategy_events();
    events[0].metadata.insert(
        "prompt_profile_assignment_sha256".to_string(),
        "0".repeat(64),
    );

    assert!(strategy_receipt_from_events(&events, Treatment::Auto, None)
        .unwrap_err()
        .contains("assignment receipt digest mismatch"));
}

#[test]
fn fallback_terminal_keeps_the_route_receipt_without_claiming_finalizer_execution() {
    let mut events = strategy_events();
    events[3]
        .metadata
        .insert("finalizer_fallback".to_string(), "true".to_string());
    events[3]
        .metadata
        .remove("direct_finalizer_profile_exercised");

    let receipt = strategy_receipt_from_events(&events, Treatment::Auto, None)
        .unwrap()
        .expect("strategy receipt");
    assert!(receipt.direct_finalizer_execution.is_none());
}

#[test]
fn assigned_but_unexercised_finalizer_keeps_the_route_receipt() {
    let mut events = strategy_events();
    events[3]
        .metadata
        .remove("direct_finalizer_profile_exercised");

    let receipt = strategy_receipt_from_events(&events, Treatment::Auto, None)
        .unwrap()
        .expect("strategy receipt");
    assert!(receipt.direct_finalizer_execution.is_none());
}

#[test]
fn exercised_finalizer_cannot_claim_a_fallback_terminal() {
    let mut events = strategy_events();
    events[3]
        .metadata
        .insert("finalizer_fallback".to_string(), "true".to_string());

    assert!(strategy_receipt_from_events(&events, Treatment::Auto, None)
        .unwrap_err()
        .contains("execution claim is attached to a fallback terminal"));
}
