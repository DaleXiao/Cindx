use super::delivery_verification::{
    project_delivery_verification_evaluation, DeliveryVerificationEvalAttempt,
    DeliveryVerificationEvalAttemptResult, DeliveryVerificationEvalCensor,
    DeliveryVerificationEvalInput, DeliveryVerificationEvalObservation,
    DeliveryVerificationEvalStage, DeliveryVerificationFindingCounts,
    DeliveryVerificationModelFailure, DeliveryVerificationTreatmentDisposition,
};
use super::delivery_verification_execution_journal::{
    DeliveryVerificationCallStageV1, DeliveryVerificationCampaignDispositionV1,
};
use super::delivery_verification_protocol::{
    calibration_decision, holdout_decision, CalibrationDecision, DeliveryVerificationStratum,
    HoldoutDecisionResult, MatchedPairCounts, OracleEvaluation, ProtocolBudget, ValidatedCase,
    ValidatedProtocol,
};
use super::delivery_verification_requests::{
    prepare_owner_repair_request, prepare_verifier_request, DeliveryRepairRequestInput,
    DeliveryVerificationRequestBudget, DeliveryVerificationRequestInput,
    MAX_DELIVERY_VERIFICATION_REQUEST_BYTES,
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
    Censored,
}

#[derive(Debug, Clone)]
pub(super) struct CaseOutcome {
    pub(super) ordinal: usize,
    pub(super) stratum: DeliveryVerificationStratum,
    pub(super) status: CaseStatus,
    pub(super) seeded_candidate_sha256: Option<String>,
    pub(super) seeded_candidate_bytes: Option<u64>,
    pub(super) control_output_sha256: Option<String>,
    pub(super) treatment_output_sha256: Option<String>,
    pub(super) control_passed: Option<bool>,
    pub(super) treatment_passed: Option<bool>,
    pub(super) initial_verifier_decision: Option<String>,
    pub(super) initial_finding_counts: DeliveryVerificationFindingCounts,
    pub(super) repair_activated: bool,
    pub(super) recheck_decision: Option<String>,
    pub(super) recheck_finding_counts: DeliveryVerificationFindingCounts,
    pub(super) treatment_disposition: Option<String>,
    pub(super) failure_stage: Option<String>,
    pub(super) failure_code: Option<String>,
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
                if !control && treatment {
                    match self.stratum {
                        DeliveryVerificationStratum::UnsupportedClaim => {
                            counts.unsupported_claim_wins = 1
                        }
                        DeliveryVerificationStratum::OmittedObligation => {
                            counts.omitted_obligation_wins = 1
                        }
                        DeliveryVerificationStratum::Contradiction => counts.contradiction_wins = 1,
                        DeliveryVerificationStratum::Preservation => {}
                    }
                }
                if control
                    && !treatment
                    && self.stratum == DeliveryVerificationStratum::Preservation
                {
                    counts.preservation_losses = 1;
                }
            }
            CaseStatus::StructuralFailure | CaseStatus::Censored => {
                counts.structural_failures = 1;
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
        HoldoutDecisionResult::SeededRepairEffective => {
            DeliveryVerificationCampaignDispositionV1::SeededRepairEffective
        }
        HoldoutDecisionResult::NotEffective => {
            DeliveryVerificationCampaignDispositionV1::NotEffective
        }
        HoldoutDecisionResult::PreservationRegression => {
            DeliveryVerificationCampaignDispositionV1::PreservationRegression
        }
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
        CaseStatus::StructuralFailure => DeliveryVerificationCampaignDispositionV1::Inconclusive,
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
    let seeded_candidate = case.seeded_candidate().to_string();
    let owner_receipt = grounded_receipt(case, &seeded_candidate, 0);
    let subject = match DeliveryVerificationSubjectV1::bind(
        case.objective(),
        &seeded_candidate,
        &owner_receipt,
        case.obligations(),
        case.evidence(),
    ) {
        Ok(subject) => subject,
        Err(_) => return structural_case(case, Some(&seeded_candidate), "seed_binding"),
    };
    let mut attempts = Vec::with_capacity(3);
    let initial_verifier_call = match verifier_call(
        case,
        budget,
        &owner_model,
        &verifier_model,
        &subject,
        &seeded_candidate,
        DeliveryVerificationCallStageV1::VerifierInitial,
    ) {
        Ok(call) => call,
        Err(error) => return structural_case(case, Some(&seeded_candidate), &error),
    };
    let initial = match runtime.dispatch(initial_verifier_call) {
        CallOutcome::Completed(output) => output,
        other => {
            return attempt_failure_case(
                case,
                &owner_model,
                &verifier_model,
                seeded_candidate,
                attempts,
                DeliveryVerificationEvalStage::InitialVerification,
                other,
            )
        }
    };
    attempts.push(DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::InitialVerification,
        configured_model: verifier_model.clone(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::VerifierResponse(initial.content.clone()),
    });
    let verdict = match DeliveryVerificationVerdictV1::from_json(&subject, &initial.content) {
        Ok(verdict) => verdict,
        Err(_) => {
            return project_case(
                case,
                &owner_model,
                &verifier_model,
                seeded_candidate,
                attempts,
            )
        }
    };
    if verdict.decision == DeliveryVerificationDecision::Passed {
        return project_case(
            case,
            &owner_model,
            &verifier_model,
            seeded_candidate,
            attempts,
        );
    }

    let repair_call = match repair_call(
        case,
        budget,
        &owner_model,
        &verifier_model,
        &subject,
        &seeded_candidate,
        &verdict,
    ) {
        Ok(call) => call,
        Err(error) => return structural_case(case, Some(&seeded_candidate), &error),
    };
    let repaired = match runtime.dispatch(repair_call) {
        CallOutcome::Completed(output) => output,
        other => {
            return attempt_failure_case(
                case,
                &owner_model,
                &verifier_model,
                seeded_candidate,
                attempts,
                DeliveryVerificationEvalStage::OwnerRepair,
                other,
            )
        }
    };
    if repaired.served_model_sha256 == initial.served_model_sha256 {
        return structural_case(
            case,
            Some(&seeded_candidate),
            "served_models_not_independent",
        );
    }
    let repaired_receipt = grounded_receipt(case, &repaired.content, 1);
    let repaired_subject = match DeliveryVerificationSubjectV1::bind(
        case.objective(),
        &repaired.content,
        &repaired_receipt,
        case.obligations(),
        case.evidence(),
    ) {
        Ok(subject) => subject,
        Err(_) => return structural_case(case, Some(&seeded_candidate), "repair_binding"),
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
        Err(error) => return structural_case(case, Some(&seeded_candidate), &error),
    };
    let recheck = match runtime.dispatch(recheck_call) {
        CallOutcome::Completed(output) => output,
        other => {
            return attempt_failure_case(
                case,
                &owner_model,
                &verifier_model,
                seeded_candidate,
                attempts,
                DeliveryVerificationEvalStage::Recheck,
                other,
            )
        }
    };
    if recheck.served_model_sha256 != initial.served_model_sha256 {
        return structural_case(
            case,
            Some(&seeded_candidate),
            "verifier_served_model_changed",
        );
    }
    attempts.push(DeliveryVerificationEvalAttempt {
        stage: DeliveryVerificationEvalStage::Recheck,
        configured_model: verifier_model.clone(),
        tool_count: 0,
        result: DeliveryVerificationEvalAttemptResult::VerifierResponse(recheck.content),
    });
    project_case(
        case,
        &owner_model,
        &verifier_model,
        seeded_candidate,
        attempts,
    )
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
        output_contract: case.output_contract(),
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
        output_contract: case.output_contract(),
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
    seeded_candidate: String,
    mut attempts: Vec<DeliveryVerificationEvalAttempt>,
    stage: DeliveryVerificationEvalStage,
    outcome: CallOutcome,
) -> CaseOutcome {
    match outcome {
        CallOutcome::ModelFailure(DeliveryVerificationModelFailure::BudgetExhausted) => {
            structural_case(case, Some(&seeded_candidate), "budget_exhausted")
        }
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
            project_case(
                case,
                owner_model,
                verifier_model,
                seeded_candidate,
                attempts,
            )
        }
        CallOutcome::Censored(reason) => censored_case(case, Some(&seeded_candidate), &reason),
        CallOutcome::StructuralFailure(reason) => {
            structural_case(case, Some(&seeded_candidate), &reason)
        }
        CallOutcome::Completed(_) => {
            structural_case(case, Some(&seeded_candidate), "invalid_dispatch_transition")
        }
    }
}

fn project_case(
    case: ValidatedCase<'_>,
    owner_model: &str,
    verifier_model: &str,
    seeded_candidate: String,
    attempts: Vec<DeliveryVerificationEvalAttempt>,
) -> CaseOutcome {
    match project_delivery_verification_evaluation(
        DeliveryVerificationEvalInput {
            evaluation_id: case.id().to_string(),
            objective: case.objective().to_string(),
            obligations: case.obligations().to_vec(),
            evidence: case.evidence().to_vec(),
            owner_draft: seeded_candidate.clone(),
            owner_receipt: grounded_receipt(case, &seeded_candidate, 0),
            owner_model: owner_model.to_string(),
            verifier_model: verifier_model.to_string(),
        },
        &attempts,
    ) {
        Ok(observation) => completed_or_treatment_failure(case, observation),
        Err(censor) => structural_case(case, Some(&seeded_candidate), eval_censor_label(censor)),
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
    let status = CaseStatus::Complete;
    let control_passed = owner.map(|value| case.evaluate_output(value) == OracleEvaluation::Passed);
    let oracle_treatment_passed = treatment
        .map(|value| case.evaluate_output(value) == OracleEvaluation::Passed)
        .unwrap_or(false);
    let treatment_passed = Some(match case.stratum() {
        DeliveryVerificationStratum::Preservation => {
            observation.disposition == DeliveryVerificationTreatmentDisposition::PassedUnchanged
                && oracle_treatment_passed
                && treatment_sha == owner_sha
        }
        _ => {
            observation.disposition == DeliveryVerificationTreatmentDisposition::PassedAfterRepair
                && oracle_treatment_passed
        }
    });
    let treatment_disposition = disposition_label(observation.disposition).to_string();
    let failure_stage = observation
        .failure_stage
        .map(failure_stage_label)
        .map(str::to_string);
    let failure_code = observation
        .failure_code
        .map(failure_code_label)
        .map(str::to_string);
    let reason = failure_code
        .as_ref()
        .map(|value| format!("component_failure:{value}"))
        .unwrap_or_else(|| match observation.disposition {
            DeliveryVerificationTreatmentDisposition::PassedUnchanged => {
                "verifier_passed_unchanged".into()
            }
            DeliveryVerificationTreatmentDisposition::PassedAfterRepair => "repair_accepted".into(),
            DeliveryVerificationTreatmentDisposition::TreatmentFailure => {
                "component_failure".into()
            }
        });
    let initial_verifier_decision = observation
        .initial_verifier_decision
        .map(decision_label)
        .map(str::to_string);
    let recheck_decision = observation
        .recheck_decision
        .map(decision_label)
        .map(str::to_string);
    let observation_sha256 = sha256_hex(
        format!(
            "cindx.delivery-verification-case-observation.v3\0{}\0{status:?}\0{}\0{}\0{control_passed:?}\0{treatment_passed:?}\0{initial_verifier_decision:?}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{recheck_decision:?}\0{treatment_disposition}\0{failure_stage:?}\0{failure_code:?}\0{}\0{}\0{}\0{reason}",
            case.id(),
            owner_sha.as_deref().unwrap_or("none"),
            treatment_sha.as_deref().unwrap_or("none"),
            observation.initial_finding_counts.unsupported_claim,
            observation.initial_finding_counts.omitted_obligation,
            observation.initial_finding_counts.contradiction,
            observation.repair_activated,
            observation.recheck_finding_counts.unsupported_claim,
            observation.recheck_finding_counts.omitted_obligation,
            observation.recheck_finding_counts.contradiction,
            observation.initial_verifier_calls,
            observation.owner_repair_calls,
            observation.recheck_calls,
        )
        .as_bytes(),
    );
    CaseOutcome {
        ordinal: case.ordinal(),
        stratum: case.stratum(),
        status,
        seeded_candidate_sha256: owner_sha.clone(),
        seeded_candidate_bytes: owner.map(|value| value.len() as u64),
        control_output_sha256: owner_sha,
        treatment_output_sha256: treatment_sha,
        control_passed,
        treatment_passed,
        initial_verifier_decision,
        initial_finding_counts: observation.initial_finding_counts,
        repair_activated: observation.repair_activated,
        recheck_decision,
        recheck_finding_counts: observation.recheck_finding_counts,
        treatment_disposition: Some(treatment_disposition),
        failure_stage,
        failure_code,
        observation_sha256,
        reason,
    }
}

fn structural_case(case: ValidatedCase<'_>, seed: Option<&str>, reason: &str) -> CaseOutcome {
    failed_case(case, seed, CaseStatus::StructuralFailure, reason)
}

fn censored_case(case: ValidatedCase<'_>, seed: Option<&str>, reason: &str) -> CaseOutcome {
    failed_case(case, seed, CaseStatus::Censored, reason)
}

fn failed_case(
    case: ValidatedCase<'_>,
    seed: Option<&str>,
    status: CaseStatus,
    reason: &str,
) -> CaseOutcome {
    let seed_sha = seed.map(|value| sha256_hex(value.as_bytes()));
    CaseOutcome {
        ordinal: case.ordinal(),
        stratum: case.stratum(),
        status,
        seeded_candidate_sha256: seed_sha.clone(),
        seeded_candidate_bytes: seed.map(|value| value.len() as u64),
        control_output_sha256: seed_sha,
        treatment_output_sha256: None,
        control_passed: None,
        treatment_passed: None,
        initial_verifier_decision: None,
        initial_finding_counts: DeliveryVerificationFindingCounts::default(),
        repair_activated: false,
        recheck_decision: None,
        recheck_finding_counts: DeliveryVerificationFindingCounts::default(),
        treatment_disposition: None,
        failure_stage: None,
        failure_code: None,
        observation_sha256: sha256_hex(
            format!(
                "cindx.delivery-verification-case-failure.v3\0{}\0{status:?}\0{reason}",
                case.ordinal()
            )
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
        unsupported_claim_wins: 0,
        omitted_obligation_wins: 0,
        contradiction_wins: 0,
        preservation_losses: 0,
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
    total.unsupported_claim_wins = add(total.unsupported_claim_wins, one.unsupported_claim_wins)?;
    total.omitted_obligation_wins =
        add(total.omitted_obligation_wins, one.omitted_obligation_wins)?;
    total.contradiction_wins = add(total.contradiction_wins, one.contradiction_wins)?;
    total.preservation_losses = add(total.preservation_losses, one.preservation_losses)?;
    Ok(())
}

pub(super) fn counts_digest(stage: &str, counts: MatchedPairCounts) -> String {
    sha256_hex(
        format!(
            "cindx.delivery-verification-counts.v3\0{stage}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
            counts.complete_cases,
            counts.control_failures,
            counts.treatment_only_wins,
            counts.control_only_losses,
            counts.structural_failures,
            counts.treatment_execution_failures,
            counts.unsupported_claim_wins,
            counts.omitted_obligation_wins,
            counts.contradiction_wins,
            counts.preservation_losses,
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
        HoldoutDecisionResult::SeededRepairEffective => "seeded_repair_effective",
        HoldoutDecisionResult::NotEffective => "not_effective",
        HoldoutDecisionResult::PreservationRegression => "preservation_regression",
        HoldoutDecisionResult::Inconclusive => "inconclusive",
        HoldoutDecisionResult::Invalid => "invalid",
    }
}

fn decision_label(value: DeliveryVerificationDecision) -> &'static str {
    match value {
        DeliveryVerificationDecision::Passed => "passed",
        DeliveryVerificationDecision::NeedsRevision => "needs_revision",
    }
}

fn disposition_label(value: DeliveryVerificationTreatmentDisposition) -> &'static str {
    match value {
        DeliveryVerificationTreatmentDisposition::PassedUnchanged => "passed_unchanged",
        DeliveryVerificationTreatmentDisposition::PassedAfterRepair => "passed_after_repair",
        DeliveryVerificationTreatmentDisposition::TreatmentFailure => "treatment_failure",
    }
}

fn failure_stage_label(value: DeliveryVerificationEvalStage) -> &'static str {
    match value {
        DeliveryVerificationEvalStage::InitialVerification => "initial_verification",
        DeliveryVerificationEvalStage::OwnerRepair => "owner_repair",
        DeliveryVerificationEvalStage::Recheck => "recheck",
    }
}

fn failure_code_label(
    value: super::delivery_verification::DeliveryVerificationTreatmentFailureCode,
) -> &'static str {
    use super::delivery_verification::DeliveryVerificationTreatmentFailureCode as Failure;
    match value {
        Failure::ProviderUnavailable => "provider_unavailable",
        Failure::Timeout => "timeout",
        Failure::BudgetExhausted => "budget_exhausted",
        Failure::InvalidVerifierResponse => "invalid_verifier_response",
        Failure::VerificationNotSatisfied => "verification_not_satisfied",
    }
}

fn eval_censor_label(value: DeliveryVerificationEvalCensor) -> &'static str {
    match value {
        DeliveryVerificationEvalCensor::Cancelled => "evaluation_cancelled",
        DeliveryVerificationEvalCensor::Steered => "evaluation_steered",
        _ => "invalid_evaluation_projection",
    }
}
