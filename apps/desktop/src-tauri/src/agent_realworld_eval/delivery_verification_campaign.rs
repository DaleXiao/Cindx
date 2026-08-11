use super::delivery_verification::{
    project_delivery_verification_evaluation, DeliveryVerificationEvalAttempt,
    DeliveryVerificationEvalAttemptResult, DeliveryVerificationEvalCensor,
    DeliveryVerificationEvalInput, DeliveryVerificationEvalObservation,
    DeliveryVerificationEvalStage, DeliveryVerificationModelFailure,
    DeliveryVerificationTreatmentDisposition,
};
use super::delivery_verification_execution_journal::{
    DeliveryVerificationCallStageV1, DeliveryVerificationCampaignDispositionV1,
};
use super::delivery_verification_protocol::{
    calibration_decision, holdout_decision, CalibrationDecision, HoldoutDecisionResult,
    MatchedPairCounts, OracleEvaluation, ProtocolBudget, ValidatedCase, ValidatedProtocol,
};
use super::delivery_verification_requests::{
    prepare_owner_draft_request, prepare_owner_repair_request, prepare_verifier_request,
    DeliveryOwnerDraftRequestInput, DeliveryRepairRequestInput, DeliveryVerificationRequestBudget,
    DeliveryVerificationRequestInput, MAX_DELIVERY_VERIFICATION_REQUEST_BYTES,
};
use agent_core::{ModelRequest, ModelRole};
use agent_runtime::{
    DeliveryVerificationDecision, DeliveryVerificationSubjectV1, DeliveryVerificationVerdictV1,
    GroundedCompletionBasis, GroundedCompletionReceipt, GROUNDED_COMPLETION_SCHEMA,
};
use orchestrator::sha256_hex;

#[derive(Debug, Clone)]
pub(super) struct PreparedDeliveryCall {
    pub(super) case_ordinal: usize,
    pub(super) stage: DeliveryVerificationCallStageV1,
    pub(super) role: ModelRole,
    pub(super) configured_model: String,
    pub(super) canonical_request_sha256: String,
    pub(super) canonical_request_bytes: u64,
    pub(super) max_output_tokens: u64,
    pub(super) request: ModelRequest,
}

#[derive(Debug, Clone)]
pub(super) struct CompletedCall {
    pub(super) content: String,
    pub(super) served_model_sha256: String,
}

#[derive(Debug, Clone)]
pub(super) enum CallOutcome {
    Completed(CompletedCall),
    ModelFailure(DeliveryVerificationModelFailure),
    StructuralFailure(String),
    Censored(String),
}

pub(super) trait DeliveryVerificationRuntime {
    fn configured_model(&self, role: &ModelRole) -> &str;
    fn begin_case(&mut self, ordinal: usize) -> Result<(), String>;
    fn dispatch(&mut self, call: PreparedDeliveryCall) -> CallOutcome;
    fn record_case(&mut self, outcome: &CaseOutcome) -> Result<(), String>;
    fn record_calibration(
        &mut self,
        decision: CalibrationDecision,
        counts: MatchedPairCounts,
    ) -> Result<(), String>;
    fn record_holdout(
        &mut self,
        decision: HoldoutDecisionResult,
        counts: MatchedPairCounts,
    ) -> Result<(), String>;
    fn finish(
        &mut self,
        disposition: DeliveryVerificationCampaignDispositionV1,
        reason: &str,
        evidence_sha256: String,
    ) -> Result<(), String>;
    fn is_terminal(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CaseStatus {
    Complete,
    StructuralFailure,
    TreatmentExecutionFailure,
    Censored,
}

#[derive(Debug, Clone)]
pub(super) struct CaseOutcome {
    pub(super) ordinal: usize,
    pub(super) status: CaseStatus,
    pub(super) owner_draft_sha256: Option<String>,
    pub(super) owner_draft_bytes: Option<u64>,
    pub(super) control_output_sha256: Option<String>,
    pub(super) treatment_output_sha256: Option<String>,
    pub(super) control_passed: Option<bool>,
    pub(super) treatment_passed: Option<bool>,
    pub(super) observation_sha256: String,
    pub(super) reason: String,
}

impl CaseOutcome {
    fn counts(&self) -> MatchedPairCounts {
        let mut counts = zero_counts();
        match self.status {
            CaseStatus::Complete => {
                counts.complete_cases = 1;
                let control = self.control_passed == Some(true);
                let treatment = self.treatment_passed == Some(true);
                counts.control_failures = usize::from(!control);
                counts.treatment_only_wins = usize::from(!control && treatment);
                counts.control_only_losses = usize::from(control && !treatment);
            }
            CaseStatus::StructuralFailure | CaseStatus::Censored => {
                counts.structural_failures = 1;
            }
            CaseStatus::TreatmentExecutionFailure => {
                counts.treatment_execution_failures = 1;
            }
        }
        counts
    }
}

pub(super) fn execute_fixed_campaign(
    protocol: &ValidatedProtocol<'_>,
    runtime: &mut impl DeliveryVerificationRuntime,
) -> Result<DeliveryVerificationCampaignDispositionV1, String> {
    let mut calibration = zero_counts();
    for case in protocol.calibration_cases() {
        runtime.begin_case(case.ordinal())?;
        let outcome = execute_case(case, protocol.budget(), runtime);
        runtime.record_case(&outcome)?;
        add_counts(&mut calibration, outcome.counts())?;
        if runtime.is_terminal() {
            return Ok(DeliveryVerificationCampaignDispositionV1::Inconclusive);
        }
        if outcome.status != CaseStatus::Complete {
            return freeze_for_case_failure(runtime, &outcome);
        }
    }
    let calibration_result = calibration_decision(calibration);
    runtime.record_calibration(calibration_result, calibration)?;
    if calibration_result != CalibrationDecision::OpenHoldout {
        let disposition = match calibration_result {
            CalibrationDecision::TerminalFutility => {
                DeliveryVerificationCampaignDispositionV1::TerminalFutility
            }
            CalibrationDecision::Inconclusive => {
                DeliveryVerificationCampaignDispositionV1::Inconclusive
            }
            CalibrationDecision::Invalid => DeliveryVerificationCampaignDispositionV1::Invalid,
            CalibrationDecision::OpenHoldout => unreachable!(),
        };
        runtime.finish(
            disposition,
            calibration_label(calibration_result),
            counts_digest("calibration", calibration),
        )?;
        return Ok(disposition);
    }

    let mut holdout = zero_counts();
    for case in protocol.holdout_cases() {
        runtime.begin_case(case.ordinal())?;
        let outcome = execute_case(case, protocol.budget(), runtime);
        runtime.record_case(&outcome)?;
        add_counts(&mut holdout, outcome.counts())?;
        if runtime.is_terminal() {
            return Ok(DeliveryVerificationCampaignDispositionV1::Inconclusive);
        }
        if outcome.status != CaseStatus::Complete {
            return freeze_for_case_failure(runtime, &outcome);
        }
    }
    let holdout_result = holdout_decision(holdout);
    runtime.record_holdout(holdout_result, holdout)?;
    let disposition = match holdout_result {
        HoldoutDecisionResult::EvidenceOfUplift => {
            DeliveryVerificationCampaignDispositionV1::EvidenceOfUplift
        }
        HoldoutDecisionResult::NoEvidence => DeliveryVerificationCampaignDispositionV1::NoEvidence,
        HoldoutDecisionResult::Regression => DeliveryVerificationCampaignDispositionV1::Regression,
        HoldoutDecisionResult::Inconclusive => {
            DeliveryVerificationCampaignDispositionV1::Inconclusive
        }
        HoldoutDecisionResult::Invalid => DeliveryVerificationCampaignDispositionV1::Invalid,
    };
    runtime.finish(
        disposition,
        holdout_label(holdout_result),
        counts_digest("holdout", holdout),
    )?;
    Ok(disposition)
}

fn freeze_for_case_failure(
    runtime: &mut impl DeliveryVerificationRuntime,
    outcome: &CaseOutcome,
) -> Result<DeliveryVerificationCampaignDispositionV1, String> {
    let disposition = match outcome.status {
        CaseStatus::Censored => DeliveryVerificationCampaignDispositionV1::Censored,
        CaseStatus::StructuralFailure | CaseStatus::TreatmentExecutionFailure => {
            DeliveryVerificationCampaignDispositionV1::Inconclusive
        }
        CaseStatus::Complete => return Err("complete case cannot freeze the campaign".into()),
    };
    runtime.finish(
        disposition,
        &outcome.reason,
        sha256_hex(outcome.observation_sha256.as_bytes()),
    )?;
    Ok(disposition)
}

pub(super) fn execute_case(
    case: ValidatedCase<'_>,
    budget: &ProtocolBudget,
    runtime: &mut impl DeliveryVerificationRuntime,
) -> CaseOutcome {
    let owner_model = runtime.configured_model(&ModelRole::Executor).to_string();
    let verifier_model = runtime.configured_model(&ModelRole::Reviewer).to_string();
    let owner_call = match owner_draft_call(case, budget, &owner_model) {
        Ok(call) => call,
        Err(error) => return structural_case(case.ordinal(), None, &error),
    };
    let owner = match runtime.dispatch(owner_call) {
        CallOutcome::Completed(output) => output,
        CallOutcome::Censored(reason) => return censored_case(case.ordinal(), None, &reason),
        CallOutcome::ModelFailure(failure) => {
            return structural_case(case.ordinal(), None, model_failure_label(failure))
        }
        CallOutcome::StructuralFailure(reason) => {
            return structural_case(case.ordinal(), None, &reason)
        }
    };
    let owner_receipt = grounded_receipt(case, &owner.content, 1);
    let subject = match DeliveryVerificationSubjectV1::bind(
        case.objective(),
        &owner.content,
        &owner_receipt,
        case.obligations(),
        case.evidence(),
    ) {
        Ok(subject) => subject,
        Err(_) => return structural_case(case.ordinal(), Some(&owner.content), "owner_binding"),
    };
    let mut attempts = Vec::with_capacity(3);
    let initial_verifier_call = match verifier_call(
        case,
        budget,
        &owner_model,
        &verifier_model,
        &subject,
        &owner.content,
        DeliveryVerificationCallStageV1::VerifierInitial,
    ) {
        Ok(call) => call,
        Err(error) => return structural_case(case.ordinal(), Some(&owner.content), &error),
    };
    let initial = match runtime.dispatch(initial_verifier_call) {
        CallOutcome::Completed(output) => output,
        other => {
            return attempt_failure_case(
                case,
                &owner_model,
                &verifier_model,
                owner,
                attempts,
                DeliveryVerificationEvalStage::InitialVerification,
                other,
            )
        }
    };
    if initial.served_model_sha256 == owner.served_model_sha256 {
        return structural_case(
            case.ordinal(),
            Some(&owner.content),
            "served_models_not_independent",
        );
    }
    attempts.push(DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::InitialVerification,
        configured_model: verifier_model.clone(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::VerifierResponse(initial.content.clone()),
    });
    let verdict = match DeliveryVerificationVerdictV1::from_json(&subject, &initial.content) {
        Ok(verdict) => verdict,
        Err(_) => return project_case(case, &owner_model, &verifier_model, owner, attempts),
    };
    if verdict.decision == DeliveryVerificationDecision::Passed {
        return project_case(case, &owner_model, &verifier_model, owner, attempts);
    }

    let repair_call = match repair_call(
        case,
        budget,
        &owner_model,
        &verifier_model,
        &subject,
        &owner.content,
        &verdict,
    ) {
        Ok(call) => call,
        Err(error) => return structural_case(case.ordinal(), Some(&owner.content), &error),
    };
    let repaired = match runtime.dispatch(repair_call) {
        CallOutcome::Completed(output) => output,
        other => {
            return attempt_failure_case(
                case,
                &owner_model,
                &verifier_model,
                owner,
                attempts,
                DeliveryVerificationEvalStage::OwnerRepair,
                other,
            )
        }
    };
    if repaired.served_model_sha256 != owner.served_model_sha256 {
        return structural_case(
            case.ordinal(),
            Some(&owner.content),
            "owner_served_model_changed",
        );
    }
    let repaired_receipt = grounded_receipt(case, &repaired.content, 2);
    let repaired_subject = match DeliveryVerificationSubjectV1::bind(
        case.objective(),
        &repaired.content,
        &repaired_receipt,
        case.obligations(),
        case.evidence(),
    ) {
        Ok(subject) => subject,
        Err(_) => return structural_case(case.ordinal(), Some(&owner.content), "repair_binding"),
    };
    attempts.push(DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::OwnerRepair,
        configured_model: owner_model.clone(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::OwnerRepair {
            candidate: repaired.content.clone(),
            receipt: repaired_receipt,
        },
    });
    let recheck_call = match verifier_call(
        case,
        budget,
        &owner_model,
        &verifier_model,
        &repaired_subject,
        &repaired.content,
        DeliveryVerificationCallStageV1::VerifierRecheck,
    ) {
        Ok(call) => call,
        Err(error) => return structural_case(case.ordinal(), Some(&owner.content), &error),
    };
    let recheck = match runtime.dispatch(recheck_call) {
        CallOutcome::Completed(output) => output,
        other => {
            return attempt_failure_case(
                case,
                &owner_model,
                &verifier_model,
                owner,
                attempts,
                DeliveryVerificationEvalStage::Recheck,
                other,
            )
        }
    };
    if recheck.served_model_sha256 != initial.served_model_sha256 {
        return structural_case(
            case.ordinal(),
            Some(&owner.content),
            "verifier_served_model_changed",
        );
    }
    attempts.push(DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::Recheck,
        configured_model: verifier_model.clone(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::VerifierResponse(recheck.content),
    });
    project_case(case, &owner_model, &verifier_model, owner, attempts)
}

fn owner_draft_call(
    case: ValidatedCase<'_>,
    budget: &ProtocolBudget,
    owner_model: &str,
) -> Result<PreparedDeliveryCall, String> {
    let prepared = prepare_owner_draft_request(DeliveryOwnerDraftRequestInput {
        objective: case.objective(),
        obligations: case.obligations(),
        evidence: case.evidence(),
        owner_model,
        budget: request_budget(budget.max_owner_output_tokens),
    })?;
    let binding = prepared.binding().clone();
    Ok(PreparedDeliveryCall {
        case_ordinal: case.ordinal(),
        stage: DeliveryVerificationCallStageV1::OwnerDraft,
        role: ModelRole::Executor,
        configured_model: owner_model.to_string(),
        canonical_request_sha256: binding.canonical_request_sha256,
        canonical_request_bytes: binding.canonical_request_bytes,
        max_output_tokens: budget.max_owner_output_tokens,
        request: prepared.into_request(),
    })
}

fn verifier_call(
    case: ValidatedCase<'_>,
    budget: &ProtocolBudget,
    owner_model: &str,
    verifier_model: &str,
    subject: &DeliveryVerificationSubjectV1,
    draft: &str,
    stage: DeliveryVerificationCallStageV1,
) -> Result<PreparedDeliveryCall, String> {
    let prepared = prepare_verifier_request(DeliveryVerificationRequestInput {
        subject,
        objective: case.objective(),
        obligations: case.obligations(),
        evidence: case.evidence(),
        owner_draft: draft,
        owner_model,
        verifier_model,
        budget: request_budget(budget.max_verifier_output_tokens),
    })?;
    let binding = prepared.binding().clone();
    Ok(PreparedDeliveryCall {
        case_ordinal: case.ordinal(),
        stage,
        role: ModelRole::Reviewer,
        configured_model: verifier_model.to_string(),
        canonical_request_sha256: binding.canonical_request_sha256,
        canonical_request_bytes: binding.canonical_request_bytes,
        max_output_tokens: budget.max_verifier_output_tokens,
        request: prepared.into_request(),
    })
}

#[allow(clippy::too_many_arguments)]
fn repair_call(
    case: ValidatedCase<'_>,
    budget: &ProtocolBudget,
    owner_model: &str,
    verifier_model: &str,
    subject: &DeliveryVerificationSubjectV1,
    draft: &str,
    verdict: &DeliveryVerificationVerdictV1,
) -> Result<PreparedDeliveryCall, String> {
    let prepared = prepare_owner_repair_request(DeliveryRepairRequestInput {
        subject,
        objective: case.objective(),
        obligations: case.obligations(),
        evidence: case.evidence(),
        owner_draft: draft,
        verdict,
        owner_role: ModelRole::Executor,
        owner_model,
        verifier_model,
        budget: request_budget(budget.max_owner_output_tokens),
    })?;
    let binding = prepared.binding().clone();
    Ok(PreparedDeliveryCall {
        case_ordinal: case.ordinal(),
        stage: DeliveryVerificationCallStageV1::OwnerRepair,
        role: ModelRole::Executor,
        configured_model: owner_model.to_string(),
        canonical_request_sha256: binding.canonical_request_sha256,
        canonical_request_bytes: binding.canonical_request_bytes,
        max_output_tokens: budget.max_owner_output_tokens,
        request: prepared.into_request(),
    })
}

fn request_budget(max_output_tokens: u64) -> DeliveryVerificationRequestBudget {
    DeliveryVerificationRequestBudget {
        max_request_bytes: MAX_DELIVERY_VERIFICATION_REQUEST_BYTES,
        max_output_tokens,
    }
}

fn grounded_receipt(
    case: ValidatedCase<'_>,
    content: &str,
    model_turn: u64,
) -> GroundedCompletionReceipt {
    GroundedCompletionReceipt {
        schema: GROUNDED_COMPLETION_SCHEMA.to_string(),
        steer_epoch: 0,
        contract_epoch: 0,
        model_turn,
        content_sha256: sha256_hex(content.as_bytes()),
        content_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
        obligation_digest: sha256_hex(&case.model_input_bytes().unwrap_or_else(|_| Vec::new())),
        covered_obligation_ids: case
            .obligations()
            .iter()
            .map(|item| item.obligation_ref.clone())
            .collect(),
        visible_evidence_sequences: case
            .evidence()
            .iter()
            .map(|item| item.evidence_ref)
            .collect(),
        constraint_codes: Vec::new(),
        basis: GroundedCompletionBasis::EvidenceVisible,
    }
}

#[allow(clippy::too_many_arguments)]
fn attempt_failure_case(
    case: ValidatedCase<'_>,
    owner_model: &str,
    verifier_model: &str,
    owner: CompletedCall,
    mut attempts: Vec<DeliveryVerificationEvalAttempt>,
    stage: DeliveryVerificationEvalStage,
    outcome: CallOutcome,
) -> CaseOutcome {
    match outcome {
        CallOutcome::ModelFailure(kind) => {
            attempts.push(DeliveryVerificationEvalAttempt {
                stage,
                configured_model: match stage {
                    DeliveryVerificationEvalStage::OwnerRepair => owner_model.to_string(),
                    _ => verifier_model.to_string(),
                },
                tool_count: 0,
                result: DeliveryVerificationEvalAttemptResult::ModelFailure { kind },
            });
            project_case(case, owner_model, verifier_model, owner, attempts)
        }
        CallOutcome::Censored(reason) => {
            censored_case(case.ordinal(), Some(&owner.content), &reason)
        }
        CallOutcome::StructuralFailure(reason) => {
            structural_case(case.ordinal(), Some(&owner.content), &reason)
        }
        CallOutcome::Completed(_) => structural_case(
            case.ordinal(),
            Some(&owner.content),
            "invalid_dispatch_transition",
        ),
    }
}

fn project_case(
    case: ValidatedCase<'_>,
    owner_model: &str,
    verifier_model: &str,
    owner: CompletedCall,
    attempts: Vec<DeliveryVerificationEvalAttempt>,
) -> CaseOutcome {
    match project_delivery_verification_evaluation(
        DeliveryVerificationEvalInput {
            evaluation_id: case.id().to_string(),
            objective: case.objective().to_string(),
            obligations: case.obligations().to_vec(),
            evidence: case.evidence().to_vec(),
            owner_draft: owner.content.clone(),
            owner_receipt: grounded_receipt(case, &owner.content, 1),
            owner_model: owner_model.to_string(),
            verifier_model: verifier_model.to_string(),
        },
        &attempts,
    ) {
        Ok(observation) => completed_or_treatment_failure(case, observation),
        Err(censor) => structural_case(
            case.ordinal(),
            Some(&owner.content),
            eval_censor_label(censor),
        ),
    }
}

fn completed_or_treatment_failure(
    case: ValidatedCase<'_>,
    observation: DeliveryVerificationEvalObservation,
) -> CaseOutcome {
    let owner = observation.control.output.as_deref();
    let treatment = observation.treatment.output.as_deref();
    let owner_sha = owner.map(|value| sha256_hex(value.as_bytes()));
    let treatment_sha = treatment.map(|value| sha256_hex(value.as_bytes()));
    let status =
        if observation.disposition == DeliveryVerificationTreatmentDisposition::TreatmentFailure {
            CaseStatus::TreatmentExecutionFailure
        } else {
            CaseStatus::Complete
        };
    let control_passed = owner.map(|value| case.evaluate_output(value) == OracleEvaluation::Passed);
    let treatment_passed =
        treatment.map(|value| case.evaluate_output(value) == OracleEvaluation::Passed);
    let reason = observation
        .failure_code
        .map(|value| format!("treatment_failure:{value:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "complete".into());
    let observation_sha256 = sha256_hex(
        format!(
            "cindx.delivery-verification-case-observation.v1\0{}\0{status:?}\0{}\0{}\0{control_passed:?}\0{treatment_passed:?}\0{reason}",
            case.id(),
            owner_sha.as_deref().unwrap_or("none"),
            treatment_sha.as_deref().unwrap_or("none"),
        )
        .as_bytes(),
    );
    CaseOutcome {
        ordinal: case.ordinal(),
        status,
        owner_draft_sha256: owner_sha.clone(),
        owner_draft_bytes: owner.map(|value| value.len() as u64),
        control_output_sha256: owner_sha,
        treatment_output_sha256: treatment_sha,
        control_passed,
        treatment_passed,
        observation_sha256,
        reason,
    }
}

fn structural_case(ordinal: usize, owner: Option<&str>, reason: &str) -> CaseOutcome {
    failed_case(ordinal, owner, CaseStatus::StructuralFailure, reason)
}

fn censored_case(ordinal: usize, owner: Option<&str>, reason: &str) -> CaseOutcome {
    failed_case(ordinal, owner, CaseStatus::Censored, reason)
}

fn failed_case(
    ordinal: usize,
    owner: Option<&str>,
    status: CaseStatus,
    reason: &str,
) -> CaseOutcome {
    let owner_sha = owner.map(|value| sha256_hex(value.as_bytes()));
    CaseOutcome {
        ordinal,
        status,
        owner_draft_sha256: owner_sha.clone(),
        owner_draft_bytes: owner.map(|value| value.len() as u64),
        control_output_sha256: owner_sha,
        treatment_output_sha256: None,
        control_passed: None,
        treatment_passed: None,
        observation_sha256: sha256_hex(
            format!("cindx.delivery-verification-case-failure.v1\0{ordinal}\0{status:?}\0{reason}")
                .as_bytes(),
        ),
        reason: reason.to_string(),
    }
}

fn zero_counts() -> MatchedPairCounts {
    MatchedPairCounts {
        complete_cases: 0,
        control_failures: 0,
        treatment_only_wins: 0,
        control_only_losses: 0,
        structural_failures: 0,
        treatment_execution_failures: 0,
    }
}

fn add_counts(total: &mut MatchedPairCounts, one: MatchedPairCounts) -> Result<(), String> {
    let add = |left: usize, right: usize| {
        left.checked_add(right)
            .ok_or_else(|| "delivery verification count overflowed".to_string())
    };
    total.complete_cases = add(total.complete_cases, one.complete_cases)?;
    total.control_failures = add(total.control_failures, one.control_failures)?;
    total.treatment_only_wins = add(total.treatment_only_wins, one.treatment_only_wins)?;
    total.control_only_losses = add(total.control_only_losses, one.control_only_losses)?;
    total.structural_failures = add(total.structural_failures, one.structural_failures)?;
    total.treatment_execution_failures = add(
        total.treatment_execution_failures,
        one.treatment_execution_failures,
    )?;
    Ok(())
}

pub(super) fn counts_digest(stage: &str, counts: MatchedPairCounts) -> String {
    sha256_hex(
        format!(
            "cindx.delivery-verification-counts.v1\0{stage}\0{}\0{}\0{}\0{}\0{}\0{}",
            counts.complete_cases,
            counts.control_failures,
            counts.treatment_only_wins,
            counts.control_only_losses,
            counts.structural_failures,
            counts.treatment_execution_failures,
        )
        .as_bytes(),
    )
}

pub(super) fn calibration_label(value: CalibrationDecision) -> &'static str {
    match value {
        CalibrationDecision::OpenHoldout => "open_holdout",
        CalibrationDecision::TerminalFutility => "terminal_futility",
        CalibrationDecision::Inconclusive => "inconclusive",
        CalibrationDecision::Invalid => "invalid",
    }
}

pub(super) fn holdout_label(value: HoldoutDecisionResult) -> &'static str {
    match value {
        HoldoutDecisionResult::EvidenceOfUplift => "evidence_of_uplift",
        HoldoutDecisionResult::NoEvidence => "no_evidence",
        HoldoutDecisionResult::Regression => "regression",
        HoldoutDecisionResult::Inconclusive => "inconclusive",
        HoldoutDecisionResult::Invalid => "invalid",
    }
}

fn model_failure_label(value: DeliveryVerificationModelFailure) -> &'static str {
    match value {
        DeliveryVerificationModelFailure::ProviderUnavailable => "owner_provider_unavailable",
        DeliveryVerificationModelFailure::Timeout => "owner_timeout",
        DeliveryVerificationModelFailure::BudgetExhausted => "owner_budget_exhausted",
    }
}

fn eval_censor_label(value: DeliveryVerificationEvalCensor) -> &'static str {
    match value {
        DeliveryVerificationEvalCensor::Cancelled => "evaluation_cancelled",
        DeliveryVerificationEvalCensor::Steered => "evaluation_steered",
        _ => "invalid_evaluation_projection",
    }
}
