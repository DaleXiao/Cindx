use super::delivery_verification::*;
use agent_runtime::{
    DeliveryVerificationDecision, DeliveryVerificationEvidence, DeliveryVerificationObligation,
    DeliveryVerificationStatus, DeliveryVerificationSubjectV1, GroundedCompletionBasis,
    GroundedCompletionReceipt, DELIVERY_VERIFICATION_VERDICT_SCHEMA, GROUNDED_COMPLETION_SCHEMA,
};
use orchestrator::sha256_hex;
use serde_json::{json, Value};

const OBJECTIVE: &str = "Report only what the frozen evidence supports.";
const OWNER_DRAFT: &str = "All checks passed.";
const REPAIRED_DRAFT: &str = "One required check failed; delivery remains blocked.";
const OWNER_MODEL: &str = "owner-primary";
const VERIFIER_MODEL: &str = "verifier-independent";

fn grounded_receipt(candidate: &str, model_turn: u64) -> GroundedCompletionReceipt {
    GroundedCompletionReceipt {
        schema: GROUNDED_COMPLETION_SCHEMA.to_string(),
        steer_epoch: 4,
        contract_epoch: 3,
        model_turn,
        content_sha256: sha256_hex(candidate.as_bytes()),
        content_bytes: candidate.len() as u64,
        obligation_digest: "1".repeat(64),
        covered_obligation_ids: vec!["2".repeat(64)],
        visible_evidence_sequences: vec![7],
        constraint_codes: Vec::new(),
        basis: GroundedCompletionBasis::EvidenceVisible,
    }
}

fn obligations() -> Vec<DeliveryVerificationObligation> {
    vec![DeliveryVerificationObligation {
        obligation_ref: "2".repeat(64),
        content: "Report whether every required check passed.".to_string(),
    }]
}

fn evidence() -> Vec<DeliveryVerificationEvidence> {
    vec![DeliveryVerificationEvidence {
        evidence_ref: 7,
        content: "One required check failed.".to_string(),
    }]
}

fn input() -> DeliveryVerificationEvalInput {
    DeliveryVerificationEvalInput {
        evaluation_id: "delivery-eval-1".to_string(),
        objective: OBJECTIVE.to_string(),
        obligations: obligations(),
        evidence: evidence(),
        owner_draft: OWNER_DRAFT.to_string(),
        owner_receipt: grounded_receipt(OWNER_DRAFT, 2),
        owner_model: OWNER_MODEL.to_string(),
        verifier_model: VERIFIER_MODEL.to_string(),
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
    .expect("fixture subject should bind")
}

fn finding() -> Value {
    json!({
        "kind": "unsupported_claim",
        "summary": "The completion claim is unsupported by the bound evidence.",
        "obligationRefs": ["2".repeat(64)],
        "evidenceRefs": [7]
    })
}

fn verdict_json(subject: &DeliveryVerificationSubjectV1, decision: &str) -> String {
    json!({
        "schema": DELIVERY_VERIFICATION_VERDICT_SCHEMA,
        "subjectSha256": subject.subject_sha256,
        "reviewedObjectiveSha256": subject.objective_sha256,
        "decision": decision,
        "reviewedObligationRefs": subject.obligation_refs,
        "reviewedEvidenceRefs": subject.evidence_refs,
        "findings": if decision == "passed" { Vec::<Value>::new() } else { vec![finding()] }
    })
    .to_string()
}

fn verifier_attempt(
    stage: DeliveryVerificationEvalStage,
    subject: &DeliveryVerificationSubjectV1,
    decision: &str,
) -> DeliveryVerificationEvalAttempt {
    DeliveryVerificationEvalAttempt {
        stage,
        configured_model: VERIFIER_MODEL.to_string(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::VerifierResponse(verdict_json(
            subject, decision,
        )),
    }
}

fn repair_attempt() -> DeliveryVerificationEvalAttempt {
    DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::OwnerRepair,
        configured_model: OWNER_MODEL.to_string(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::OwnerRepair {
            candidate: REPAIRED_DRAFT.to_string(),
            receipt: grounded_receipt(REPAIRED_DRAFT, 3),
        },
    }
}

#[test]
fn agent_delivery_verification_contract_initial_pass_preserves_exact_owner_bytes() {
    println!("{}", DELIVERY_VERIFICATION_EVAL_SCHEMA);
    let initial = subject(OWNER_DRAFT, 2);
    let observation = project_delivery_verification_evaluation(
        input(),
        &[verifier_attempt(
            DeliveryVerificationEvalStage::InitialVerification,
            &initial,
            "passed",
        )],
    )
    .expect("a valid pass should be observed");

    assert_eq!(observation.schema, DELIVERY_VERIFICATION_EVAL_SCHEMA);
    assert_eq!(
        observation.disposition,
        DeliveryVerificationTreatmentDisposition::PassedUnchanged
    );
    assert_eq!(
        observation.terminal_status,
        DeliveryVerificationStatus::Passed
    );
    assert_eq!(observation.control.output.as_deref(), Some(OWNER_DRAFT));
    assert_eq!(observation.treatment.output.as_deref(), Some(OWNER_DRAFT));
    assert_eq!(observation.control, observation.treatment);
    assert_eq!(
        observation.initial_subject_sha256,
        observation.final_subject_sha256
    );
    assert_eq!(observation.initial_verifier_calls, 1);
    assert_eq!(observation.owner_repair_calls, 0);
    assert_eq!(observation.recheck_calls, 0);
    assert_eq!(
        observation.initial_verifier_decision,
        Some(DeliveryVerificationDecision::Passed)
    );
    assert_eq!(
        observation.initial_finding_counts,
        DeliveryVerificationFindingCounts::default()
    );
    assert!(!observation.repair_activated);
    assert_eq!(observation.recheck_decision, None);
}

#[test]
fn agent_delivery_verification_contract_failed_verdict_runs_one_repair_and_one_recheck() {
    let initial = subject(OWNER_DRAFT, 2);
    let repaired = subject(REPAIRED_DRAFT, 3);
    let attempts = vec![
        verifier_attempt(
            DeliveryVerificationEvalStage::InitialVerification,
            &initial,
            "needs_revision",
        ),
        repair_attempt(),
        verifier_attempt(DeliveryVerificationEvalStage::Recheck, &repaired, "passed"),
    ];
    let observation = project_delivery_verification_evaluation(input(), &attempts)
        .expect("a bounded repair should be observed");

    assert_eq!(
        observation.disposition,
        DeliveryVerificationTreatmentDisposition::PassedAfterRepair
    );
    assert_eq!(observation.control.output.as_deref(), Some(OWNER_DRAFT));
    assert_eq!(
        observation.treatment.output.as_deref(),
        Some(REPAIRED_DRAFT)
    );
    assert_eq!(observation.initial_verifier_calls, 1);
    assert_eq!(observation.owner_repair_calls, 1);
    assert_eq!(observation.recheck_calls, 1);
    assert_eq!(
        observation.initial_verifier_decision,
        Some(DeliveryVerificationDecision::NeedsRevision)
    );
    assert_eq!(observation.initial_finding_counts.unsupported_claim, 1);
    assert!(observation.repair_activated);
    assert_eq!(
        observation.recheck_decision,
        Some(DeliveryVerificationDecision::Passed)
    );
    assert_ne!(
        observation.initial_subject_sha256,
        observation.final_subject_sha256
    );
}

#[test]
fn agent_delivery_verification_contract_recheck_rejection_is_a_treatment_failure() {
    let initial = subject(OWNER_DRAFT, 2);
    let repaired = subject(REPAIRED_DRAFT, 3);
    let attempts = vec![
        verifier_attempt(
            DeliveryVerificationEvalStage::InitialVerification,
            &initial,
            "needs_revision",
        ),
        repair_attempt(),
        verifier_attempt(
            DeliveryVerificationEvalStage::Recheck,
            &repaired,
            "needs_revision",
        ),
    ];
    let observation = project_delivery_verification_evaluation(input(), &attempts)
        .expect("a model-level rejection remains observed evidence");

    assert_eq!(
        observation.disposition,
        DeliveryVerificationTreatmentDisposition::TreatmentFailure
    );
    assert_eq!(
        observation.terminal_status,
        DeliveryVerificationStatus::Unverified
    );
    assert!(observation.control.completed);
    assert!(!observation.treatment.completed);
    assert_eq!(
        observation.failure_stage,
        Some(DeliveryVerificationEvalStage::Recheck)
    );
    assert_eq!(
        observation.failure_code,
        Some(DeliveryVerificationTreatmentFailureCode::VerificationNotSatisfied)
    );
}

#[test]
fn agent_delivery_verification_contract_model_failure_is_not_censored() {
    let attempts = [DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::InitialVerification,
        configured_model: VERIFIER_MODEL.to_string(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::ModelFailure {
            kind: DeliveryVerificationModelFailure::Timeout,
        },
    }];
    let observation = project_delivery_verification_evaluation(input(), &attempts)
        .expect("a provider failure must stay in the treatment denominator");

    assert_eq!(
        observation.disposition,
        DeliveryVerificationTreatmentDisposition::TreatmentFailure
    );
    assert_eq!(
        observation.failure_code,
        Some(DeliveryVerificationTreatmentFailureCode::Timeout)
    );
    assert!(observation.control.completed);
    assert!(!observation.treatment.completed);
}

#[test]
fn agent_delivery_verification_contract_invalid_verifier_json_is_not_censored() {
    let attempts = [DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::InitialVerification,
        configured_model: VERIFIER_MODEL.to_string(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::VerifierResponse(
            "not strict json".to_string(),
        ),
    }];
    let observation = project_delivery_verification_evaluation(input(), &attempts)
        .expect("invalid model output is a treatment failure");

    assert_eq!(
        observation.disposition,
        DeliveryVerificationTreatmentDisposition::TreatmentFailure
    );
    assert_eq!(
        observation.failure_code,
        Some(DeliveryVerificationTreatmentFailureCode::InvalidVerifierResponse)
    );
}

#[test]
fn agent_delivery_verification_contract_user_cancellation_is_censored() {
    let attempts = [DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::InitialVerification,
        configured_model: VERIFIER_MODEL.to_string(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::Cancelled,
    }];

    assert_eq!(
        project_delivery_verification_evaluation(input(), &attempts),
        Err(DeliveryVerificationEvalCensor::Cancelled)
    );
}

#[test]
fn agent_delivery_verification_contract_user_steer_is_censored_separately() {
    let attempts = [DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::InitialVerification,
        configured_model: VERIFIER_MODEL.to_string(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::Steered,
    }];

    assert_eq!(
        project_delivery_verification_evaluation(input(), &attempts),
        Err(DeliveryVerificationEvalCensor::Steered)
    );
}

#[test]
fn agent_delivery_verification_contract_requires_distinct_owner_and_verifier_models() {
    let mut same_model = input();
    same_model.verifier_model = same_model.owner_model.clone();

    assert_eq!(
        project_delivery_verification_evaluation(same_model, &[]),
        Err(DeliveryVerificationEvalCensor::ModelsNotIndependent)
    );
}

#[test]
fn agent_delivery_verification_contract_rejects_tool_authority_and_wrong_model() {
    let initial = subject(OWNER_DRAFT, 2);
    let mut toolful = verifier_attempt(
        DeliveryVerificationEvalStage::InitialVerification,
        &initial,
        "passed",
    );
    toolful.tool_count = 1;
    assert_eq!(
        project_delivery_verification_evaluation(input(), &[toolful]),
        Err(DeliveryVerificationEvalCensor::ToolAuthorityEscaped)
    );

    let mut wrong_model = verifier_attempt(
        DeliveryVerificationEvalStage::InitialVerification,
        &initial,
        "passed",
    );
    wrong_model.configured_model = OWNER_MODEL.to_string();
    assert_eq!(
        project_delivery_verification_evaluation(input(), &[wrong_model]),
        Err(DeliveryVerificationEvalCensor::WrongModel)
    );
}

#[test]
fn agent_delivery_verification_contract_rejects_missing_extra_or_replayed_attempts() {
    assert_eq!(
        project_delivery_verification_evaluation(input(), &[]),
        Err(DeliveryVerificationEvalCensor::MissingAttempt)
    );

    let initial = subject(OWNER_DRAFT, 2);
    let pass = verifier_attempt(
        DeliveryVerificationEvalStage::InitialVerification,
        &initial,
        "passed",
    );
    assert_eq!(
        project_delivery_verification_evaluation(input(), &[pass.clone(), pass]),
        Err(DeliveryVerificationEvalCensor::UnexpectedAttempt)
    );
}

#[test]
fn agent_delivery_verification_contract_rejects_unbound_repair_receipt() {
    let initial = subject(OWNER_DRAFT, 2);
    let attempts = vec![
        verifier_attempt(
            DeliveryVerificationEvalStage::InitialVerification,
            &initial,
            "needs_revision",
        ),
        DeliveryVerificationEvalAttempt {
            stage: DeliveryVerificationEvalStage::OwnerRepair,
            configured_model: OWNER_MODEL.to_string(),
            tool_count: 0,
            result: DeliveryVerificationEvalAttemptResult::OwnerRepair {
                candidate: REPAIRED_DRAFT.to_string(),
                receipt: grounded_receipt("different bytes", 3),
            },
        },
    ];

    assert_eq!(
        project_delivery_verification_evaluation(input(), &attempts),
        Err(DeliveryVerificationEvalCensor::InvalidRepairBinding)
    );
}
