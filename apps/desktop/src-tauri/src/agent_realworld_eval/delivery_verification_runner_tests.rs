use super::*;
use crate::agent_realworld_eval::delivery_verification_campaign::execute_case;
use crate::agent_realworld_eval::delivery_verification_protocol::{
    parse_and_validate_protocol, ValidatedProtocol,
};
use agent_core::{Message, MessageRole, Metadata, ModelCallMode, ModelToolCall};
use agent_runtime::DELIVERY_VERIFICATION_VERDICT_SCHEMA;
use serde_json::{json, Value};

const OWNER_MODEL: &str = "owner-configured";
const VERIFIER_MODEL: &str = "verifier-configured";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptMode {
    CalibrationOpenNotEffective,
    CalibrationFutility,
    HoldoutSeededRepairEffective,
    HoldoutPreservationRegression,
    InvalidVerifier,
    EmptyVerifier,
    InitialModelFailure(DeliveryVerificationModelFailure),
    RepairModelFailure(DeliveryVerificationModelFailure),
    SameServedModel,
}

struct ScriptedRuntime {
    mode: ScriptMode,
    stages: Vec<(usize, DeliveryVerificationCallStageV1)>,
    cases: Vec<CaseOutcome>,
    begun_cases: Vec<usize>,
    calibration: Option<(CalibrationDecision, MatchedPairCounts)>,
    holdout: Option<(HoldoutDecisionResult, MatchedPairCounts)>,
    terminal: Option<DeliveryVerificationCampaignDispositionV1>,
}

impl ScriptedRuntime {
    fn new(mode: ScriptMode) -> Self {
        Self {
            mode,
            stages: Vec::new(),
            cases: Vec::new(),
            begun_cases: Vec::new(),
            calibration: None,
            holdout: None,
            terminal: None,
        }
    }

    fn should_revise(&self, ordinal: usize) -> bool {
        match self.mode {
            ScriptMode::CalibrationFutility => false,
            ScriptMode::CalibrationOpenNotEffective => matches!(
                ordinal,
                1 | 2 | 3 | 5 | 9 | 10 | 11 | 13 | 14 | 15 | 17 | 18 | 19 | 21 | 22 | 23
            ),
            ScriptMode::HoldoutSeededRepairEffective => matches!(
                ordinal,
                1 | 2 | 3 | 5 | 9 | 10 | 11 | 13 | 14 | 15 | 17 | 18 | 19 | 21 | 22 | 23 | 25
            ),
            ScriptMode::HoldoutPreservationRegression => {
                matches!(ordinal, 1 | 2 | 3 | 5 | 12)
            }
            ScriptMode::InvalidVerifier
            | ScriptMode::EmptyVerifier
            | ScriptMode::InitialModelFailure(_)
            | ScriptMode::RepairModelFailure(_)
            | ScriptMode::SameServedModel => ordinal == 1,
        }
    }

    fn served_model_sha256(&self, role: &ModelRole) -> String {
        if self.mode == ScriptMode::SameServedModel || *role == ModelRole::Executor {
            sha256_hex(b"served-owner")
        } else {
            sha256_hex(b"served-verifier")
        }
    }
}

impl DeliveryVerificationRuntime for ScriptedRuntime {
    fn configured_model(&self, role: &ModelRole) -> &str {
        match role {
            ModelRole::Executor => OWNER_MODEL,
            ModelRole::Reviewer => VERIFIER_MODEL,
            _ => "",
        }
    }

    fn begin_case(&mut self, ordinal: usize) -> Result<(), String> {
        self.begun_cases.push(ordinal);
        Ok(())
    }

    fn dispatch(&mut self, call: PreparedDeliveryCall) -> CallOutcome {
        assert!(call.request.tools.is_empty());
        assert_eq!(call.request.mode, ModelCallMode::NonStreaming);
        assert_eq!(call.request.role, call.role);
        assert_eq!(
            call.request
                .metadata
                .get("delivery_verification_target_model")
                .map(String::as_str),
            Some(call.configured_model.as_str())
        );
        let payload: Value = serde_json::from_str(&call.request.messages[1].content).unwrap();
        let encoded = serde_json::to_string(&payload).unwrap();
        assert!(!encoded.contains("hiddenFromModel"));
        assert!(!encoded.contains("exactJson"));
        assert!(payload.get("oracle").is_none());
        self.stages.push((call.case_ordinal, call.stage));

        if call.stage == DeliveryVerificationCallStageV1::VerifierInitial {
            assert_eq!(
                payload["ownerDraft"].as_str(),
                Some(seeded_candidate(call.case_ordinal))
            );
        }
        match (self.mode, call.stage) {
            (
                ScriptMode::InitialModelFailure(failure),
                DeliveryVerificationCallStageV1::VerifierInitial,
            )
            | (
                ScriptMode::RepairModelFailure(failure),
                DeliveryVerificationCallStageV1::OwnerRepair,
            ) => return CallOutcome::ModelFailure(failure),
            _ => {}
        }

        let content = match call.stage {
            DeliveryVerificationCallStageV1::VerifierInitial => {
                if self.mode == ScriptMode::InvalidVerifier {
                    "not-json".to_string()
                } else if self.mode == ScriptMode::EmptyVerifier {
                    String::new()
                } else {
                    verifier_verdict(&payload, self.should_revise(call.case_ordinal))
                }
            }
            DeliveryVerificationCallStageV1::OwnerRepair => {
                if self.mode == ScriptMode::HoldoutPreservationRegression && call.case_ordinal == 12
                {
                    r#"{"regressed":true}"#.to_string()
                } else {
                    oracle_output(call.case_ordinal)
                }
            }
            DeliveryVerificationCallStageV1::VerifierRecheck => verifier_verdict(&payload, false),
        };
        CallOutcome::Completed(CompletedCall {
            content,
            served_model_sha256: self.served_model_sha256(&call.role),
        })
    }

    fn record_case(&mut self, outcome: &CaseOutcome) -> Result<(), String> {
        self.cases.push(outcome.clone());
        Ok(())
    }

    fn record_calibration(
        &mut self,
        decision: CalibrationDecision,
        counts: MatchedPairCounts,
    ) -> Result<(), String> {
        self.calibration = Some((decision, counts));
        Ok(())
    }

    fn record_holdout(
        &mut self,
        decision: HoldoutDecisionResult,
        counts: MatchedPairCounts,
    ) -> Result<(), String> {
        self.holdout = Some((decision, counts));
        Ok(())
    }

    fn finish(
        &mut self,
        disposition: DeliveryVerificationCampaignDispositionV1,
        _reason: &str,
        evidence_sha256: String,
    ) -> Result<(), String> {
        assert_eq!(evidence_sha256.len(), 64);
        self.terminal = Some(disposition);
        Ok(())
    }

    fn is_terminal(&self) -> bool {
        false
    }
}

fn protocol() -> ValidatedProtocol<'static> {
    parse_and_validate_protocol(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/delivery-verification-protocol-v4.json"
        )),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/delivery-verification-v3.json"
        )),
    )
    .unwrap()
}

fn suite() -> &'static Value {
    static SUITE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SUITE.get_or_init(|| {
        serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/delivery-verification-v3.json"
        )))
        .unwrap()
    })
}

fn oracle_output(ordinal: usize) -> String {
    serde_json::to_string(&suite()["cases"][ordinal - 1]["oracle"]["exactJson"]).unwrap()
}

fn seeded_candidate(ordinal: usize) -> &'static str {
    suite()["cases"][ordinal - 1]["seed"]["candidate"]
        .as_str()
        .unwrap()
}

fn verifier_verdict(payload: &Value, needs_revision: bool) -> String {
    let subject = &payload["subject"];
    let obligation_refs = subject["obligationRefs"].clone();
    let evidence_refs = subject["evidenceRefs"].clone();
    let findings = if needs_revision {
        vec![json!({
            "kind": "unsupported_claim",
            "summary": "The draft does not satisfy the bound evidence.",
            "obligationRefs": obligation_refs,
            "evidenceRefs": evidence_refs,
        })]
    } else {
        Vec::new()
    };
    json!({
        "schema": DELIVERY_VERIFICATION_VERDICT_SCHEMA,
        "subjectSha256": subject["subjectSha256"],
        "reviewedObjectiveSha256": subject["objectiveSha256"],
        "decision": if needs_revision { "needs_revision" } else { "passed" },
        "reviewedObligationRefs": subject["obligationRefs"],
        "reviewedEvidenceRefs": subject["evidenceRefs"],
        "findings": findings,
    })
    .to_string()
}

fn exact_response() -> ModelResponse {
    ModelResponse {
        message: Message {
            role: MessageRole::Assistant,
            content: "done".to_string(),
            metadata: Metadata::new(),
        },
        raw_tool_calls_json: None,
        tool_calls: Vec::new(),
        metadata: [
            ("finish_reason", "stop"),
            ("request_payload_sha256", &"1".repeat(64)),
            ("response_semantic_sha256", &"2".repeat(64)),
            ("provider_response_id", "response-1"),
            ("provider_response_model", "served-verifier"),
            ("provider_receipt_status", "observed"),
            ("prompt_tokens", "10"),
            ("completion_tokens", "2"),
            ("total_tokens", "12"),
            ("usage_source", "provider"),
            ("usage_estimated", "false"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect(),
    }
}

#[test]
fn agent_delivery_verification_execution_contract_requires_explicit_authorize_command() {
    assert!(require_authorize_arguments([
        OsString::from("authorize"),
        OsString::from(AUTHORIZE_FLAG),
        OsString::from(DELIVERY_VERIFICATION_PROTOCOL_ID),
    ])
    .is_ok());
    assert!(require_authorize_arguments([OsString::from("authorize")]).is_err());
    assert!(require_authorize_arguments([
        OsString::from("authorize"),
        OsString::from(AUTHORIZE_FLAG),
        OsString::from(CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID),
    ])
    .is_err());
    assert!(require_authorize_arguments([
        OsString::from("authorize"),
        OsString::from(AUTHORIZE_FLAG),
        OsString::from(CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V2_ID),
    ])
    .is_err());
    assert!(require_authorize_arguments([
        OsString::from("authorize"),
        OsString::from(AUTHORIZE_FLAG),
        OsString::from(CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V3_ID),
    ])
    .is_err());
    assert!(require_authorize_arguments([
        OsString::from("authorize"),
        OsString::from(AUTHORIZE_FLAG),
        OsString::from("wrong-protocol"),
    ])
    .is_err());
}

#[test]
fn agent_delivery_verification_execution_contract_consumed_v1_v2_and_v3_entrypoints_fail_closed() {
    for protocol_id in [
        CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID,
        CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V2_ID,
        CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V3_ID,
    ] {
        let error = reject_consumed_delivery_protocol(protocol_id).unwrap_err();
        assert!(error.contains(protocol_id));
        assert!(error.contains("is consumed"));
        assert!(error.contains("successor protocol"));
    }
    assert!(reject_consumed_delivery_protocol(DELIVERY_VERIFICATION_PROTOCOL_ID).is_ok());
    for (protocol_id, retired) in [
        (
            CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID,
            [
                crate::agent_realworld_eval::run_delivery_verification_preflight(),
                crate::agent_realworld_eval::run_delivery_verification_authorize(),
                crate::agent_realworld_eval::run_delivery_verification_execute(),
            ],
        ),
        (
            CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V2_ID,
            [
                crate::agent_realworld_eval::run_delivery_verification_v2_preflight(),
                crate::agent_realworld_eval::run_delivery_verification_v2_authorize(),
                crate::agent_realworld_eval::run_delivery_verification_v2_execute(),
            ],
        ),
        (
            CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V3_ID,
            [
                crate::agent_realworld_eval::run_delivery_verification_v3_preflight(),
                crate::agent_realworld_eval::run_delivery_verification_v3_authorize(),
                crate::agent_realworld_eval::run_delivery_verification_v3_execute(),
            ],
        ),
    ] {
        for result in retired {
            let error = result.unwrap_err();
            assert!(error.contains(protocol_id));
            assert!(error.contains("is consumed"));
        }
    }
}

#[test]
fn agent_delivery_verification_execution_contract_accepts_only_exact_provider_receipts() {
    let response = exact_response();
    assert_eq!(
        validate_model_response(&response).unwrap(),
        sha256_hex(b"served-verifier")
    );
    let mut empty = response.clone();
    empty.message.content.clear();
    assert_eq!(
        validate_model_response(&empty).unwrap(),
        sha256_hex(b"served-verifier")
    );
    let mut estimated = response.clone();
    estimated
        .metadata
        .insert("usage_estimated".into(), "true".into());
    assert!(validate_model_response(&estimated).is_err());
    let mut conflicting = response.clone();
    conflicting
        .metadata
        .insert("total_tokens".into(), "13".into());
    assert!(validate_model_response(&conflicting).is_err());
    let mut missing_id = response.clone();
    missing_id.metadata.remove("provider_response_id");
    assert!(validate_model_response(&missing_id).is_err());
}

#[test]
fn agent_delivery_verification_execution_contract_rejects_tools_and_incomplete_outputs() {
    let mut response = exact_response();
    response.tool_calls.push(ModelToolCall {
        id: "call-1".into(),
        name: "forbidden".into(),
        arguments_json: "{}".into(),
    });
    assert!(validate_model_response(&response).is_err());
    let mut limited = exact_response();
    limited
        .metadata
        .insert("finish_reason".into(), "length".into());
    assert!(validate_model_response(&limited).is_err());
}

#[test]
fn agent_delivery_verification_execution_contract_pass_preserves_exact_seed_in_one_call() {
    let protocol = protocol();
    let case = protocol.cases().nth(3).unwrap();
    let seed = case.seeded_candidate();
    let seed_sha256 = sha256_hex(seed.as_bytes());
    let mut runtime = ScriptedRuntime::new(ScriptMode::CalibrationOpenNotEffective);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::Complete);
    assert_eq!(outcome.seeded_candidate_sha256, Some(seed_sha256.clone()));
    assert_eq!(outcome.seeded_candidate_bytes, Some(seed.len() as u64));
    assert_eq!(outcome.control_output_sha256, Some(seed_sha256.clone()));
    assert_eq!(outcome.treatment_output_sha256, Some(seed_sha256));
    assert_eq!(outcome.control_passed, Some(true));
    assert_eq!(outcome.treatment_passed, Some(true));
    assert!(!outcome.repair_activated);
    assert_eq!(
        outcome.treatment_disposition.as_deref(),
        Some("passed_unchanged")
    );
    assert_eq!(
        runtime.stages,
        vec![(4, DeliveryVerificationCallStageV1::VerifierInitial)]
    );
}

#[test]
fn agent_delivery_verification_execution_contract_revision_is_exactly_one_repair_and_recheck() {
    let protocol = protocol();
    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::CalibrationOpenNotEffective);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::Complete);
    assert_eq!(outcome.control_passed, Some(false));
    assert_eq!(outcome.treatment_passed, Some(true));
    assert!(outcome.repair_activated);
    assert_eq!(
        outcome.treatment_disposition.as_deref(),
        Some("passed_after_repair")
    );
    assert_eq!(
        runtime.stages,
        vec![
            (1, DeliveryVerificationCallStageV1::VerifierInitial),
            (1, DeliveryVerificationCallStageV1::OwnerRepair),
            (1, DeliveryVerificationCallStageV1::VerifierRecheck),
        ]
    );
}

#[test]
fn agent_delivery_verification_execution_contract_invalid_verdict_is_itt_loss_without_retry() {
    let protocol = protocol();
    for mode in [ScriptMode::InvalidVerifier, ScriptMode::EmptyVerifier] {
        let case = protocol.cases().next().unwrap();
        let mut runtime = ScriptedRuntime::new(mode);
        let outcome = execute_case(case, protocol.budget(), &mut runtime);
        assert_eq!(outcome.status, CaseStatus::Complete);
        assert_eq!(outcome.control_passed, Some(false));
        assert_eq!(outcome.treatment_passed, Some(false));
        assert_eq!(outcome.initial_verifier_decision, None);
        assert!(!outcome.repair_activated);
        assert_eq!(
            outcome.treatment_disposition.as_deref(),
            Some("treatment_failure")
        );
        assert_eq!(
            outcome.failure_stage.as_deref(),
            Some("initial_verification")
        );
        assert_eq!(
            outcome.failure_code.as_deref(),
            Some("invalid_verifier_response")
        );
        assert_eq!(
            runtime.stages,
            vec![(1, DeliveryVerificationCallStageV1::VerifierInitial)]
        );
    }
}

#[test]
fn agent_delivery_verification_execution_contract_provider_failures_are_itt_losses_but_budget_is_structural(
) {
    let protocol = protocol();
    for (failure, expected_code) in [
        (
            DeliveryVerificationModelFailure::ProviderUnavailable,
            "provider_unavailable",
        ),
        (DeliveryVerificationModelFailure::Timeout, "timeout"),
    ] {
        let case = protocol.cases().next().unwrap();
        let mut runtime = ScriptedRuntime::new(ScriptMode::InitialModelFailure(failure));
        let outcome = execute_case(case, protocol.budget(), &mut runtime);
        assert_eq!(outcome.status, CaseStatus::Complete);
        assert_eq!(outcome.control_passed, Some(false));
        assert_eq!(outcome.treatment_passed, Some(false));
        assert!(!outcome.repair_activated);
        assert_eq!(
            outcome.treatment_disposition.as_deref(),
            Some("treatment_failure")
        );
        assert_eq!(
            outcome.failure_stage.as_deref(),
            Some("initial_verification")
        );
        assert_eq!(outcome.failure_code.as_deref(), Some(expected_code));
        assert_eq!(
            runtime.stages,
            vec![(1, DeliveryVerificationCallStageV1::VerifierInitial)]
        );
    }

    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::InitialModelFailure(
        DeliveryVerificationModelFailure::BudgetExhausted,
    ));
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::StructuralFailure);
    assert_eq!(outcome.reason, "budget_exhausted");
    assert_eq!(outcome.treatment_passed, None);
    assert_eq!(outcome.treatment_disposition, None);
    assert_eq!(
        runtime.stages,
        vec![(1, DeliveryVerificationCallStageV1::VerifierInitial)]
    );
}

#[test]
fn agent_delivery_verification_execution_contract_repair_model_failure_is_itt_loss_without_retry() {
    let protocol = protocol();
    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::RepairModelFailure(
        DeliveryVerificationModelFailure::ProviderUnavailable,
    ));
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::Complete);
    assert_eq!(outcome.control_passed, Some(false));
    assert_eq!(outcome.treatment_passed, Some(false));
    assert!(outcome.repair_activated);
    assert_eq!(
        outcome.treatment_disposition.as_deref(),
        Some("treatment_failure")
    );
    assert_eq!(outcome.failure_stage.as_deref(), Some("owner_repair"));
    assert_eq!(
        outcome.failure_code.as_deref(),
        Some("provider_unavailable")
    );
    assert_eq!(
        runtime.stages,
        vec![
            (1, DeliveryVerificationCallStageV1::VerifierInitial),
            (1, DeliveryVerificationCallStageV1::OwnerRepair),
        ]
    );
}

#[test]
fn agent_delivery_verification_execution_contract_served_models_must_be_independent() {
    let protocol = protocol();
    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::SameServedModel);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::StructuralFailure);
    assert_eq!(
        runtime.stages,
        vec![
            (1, DeliveryVerificationCallStageV1::VerifierInitial),
            (1, DeliveryVerificationCallStageV1::OwnerRepair),
        ]
    );
}

#[test]
fn agent_delivery_verification_execution_contract_served_models_cannot_drift_between_cases() {
    let mut binding = ServedModelBinding::default();
    let owner = sha256_hex(b"served-owner");
    let verifier = sha256_hex(b"served-verifier");
    assert!(binding.observe(&ModelRole::Executor, &owner).is_ok());
    assert!(binding.observe(&ModelRole::Reviewer, &verifier).is_ok());
    assert!(binding.observe(&ModelRole::Executor, &owner).is_ok());
    assert!(binding
        .observe(&ModelRole::Executor, &sha256_hex(b"owner-drift"))
        .is_err());

    let mut aliased = ServedModelBinding::default();
    assert!(aliased.observe(&ModelRole::Executor, &owner).is_ok());
    assert!(aliased.observe(&ModelRole::Reviewer, &owner).is_err());
}

#[test]
fn agent_delivery_verification_execution_contract_calibration_opens_to_not_effective_holdout() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::CalibrationOpenNotEffective);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::NotEffective
    );
    assert_eq!(
        runtime.calibration.map(|value| value.0),
        Some(CalibrationDecision::OpenHoldout)
    );
    assert_eq!(
        runtime.holdout.map(|value| value.0),
        Some(HoldoutDecisionResult::NotEffective)
    );
    assert_eq!(runtime.begun_cases, (1..=32).collect::<Vec<_>>());
    assert_eq!(runtime.stages.len(), 64);
    let (_, calibration_counts) = runtime.calibration.unwrap();
    assert_eq!(calibration_counts.treatment_only_wins, 4);
    assert_eq!(calibration_counts.unsupported_claim_wins, 2);
    assert_eq!(calibration_counts.omitted_obligation_wins, 1);
    assert_eq!(calibration_counts.contradiction_wins, 1);
    let (_, holdout_counts) = runtime.holdout.unwrap();
    assert_eq!(holdout_counts.treatment_only_wins, 12);
    assert_eq!(holdout_counts.unsupported_claim_wins, 4);
    assert_eq!(holdout_counts.omitted_obligation_wins, 4);
    assert_eq!(holdout_counts.contradiction_wins, 4);
}

#[test]
fn agent_delivery_verification_execution_contract_calibration_futility_never_opens_holdout() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::CalibrationFutility);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::TerminalFutility
    );
    assert_eq!(runtime.begun_cases, (1..=8).collect::<Vec<_>>());
    assert!(runtime.holdout.is_none());
    assert_eq!(runtime.stages.len(), 8);
}

#[test]
fn agent_delivery_verification_execution_contract_thirteen_zero_holdout_is_seeded_repair_effective()
{
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::HoldoutSeededRepairEffective);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::SeededRepairEffective
    );
    let (decision, counts) = runtime.holdout.unwrap();
    assert_eq!(decision, HoldoutDecisionResult::SeededRepairEffective);
    assert_eq!(counts.treatment_only_wins, 13);
    assert_eq!(counts.control_only_losses, 0);
    assert_eq!(counts.unsupported_claim_wins, 5);
    assert_eq!(counts.omitted_obligation_wins, 4);
    assert_eq!(counts.contradiction_wins, 4);
}

#[test]
fn agent_delivery_verification_execution_contract_unnecessary_preservation_repair_is_regression() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::HoldoutPreservationRegression);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::PreservationRegression
    );
    let (decision, counts) = runtime.holdout.unwrap();
    assert_eq!(decision, HoldoutDecisionResult::PreservationRegression);
    assert_eq!(counts.control_only_losses, 1);
    assert_eq!(counts.preservation_losses, 1);
    let preservation = runtime
        .cases
        .iter()
        .find(|case| case.ordinal == 12)
        .unwrap();
    assert_eq!(preservation.control_passed, Some(true));
    assert_eq!(preservation.treatment_passed, Some(false));
    assert!(preservation.repair_activated);
    assert_eq!(
        preservation.treatment_disposition.as_deref(),
        Some("passed_after_repair")
    );
}

#[test]
fn agent_delivery_verification_execution_contract_itt_loss_does_not_stop_or_replace_cases() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::InvalidVerifier);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::TerminalFutility
    );
    assert_eq!(runtime.begun_cases, (1..=8).collect::<Vec<_>>());
    assert!(runtime
        .cases
        .iter()
        .all(|case| case.status == CaseStatus::Complete));
    assert_eq!(runtime.stages.len(), 8);
}
