use super::*;
use agent_runtime::{
    DeliveryVerificationSubjectV1, DeliveryVerificationVerdictV1, GroundedCompletionBasis,
    GroundedCompletionReceipt, DELIVERY_VERIFICATION_VERDICT_SCHEMA, GROUNDED_COMPLETION_SCHEMA,
    MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES,
};
use serde_json::{json, Value};

const OBJECTIVE: &str = "Answer using only the supplied evidence.";
const DRAFT: &str = "The observed color is amber.";
const REPAIRED_DRAFT: &str = "The supplied evidence reports the color as amber.";

fn grounded_receipt(candidate: &str, model_turn: u64) -> GroundedCompletionReceipt {
    GroundedCompletionReceipt {
        schema: GROUNDED_COMPLETION_SCHEMA.to_string(),
        steer_epoch: 4,
        contract_epoch: 3,
        model_turn,
        content_sha256: sha256_hex(candidate.as_bytes()),
        content_bytes: candidate.len() as u64,
        obligation_digest: "1".repeat(64),
        covered_obligation_ids: vec!["2".repeat(64), "3".repeat(64)],
        visible_evidence_sequences: vec![7, 9],
        constraint_codes: Vec::new(),
        basis: GroundedCompletionBasis::EvidenceVisible,
    }
}

fn subject(candidate: &str, model_turn: u64) -> DeliveryVerificationSubjectV1 {
    DeliveryVerificationSubjectV1::bind(
        OBJECTIVE,
        candidate,
        &grounded_receipt(candidate, model_turn),
        &obligations(),
        &evidence(),
    )
    .expect("fixture subject")
}

fn budget() -> DeliveryVerificationRequestBudget {
    DeliveryVerificationRequestBudget {
        max_request_bytes: 64 * 1024,
        max_output_tokens: 2_048,
    }
}

fn obligations() -> Vec<DeliveryVerificationObligation> {
    vec![
        DeliveryVerificationObligation {
            obligation_ref: "2".repeat(64),
            content: "State the observed color.".to_string(),
        },
        DeliveryVerificationObligation {
            obligation_ref: "3".repeat(64),
            content: "Do not claim evidence that was not supplied.".to_string(),
        },
    ]
}

fn evidence() -> Vec<DeliveryVerificationEvidence> {
    vec![
        DeliveryVerificationEvidence {
            evidence_ref: 7,
            content: "The observed color is amber.".to_string(),
        },
        DeliveryVerificationEvidence {
            evidence_ref: 9,
            content: "No other color observation was supplied.".to_string(),
        },
    ]
}

fn verifier_input<'a>(
    subject: &'a DeliveryVerificationSubjectV1,
    obligations: &'a [DeliveryVerificationObligation],
    evidence: &'a [DeliveryVerificationEvidence],
) -> DeliveryVerificationRequestInput<'a> {
    DeliveryVerificationRequestInput {
        subject,
        objective: OBJECTIVE,
        obligations,
        evidence,
        owner_draft: DRAFT,
        owner_model: "owner-primary",
        verifier_model: "independent-reviewer",
        budget: budget(),
    }
}

#[test]
fn agent_delivery_verification_contract_owner_draft_request_is_exact_tool_free_non_streaming_and_executor_owned(
) {
    let obligations = obligations();
    let evidence = evidence();
    let prepared = prepare_owner_draft_request(DeliveryOwnerDraftRequestInput {
        objective: OBJECTIVE,
        obligations: &obligations,
        evidence: &evidence,
        owner_model: "owner-primary",
        budget: DeliveryVerificationRequestBudget {
            max_request_bytes: 64 * 1024,
            max_output_tokens: 4_096,
        },
    })
    .expect("Owner draft request");
    let request = prepared.request();

    assert_eq!(request.role, ModelRole::Executor);
    assert_eq!(request.mode, ModelCallMode::NonStreaming);
    assert!(request.tools.is_empty());
    assert_eq!(
        request
            .metadata
            .get("delivery_verification_target_model")
            .map(String::as_str),
        Some("owner-primary")
    );
    assert_eq!(
        request
            .metadata
            .get("max_output_tokens")
            .map(String::as_str),
        Some("4096")
    );
    let payload = serde_json::from_str::<Value>(&request.messages[1].content)
        .expect("exact Owner draft JSON payload");
    assert_eq!(
        payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["evidence", "kind", "objective", "obligations", "schema"]
    );
    assert_eq!(payload["kind"], "owner_draft");
    assert_eq!(payload["objective"], OBJECTIVE);
    assert_eq!(payload["obligations"][0]["obligationRef"], "2".repeat(64));
    assert_eq!(payload["evidence"][0]["evidenceRef"], 7);
    assert!(payload.get("oracle").is_none());
    assert!(payload.get("ownerDraft").is_none());
    assert_eq!(
        prepared.binding().objective_sha256,
        sha256_hex(OBJECTIVE.as_bytes())
    );
    assert!(prepared.binding().canonical_request_bytes <= 64 * 1024);

    let repeated = prepare_owner_draft_request(DeliveryOwnerDraftRequestInput {
        objective: OBJECTIVE,
        obligations: &obligations,
        evidence: &evidence,
        owner_model: "owner-primary",
        budget: DeliveryVerificationRequestBudget {
            max_request_bytes: 64 * 1024,
            max_output_tokens: 4_096,
        },
    })
    .unwrap();
    assert_eq!(
        prepared.binding().canonical_request_sha256,
        repeated.binding().canonical_request_sha256
    );
}

#[test]
fn agent_delivery_verification_contract_owner_draft_request_fails_closed_before_dispatch() {
    let obligations = obligations();
    let evidence = evidence();
    let input = |objective, owner_model, budget| DeliveryOwnerDraftRequestInput {
        objective,
        obligations: &obligations,
        evidence: &evidence,
        owner_model,
        budget,
    };

    assert!(prepare_owner_draft_request(input(" ", "owner-primary", budget())).is_err());
    assert!(prepare_owner_draft_request(input(OBJECTIVE, " owner-primary", budget())).is_err());
    assert!(prepare_owner_draft_request(input(
        OBJECTIVE,
        "owner-primary",
        DeliveryVerificationRequestBudget {
            max_request_bytes: 1,
            max_output_tokens: 4_096,
        },
    ))
    .is_err());

    let mut oversized = obligations.clone();
    oversized[0].content = "x".repeat(MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES + 1);
    assert!(prepare_owner_draft_request(DeliveryOwnerDraftRequestInput {
        objective: OBJECTIVE,
        obligations: &oversized,
        evidence: &evidence,
        owner_model: "owner-primary",
        budget: budget(),
    })
    .is_err());
}

fn needs_revision_verdict(
    subject: &DeliveryVerificationSubjectV1,
) -> DeliveryVerificationVerdictV1 {
    DeliveryVerificationVerdictV1::from_json(
        subject,
        &json!({
            "schema": DELIVERY_VERIFICATION_VERDICT_SCHEMA,
            "subjectSha256": subject.subject_sha256,
            "reviewedObjectiveSha256": subject.objective_sha256,
            "decision": "needs_revision",
            "reviewedObligationRefs": subject.obligation_refs,
            "reviewedEvidenceRefs": subject.evidence_refs,
            "findings": [{
                "kind": "unsupported_claim",
                "summary": "Use the evidence-qualified wording.",
                "obligationRefs": subject.obligation_refs,
                "evidenceRefs": subject.evidence_refs
            }]
        })
        .to_string(),
    )
    .expect("fixture verdict")
}

#[test]
fn agent_delivery_verification_contract_verifier_request_is_exact_tool_free_non_streaming_and_reviewer_owned(
) {
    let subject = subject(DRAFT, 2);
    let obligations = obligations();
    let evidence = evidence();
    let prepared = prepare_verifier_request(verifier_input(&subject, &obligations, &evidence))
        .expect("verifier request");
    let request = prepared.request();

    assert_eq!(request.role, ModelRole::Reviewer);
    assert_eq!(request.mode, ModelCallMode::NonStreaming);
    assert!(request.tools.is_empty());
    assert_eq!(
        request
            .metadata
            .get("delivery_verification_target_model")
            .map(String::as_str),
        Some("independent-reviewer")
    );
    assert_eq!(
        request.metadata.get("delivery_verification_subject_sha256"),
        Some(&subject.subject_sha256)
    );
    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0].role, MessageRole::System);
    assert_eq!(request.messages[1].role, MessageRole::User);

    let payload = serde_json::from_str::<Value>(&request.messages[1].content)
        .expect("exact verifier JSON payload");
    assert_eq!(payload["objective"], OBJECTIVE);
    assert_eq!(payload["obligations"][0]["obligationRef"], "2".repeat(64));
    assert_eq!(payload["obligations"][1]["obligationRef"], "3".repeat(64));
    assert_eq!(payload["evidence"][0]["evidenceRef"], 7);
    assert_eq!(payload["evidence"][1]["evidenceRef"], 9);
    assert_eq!(payload["ownerDraft"], DRAFT);
    assert_eq!(payload["subject"]["subjectSha256"], subject.subject_sha256);
    assert_eq!(prepared.binding().subject_sha256, subject.subject_sha256);
    assert_eq!(prepared.binding().draft_sha256, subject.candidate_sha256);
    assert!(prepared.binding().canonical_request_bytes <= budget().max_request_bytes as u64);
}

#[test]
fn agent_delivery_verification_contract_repair_uses_actual_owner_role_and_binds_verdict() {
    let subject = subject(DRAFT, 2);
    let verdict = needs_revision_verdict(&subject);
    let obligations = obligations();
    let evidence = evidence();
    let prepared = prepare_owner_repair_request(DeliveryRepairRequestInput {
        subject: &subject,
        objective: OBJECTIVE,
        obligations: &obligations,
        evidence: &evidence,
        owner_draft: DRAFT,
        verdict: &verdict,
        owner_role: ModelRole::Executor,
        owner_model: "owner-primary",
        verifier_model: "independent-reviewer",
        budget: budget(),
    })
    .expect("Owner repair request");
    let request = prepared.request();

    assert_eq!(request.role, ModelRole::Executor);
    assert_eq!(request.mode, ModelCallMode::NonStreaming);
    assert!(request.tools.is_empty());
    assert_eq!(
        request
            .metadata
            .get("delivery_verification_target_model")
            .map(String::as_str),
        Some("owner-primary")
    );
    assert_eq!(
        prepared.binding().verdict_sha256.as_deref(),
        Some(verdict.receipt_sha256.as_str())
    );
    let payload = serde_json::from_str::<Value>(&request.messages[1].content)
        .expect("exact repair JSON payload");
    assert_eq!(
        payload["verifierVerdict"]["receiptSha256"],
        verdict.receipt_sha256
    );
    assert_eq!(payload["ownerDraft"], DRAFT);
}

#[test]
fn agent_delivery_verification_contract_repair_accepts_planner_and_rejects_non_owner_roles() {
    let subject = subject(DRAFT, 2);
    let verdict = needs_revision_verdict(&subject);
    let obligations = obligations();
    let evidence = evidence();
    let make_input = |owner_role| DeliveryRepairRequestInput {
        subject: &subject,
        objective: OBJECTIVE,
        obligations: &obligations,
        evidence: &evidence,
        owner_draft: DRAFT,
        verdict: &verdict,
        owner_role,
        owner_model: "owner-reasoning",
        verifier_model: "independent-reviewer",
        budget: budget(),
    };

    let planner = prepare_owner_repair_request(make_input(ModelRole::Planner))
        .expect("routed reasoning Owner");
    assert_eq!(planner.request().role, ModelRole::Planner);
    for role in [
        ModelRole::Reviewer,
        ModelRole::Summarizer,
        ModelRole::Embedder,
    ] {
        assert!(prepare_owner_repair_request(make_input(role)).is_err());
    }
}

#[test]
fn agent_delivery_verification_contract_requests_fail_closed_on_identity_mismatch() {
    let subject = subject(DRAFT, 2);
    let obligations = obligations();
    let evidence = evidence();
    let mut wrong_objective = verifier_input(&subject, &obligations, &evidence);
    wrong_objective.objective = "A different objective.";
    assert!(prepare_verifier_request(wrong_objective).is_err());

    let mut wrong_draft = verifier_input(&subject, &obligations, &evidence);
    wrong_draft.owner_draft = REPAIRED_DRAFT;
    assert!(prepare_verifier_request(wrong_draft).is_err());

    let mut same_model = verifier_input(&subject, &obligations, &evidence);
    same_model.verifier_model = same_model.owner_model;
    assert!(prepare_verifier_request(same_model).is_err());
}

#[test]
fn agent_delivery_verification_contract_request_rejects_replaced_bound_content() {
    let subject = subject(DRAFT, 2);
    let obligations = obligations();
    let evidence = evidence();
    let first = prepare_verifier_request(verifier_input(&subject, &obligations, &evidence))
        .expect("first request");
    let second = prepare_verifier_request(verifier_input(&subject, &obligations, &evidence))
        .expect("second request");
    let mut changed_evidence = evidence.clone();
    changed_evidence[0].content = "The observed color is ochre.".to_string();
    let changed_input = verifier_input(&subject, &obligations, &changed_evidence);

    assert_eq!(
        first.binding().canonical_request_sha256,
        second.binding().canonical_request_sha256
    );
    assert!(prepare_verifier_request(changed_input).is_err());

    let verdict = needs_revision_verdict(&subject);
    assert!(prepare_owner_repair_request(DeliveryRepairRequestInput {
        subject: &subject,
        objective: OBJECTIVE,
        obligations: &obligations,
        evidence: &changed_evidence,
        owner_draft: DRAFT,
        verdict: &verdict,
        owner_role: ModelRole::Executor,
        owner_model: "owner-primary",
        verifier_model: "independent-reviewer",
        budget: budget(),
    })
    .is_err());
}

#[test]
fn agent_delivery_verification_contract_recheck_reuses_builder_for_repaired_subject() {
    let repaired_subject = subject(REPAIRED_DRAFT, 3);
    let obligations = obligations();
    let evidence = evidence();
    let mut input = verifier_input(&repaired_subject, &obligations, &evidence);
    input.owner_draft = REPAIRED_DRAFT;
    let prepared = prepare_verifier_request(input).expect("recheck request");

    assert_eq!(
        prepared.binding().subject_sha256,
        repaired_subject.subject_sha256
    );
    assert_eq!(
        prepared.binding().draft_sha256,
        repaired_subject.candidate_sha256
    );
}

#[test]
fn agent_delivery_verification_contract_request_and_output_budgets_are_hard_bounded() {
    let subject = subject(DRAFT, 2);
    let obligations = obligations();
    let evidence = evidence();
    let mut too_small = verifier_input(&subject, &obligations, &evidence);
    too_small.budget.max_request_bytes = 1;
    assert!(prepare_verifier_request(too_small).is_err());

    let mut too_large = verifier_input(&subject, &obligations, &evidence);
    too_large.budget.max_request_bytes = MAX_DELIVERY_VERIFICATION_REQUEST_BYTES + 1;
    assert!(prepare_verifier_request(too_large).is_err());

    let mut unbounded_output = verifier_input(&subject, &obligations, &evidence);
    unbounded_output.budget.max_output_tokens = MAX_DELIVERY_VERIFICATION_REQUEST_OUTPUT_TOKENS + 1;
    assert!(prepare_verifier_request(unbounded_output).is_err());
}

#[test]
fn agent_delivery_verification_contract_reference_context_exactly_covers_subject_refs() {
    let subject = subject(DRAFT, 2);
    let obligations = obligations();
    let evidence = evidence();

    let mut empty_content = obligations.clone();
    empty_content[0].content = "  ".to_string();
    assert!(prepare_verifier_request(verifier_input(&subject, &empty_content, &evidence)).is_err());

    assert!(
        prepare_verifier_request(verifier_input(&subject, &obligations[..1], &evidence)).is_err()
    );

    let mut out_of_order = evidence.clone();
    out_of_order.swap(0, 1);
    assert!(
        prepare_verifier_request(verifier_input(&subject, &obligations, &out_of_order)).is_err()
    );

    let mut unknown_ref = obligations.clone();
    unknown_ref[1].obligation_ref = "4".repeat(64);
    assert!(prepare_verifier_request(verifier_input(&subject, &unknown_ref, &evidence)).is_err());
}

#[test]
fn agent_delivery_verification_contract_reference_context_has_total_byte_bound() {
    let subject = subject(DRAFT, 2);
    let mut obligations = obligations();
    let evidence = evidence();
    obligations[0].content = "x".repeat(MAX_DELIVERY_VERIFICATION_BOUND_CONTEXT_BYTES + 1);

    assert!(prepare_verifier_request(verifier_input(&subject, &obligations, &evidence)).is_err());
}
