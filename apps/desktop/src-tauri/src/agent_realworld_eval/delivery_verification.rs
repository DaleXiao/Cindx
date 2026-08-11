use agent_runtime::{
    DeliveryVerificationDecision, DeliveryVerificationEvidence, DeliveryVerificationFindingKind,
    DeliveryVerificationObligation, DeliveryVerificationStateV1, DeliveryVerificationStatus,
    DeliveryVerificationSubjectV1, DeliveryVerificationVerdictV1, GroundedCompletionReceipt,
};
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};

pub(super) const DELIVERY_VERIFICATION_EVAL_SCHEMA: &str =
    "cindx.agent-eval.delivery-verification-observation.v4";

#[derive(Debug, Clone)]
pub(super) struct DeliveryVerificationEvalInput {
    pub(super) evaluation_id: String,
    pub(super) objective: String,
    pub(super) obligations: Vec<DeliveryVerificationObligation>,
    pub(super) evidence: Vec<DeliveryVerificationEvidence>,
    pub(super) owner_draft: String,
    pub(super) owner_receipt: GroundedCompletionReceipt,
    pub(super) owner_model: String,
    pub(super) verifier_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryVerificationEvalStage {
    InitialVerification,
    OwnerRepair,
    Recheck,
}

#[derive(Debug, Clone)]
pub(super) struct DeliveryVerificationEvalAttempt {
    pub(super) stage: DeliveryVerificationEvalStage,
    pub(super) configured_model: String,
    pub(super) tool_count: usize,
    pub(super) result: DeliveryVerificationEvalAttemptResult,
}

#[derive(Debug, Clone)]
pub(super) enum DeliveryVerificationEvalAttemptResult {
    VerifierResponse(String),
    OwnerRepair {
        candidate: String,
        receipt: GroundedCompletionReceipt,
    },
    ModelFailure {
        kind: DeliveryVerificationModelFailure,
    },
    Cancelled,
    Steered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryVerificationModelFailure {
    ProviderUnavailable,
    Timeout,
    BudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryVerificationTreatmentFailureCode {
    ProviderUnavailable,
    Timeout,
    BudgetExhausted,
    InvalidVerifierResponse,
    VerificationNotSatisfied,
}

impl From<DeliveryVerificationModelFailure> for DeliveryVerificationTreatmentFailureCode {
    fn from(failure: DeliveryVerificationModelFailure) -> Self {
        match failure {
            DeliveryVerificationModelFailure::ProviderUnavailable => Self::ProviderUnavailable,
            DeliveryVerificationModelFailure::Timeout => Self::Timeout,
            DeliveryVerificationModelFailure::BudgetExhausted => Self::BudgetExhausted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryVerificationTreatmentDisposition {
    PassedUnchanged,
    PassedAfterRepair,
    TreatmentFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryVerificationArmObservation {
    pub(super) completed: bool,
    pub(super) output: Option<String>,
    pub(super) output_sha256: Option<String>,
    pub(super) output_bytes: Option<u64>,
}

impl DeliveryVerificationArmObservation {
    fn completed(output: &str) -> Self {
        Self {
            completed: true,
            output: Some(output.to_string()),
            output_sha256: Some(sha256_hex(output.as_bytes())),
            output_bytes: Some(output.len() as u64),
        }
    }

    fn failed() -> Self {
        Self {
            completed: false,
            output: None,
            output_sha256: None,
            output_bytes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryVerificationEvalObservation {
    pub(super) schema: &'static str,
    pub(super) evaluation_id: String,
    pub(super) initial_subject_sha256: String,
    pub(super) final_subject_sha256: String,
    pub(super) owner_model: String,
    pub(super) verifier_model: String,
    pub(super) control: DeliveryVerificationArmObservation,
    pub(super) treatment: DeliveryVerificationArmObservation,
    pub(super) disposition: DeliveryVerificationTreatmentDisposition,
    pub(super) terminal_status: DeliveryVerificationStatus,
    pub(super) initial_verifier_calls: usize,
    pub(super) owner_repair_calls: usize,
    pub(super) recheck_calls: usize,
    pub(super) failure_stage: Option<DeliveryVerificationEvalStage>,
    pub(super) failure_code: Option<DeliveryVerificationTreatmentFailureCode>,
    pub(super) initial_verifier_decision: Option<DeliveryVerificationDecision>,
    pub(super) initial_finding_counts: DeliveryVerificationFindingCounts,
    pub(super) repair_activated: bool,
    pub(super) recheck_decision: Option<DeliveryVerificationDecision>,
    pub(super) recheck_finding_counts: DeliveryVerificationFindingCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationFindingCounts {
    pub(super) unsupported_claim: usize,
    pub(super) omitted_obligation: usize,
    pub(super) contradiction: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct EvaluationTelemetry {
    initial_verifier_decision: Option<DeliveryVerificationDecision>,
    initial_finding_counts: DeliveryVerificationFindingCounts,
    repair_activated: bool,
    recheck_decision: Option<DeliveryVerificationDecision>,
    recheck_finding_counts: DeliveryVerificationFindingCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryVerificationEvalCensor {
    InvalidEvaluationIdentity,
    InvalidOwnerSubject,
    ModelsNotIndependent,
    MissingAttempt,
    UnexpectedAttempt,
    WrongModel,
    ToolAuthorityEscaped,
    InvalidAttemptResult,
    InvalidRepairBinding,
    InvalidStateTransition,
    Cancelled,
    Steered,
}

pub(super) fn project_delivery_verification_evaluation(
    input: DeliveryVerificationEvalInput,
    attempts: &[DeliveryVerificationEvalAttempt],
) -> Result<DeliveryVerificationEvalObservation, DeliveryVerificationEvalCensor> {
    if input.evaluation_id.trim().is_empty()
        || input.owner_model.trim().is_empty()
        || input.verifier_model.trim().is_empty()
    {
        return Err(DeliveryVerificationEvalCensor::InvalidEvaluationIdentity);
    }
    if input.owner_model.trim() == input.verifier_model.trim() {
        return Err(DeliveryVerificationEvalCensor::ModelsNotIndependent);
    }

    let initial_subject = DeliveryVerificationSubjectV1::bind(
        &input.objective,
        &input.owner_draft,
        &input.owner_receipt,
        &input.obligations,
        &input.evidence,
    )
    .map_err(|_| DeliveryVerificationEvalCensor::InvalidOwnerSubject)?;
    let initial_subject_sha256 = initial_subject.subject_sha256.clone();
    let control = DeliveryVerificationArmObservation::completed(&input.owner_draft);
    let mut state = DeliveryVerificationStateV1::new(initial_subject)
        .map_err(|_| DeliveryVerificationEvalCensor::InvalidOwnerSubject)?;
    let mut cursor = 0usize;
    let mut telemetry = EvaluationTelemetry::default();

    let initial = take_attempt(
        attempts,
        &mut cursor,
        DeliveryVerificationEvalStage::InitialVerification,
        &input.verifier_model,
    )?;
    let initial_json = match &initial.result {
        DeliveryVerificationEvalAttemptResult::VerifierResponse(json) => json,
        DeliveryVerificationEvalAttemptResult::ModelFailure { kind } => {
            state
                .fail_closed()
                .map_err(|_| DeliveryVerificationEvalCensor::InvalidStateTransition)?;
            ensure_consumed(attempts, cursor)?;
            return Ok(failed_observation(
                &input,
                &state,
                initial_subject_sha256,
                control,
                1,
                0,
                0,
                DeliveryVerificationEvalStage::InitialVerification,
                (*kind).into(),
                &telemetry,
            ));
        }
        DeliveryVerificationEvalAttemptResult::Cancelled => {
            return Err(DeliveryVerificationEvalCensor::Cancelled)
        }
        DeliveryVerificationEvalAttemptResult::Steered => {
            return Err(DeliveryVerificationEvalCensor::Steered)
        }
        DeliveryVerificationEvalAttemptResult::OwnerRepair { .. } => {
            return Err(DeliveryVerificationEvalCensor::InvalidAttemptResult)
        }
    };
    let initial_verdict =
        match DeliveryVerificationVerdictV1::from_json(state.subject(), initial_json) {
            Ok(verdict) => verdict,
            Err(_) => {
                state
                    .fail_closed()
                    .map_err(|_| DeliveryVerificationEvalCensor::InvalidStateTransition)?;
                ensure_consumed(attempts, cursor)?;
                return Ok(failed_observation(
                    &input,
                    &state,
                    initial_subject_sha256,
                    control,
                    1,
                    0,
                    0,
                    DeliveryVerificationEvalStage::InitialVerification,
                    DeliveryVerificationTreatmentFailureCode::InvalidVerifierResponse,
                    &telemetry,
                ));
            }
        };
    let initial_decision = initial_verdict.decision;
    telemetry.initial_verifier_decision = Some(initial_decision);
    telemetry.initial_finding_counts = finding_counts(&initial_verdict);
    state
        .record_initial_verdict(initial_verdict)
        .map_err(|_| DeliveryVerificationEvalCensor::InvalidStateTransition)?;

    if initial_decision == DeliveryVerificationDecision::Passed {
        ensure_consumed(attempts, cursor)?;
        return Ok(DeliveryVerificationEvalObservation {
            schema: DELIVERY_VERIFICATION_EVAL_SCHEMA,
            evaluation_id: input.evaluation_id,
            initial_subject_sha256: initial_subject_sha256.clone(),
            final_subject_sha256: initial_subject_sha256,
            owner_model: input.owner_model,
            verifier_model: input.verifier_model,
            control: control.clone(),
            treatment: control,
            disposition: DeliveryVerificationTreatmentDisposition::PassedUnchanged,
            terminal_status: state.status(),
            initial_verifier_calls: 1,
            owner_repair_calls: 0,
            recheck_calls: 0,
            failure_stage: None,
            failure_code: None,
            initial_verifier_decision: telemetry.initial_verifier_decision,
            initial_finding_counts: telemetry.initial_finding_counts,
            repair_activated: telemetry.repair_activated,
            recheck_decision: telemetry.recheck_decision,
            recheck_finding_counts: telemetry.recheck_finding_counts,
        });
    }

    telemetry.repair_activated = true;
    let repair = take_attempt(
        attempts,
        &mut cursor,
        DeliveryVerificationEvalStage::OwnerRepair,
        &input.owner_model,
    )?;
    let (repaired_candidate, repaired_receipt) = match &repair.result {
        DeliveryVerificationEvalAttemptResult::OwnerRepair { candidate, receipt } => {
            (candidate, receipt)
        }
        DeliveryVerificationEvalAttemptResult::ModelFailure { kind } => {
            state
                .fail_closed()
                .map_err(|_| DeliveryVerificationEvalCensor::InvalidStateTransition)?;
            ensure_consumed(attempts, cursor)?;
            return Ok(failed_observation(
                &input,
                &state,
                initial_subject_sha256,
                control,
                1,
                1,
                0,
                DeliveryVerificationEvalStage::OwnerRepair,
                (*kind).into(),
                &telemetry,
            ));
        }
        DeliveryVerificationEvalAttemptResult::Cancelled => {
            return Err(DeliveryVerificationEvalCensor::Cancelled)
        }
        DeliveryVerificationEvalAttemptResult::Steered => {
            return Err(DeliveryVerificationEvalCensor::Steered)
        }
        DeliveryVerificationEvalAttemptResult::VerifierResponse(_) => {
            return Err(DeliveryVerificationEvalCensor::InvalidAttemptResult)
        }
    };
    let repaired_subject = DeliveryVerificationSubjectV1::bind(
        &input.objective,
        repaired_candidate,
        repaired_receipt,
        &input.obligations,
        &input.evidence,
    )
    .map_err(|_| DeliveryVerificationEvalCensor::InvalidRepairBinding)?;
    state
        .record_repair(repaired_subject)
        .map_err(|_| DeliveryVerificationEvalCensor::InvalidRepairBinding)?;

    let recheck = take_attempt(
        attempts,
        &mut cursor,
        DeliveryVerificationEvalStage::Recheck,
        &input.verifier_model,
    )?;
    let recheck_json = match &recheck.result {
        DeliveryVerificationEvalAttemptResult::VerifierResponse(json) => json,
        DeliveryVerificationEvalAttemptResult::ModelFailure { kind } => {
            state
                .fail_closed()
                .map_err(|_| DeliveryVerificationEvalCensor::InvalidStateTransition)?;
            ensure_consumed(attempts, cursor)?;
            return Ok(failed_observation(
                &input,
                &state,
                initial_subject_sha256,
                control,
                1,
                1,
                1,
                DeliveryVerificationEvalStage::Recheck,
                (*kind).into(),
                &telemetry,
            ));
        }
        DeliveryVerificationEvalAttemptResult::Cancelled => {
            return Err(DeliveryVerificationEvalCensor::Cancelled)
        }
        DeliveryVerificationEvalAttemptResult::Steered => {
            return Err(DeliveryVerificationEvalCensor::Steered)
        }
        DeliveryVerificationEvalAttemptResult::OwnerRepair { .. } => {
            return Err(DeliveryVerificationEvalCensor::InvalidAttemptResult)
        }
    };
    let recheck_verdict =
        match DeliveryVerificationVerdictV1::from_json(state.subject(), recheck_json) {
            Ok(verdict) => verdict,
            Err(_) => {
                state
                    .fail_closed()
                    .map_err(|_| DeliveryVerificationEvalCensor::InvalidStateTransition)?;
                ensure_consumed(attempts, cursor)?;
                return Ok(failed_observation(
                    &input,
                    &state,
                    initial_subject_sha256,
                    control,
                    1,
                    1,
                    1,
                    DeliveryVerificationEvalStage::Recheck,
                    DeliveryVerificationTreatmentFailureCode::InvalidVerifierResponse,
                    &telemetry,
                ));
            }
        };
    let recheck_decision = recheck_verdict.decision;
    telemetry.recheck_decision = Some(recheck_decision);
    telemetry.recheck_finding_counts = finding_counts(&recheck_verdict);
    state
        .record_recheck(recheck_verdict)
        .map_err(|_| DeliveryVerificationEvalCensor::InvalidStateTransition)?;
    ensure_consumed(attempts, cursor)?;

    if recheck_decision == DeliveryVerificationDecision::Passed {
        Ok(DeliveryVerificationEvalObservation {
            schema: DELIVERY_VERIFICATION_EVAL_SCHEMA,
            evaluation_id: input.evaluation_id,
            initial_subject_sha256,
            final_subject_sha256: state.subject().subject_sha256.clone(),
            owner_model: input.owner_model,
            verifier_model: input.verifier_model,
            control,
            treatment: DeliveryVerificationArmObservation::completed(repaired_candidate),
            disposition: DeliveryVerificationTreatmentDisposition::PassedAfterRepair,
            terminal_status: state.status(),
            initial_verifier_calls: 1,
            owner_repair_calls: 1,
            recheck_calls: 1,
            failure_stage: None,
            failure_code: None,
            initial_verifier_decision: telemetry.initial_verifier_decision,
            initial_finding_counts: telemetry.initial_finding_counts,
            repair_activated: telemetry.repair_activated,
            recheck_decision: telemetry.recheck_decision,
            recheck_finding_counts: telemetry.recheck_finding_counts,
        })
    } else {
        Ok(failed_observation(
            &input,
            &state,
            initial_subject_sha256,
            control,
            1,
            1,
            1,
            DeliveryVerificationEvalStage::Recheck,
            DeliveryVerificationTreatmentFailureCode::VerificationNotSatisfied,
            &telemetry,
        ))
    }
}

fn take_attempt<'a>(
    attempts: &'a [DeliveryVerificationEvalAttempt],
    cursor: &mut usize,
    expected_stage: DeliveryVerificationEvalStage,
    expected_model: &str,
) -> Result<&'a DeliveryVerificationEvalAttempt, DeliveryVerificationEvalCensor> {
    let attempt = attempts
        .get(*cursor)
        .ok_or(DeliveryVerificationEvalCensor::MissingAttempt)?;
    *cursor += 1;
    if attempt.stage != expected_stage {
        return Err(DeliveryVerificationEvalCensor::UnexpectedAttempt);
    }
    if attempt.configured_model.trim() != expected_model.trim() {
        return Err(DeliveryVerificationEvalCensor::WrongModel);
    }
    if attempt.tool_count != 0 {
        return Err(DeliveryVerificationEvalCensor::ToolAuthorityEscaped);
    }
    Ok(attempt)
}

fn ensure_consumed(
    attempts: &[DeliveryVerificationEvalAttempt],
    cursor: usize,
) -> Result<(), DeliveryVerificationEvalCensor> {
    if cursor == attempts.len() {
        Ok(())
    } else {
        Err(DeliveryVerificationEvalCensor::UnexpectedAttempt)
    }
}

#[allow(clippy::too_many_arguments)]
fn failed_observation(
    input: &DeliveryVerificationEvalInput,
    state: &DeliveryVerificationStateV1,
    initial_subject_sha256: String,
    control: DeliveryVerificationArmObservation,
    initial_verifier_calls: usize,
    owner_repair_calls: usize,
    recheck_calls: usize,
    failure_stage: DeliveryVerificationEvalStage,
    failure_code: DeliveryVerificationTreatmentFailureCode,
    telemetry: &EvaluationTelemetry,
) -> DeliveryVerificationEvalObservation {
    DeliveryVerificationEvalObservation {
        schema: DELIVERY_VERIFICATION_EVAL_SCHEMA,
        evaluation_id: input.evaluation_id.clone(),
        initial_subject_sha256,
        final_subject_sha256: state.subject().subject_sha256.clone(),
        owner_model: input.owner_model.clone(),
        verifier_model: input.verifier_model.clone(),
        control,
        treatment: DeliveryVerificationArmObservation::failed(),
        disposition: DeliveryVerificationTreatmentDisposition::TreatmentFailure,
        terminal_status: state.status(),
        initial_verifier_calls,
        owner_repair_calls,
        recheck_calls,
        failure_stage: Some(failure_stage),
        failure_code: Some(failure_code),
        initial_verifier_decision: telemetry.initial_verifier_decision,
        initial_finding_counts: telemetry.initial_finding_counts,
        repair_activated: telemetry.repair_activated,
        recheck_decision: telemetry.recheck_decision,
        recheck_finding_counts: telemetry.recheck_finding_counts,
    }
}

fn finding_counts(verdict: &DeliveryVerificationVerdictV1) -> DeliveryVerificationFindingCounts {
    let mut counts = DeliveryVerificationFindingCounts::default();
    for finding in &verdict.findings {
        match finding.kind {
            DeliveryVerificationFindingKind::UnsupportedClaim => counts.unsupported_claim += 1,
            DeliveryVerificationFindingKind::OmittedObligation => counts.omitted_obligation += 1,
            DeliveryVerificationFindingKind::Contradiction => counts.contradiction += 1,
        }
    }
    counts
}
