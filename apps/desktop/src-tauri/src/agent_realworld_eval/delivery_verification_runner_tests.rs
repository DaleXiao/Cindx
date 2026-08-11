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
    CalibrationOpenNoEvidence,
    CalibrationFutility,
    HoldoutUplift,
    HoldoutRegression,
    InvalidVerifier,
    OwnerFailure,
    VerifierTimeout,
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
            ScriptMode::CalibrationFutility | ScriptMode::OwnerFailure => false,
            ScriptMode::HoldoutUplift => ordinal <= 2 || (9..=13).contains(&ordinal),
            ScriptMode::HoldoutRegression => ordinal <= 2 || ordinal == 9,
            _ => ordinal <= 2,
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

        if self.mode == ScriptMode::OwnerFailure
            && call.stage == DeliveryVerificationCallStageV1::OwnerDraft
        {
            return CallOutcome::ModelFailure(
                DeliveryVerificationModelFailure::ProviderUnavailable,
            );
        }
        if self.mode == ScriptMode::VerifierTimeout
            && call.stage == DeliveryVerificationCallStageV1::VerifierInitial
        {
            return CallOutcome::ModelFailure(DeliveryVerificationModelFailure::Timeout);
        }

        let content = match call.stage {
            DeliveryVerificationCallStageV1::OwnerDraft => {
                if self.mode == ScriptMode::HoldoutRegression && call.case_ordinal == 9 {
                    oracle_output(call.case_ordinal)
                } else if self.should_revise(call.case_ordinal) {
                    "{}".to_string()
                } else {
                    oracle_output(call.case_ordinal)
                }
            }
            DeliveryVerificationCallStageV1::VerifierInitial => {
                if self.mode == ScriptMode::InvalidVerifier {
                    "not-json".to_string()
                } else {
                    verifier_verdict(&payload, self.should_revise(call.case_ordinal))
                }
            }
            DeliveryVerificationCallStageV1::OwnerRepair => {
                if self.mode == ScriptMode::HoldoutRegression && call.case_ordinal == 9 {
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
            "/../../../benchmarks/agent/delivery-verification-protocol-v1.json"
        )),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/delivery-verification-v1.json"
        )),
    )
    .unwrap()
}

fn suite() -> &'static Value {
    static SUITE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SUITE.get_or_init(|| {
        serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/delivery-verification-v1.json"
        )))
        .unwrap()
    })
}

fn oracle_output(ordinal: usize) -> String {
    serde_json::to_string(&suite()["cases"][ordinal - 1]["oracle"]["exactJson"]).unwrap()
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
        OsString::from("wrong-protocol"),
    ])
    .is_err());
}

#[test]
fn agent_delivery_verification_execution_contract_consumed_v1_cannot_authorize_or_execute() {
    let error = reject_consumed_delivery_protocol(DELIVERY_VERIFICATION_PROTOCOL_ID).unwrap_err();
    assert!(error.contains("is consumed"));
    assert!(error.contains("successor protocol"));
    assert!(reject_consumed_delivery_protocol("cindx-delivery-verification-protocol-v2").is_ok());
}

#[test]
fn agent_delivery_verification_execution_contract_accepts_only_exact_provider_receipts() {
    let response = exact_response();
    assert_eq!(
        validate_model_response(&response).unwrap(),
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
fn agent_delivery_verification_execution_contract_pass_preserves_one_shared_owner_draft() {
    let protocol = protocol();
    let case = protocol.cases().nth(2).unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::CalibrationOpenNoEvidence);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::Complete);
    assert_eq!(
        outcome.control_output_sha256,
        outcome.treatment_output_sha256
    );
    assert_eq!(
        runtime.stages,
        vec![
            (3, DeliveryVerificationCallStageV1::OwnerDraft),
            (3, DeliveryVerificationCallStageV1::VerifierInitial),
        ]
    );
}

#[test]
fn agent_delivery_verification_execution_contract_revision_is_exactly_one_repair_and_recheck() {
    let protocol = protocol();
    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::CalibrationOpenNoEvidence);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::Complete);
    assert_eq!(outcome.control_passed, Some(false));
    assert_eq!(outcome.treatment_passed, Some(true));
    assert_eq!(
        runtime.stages,
        vec![
            (1, DeliveryVerificationCallStageV1::OwnerDraft),
            (1, DeliveryVerificationCallStageV1::VerifierInitial),
            (1, DeliveryVerificationCallStageV1::OwnerRepair),
            (1, DeliveryVerificationCallStageV1::VerifierRecheck),
        ]
    );
}

#[test]
fn agent_delivery_verification_execution_contract_invalid_verifier_has_no_retry() {
    let protocol = protocol();
    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::InvalidVerifier);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::TreatmentExecutionFailure);
    assert_eq!(runtime.stages.len(), 2);
}

#[test]
fn agent_delivery_verification_execution_contract_provider_timeout_has_no_retry() {
    let protocol = protocol();
    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::VerifierTimeout);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::TreatmentExecutionFailure);
    assert_eq!(runtime.stages.len(), 2);
}

#[test]
fn agent_delivery_verification_execution_contract_owner_failure_is_unmatched_and_terminal() {
    let protocol = protocol();
    let case = protocol.cases().next().unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::OwnerFailure);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::StructuralFailure);
    assert!(outcome.owner_draft_sha256.is_none());
    assert_eq!(runtime.stages.len(), 1);
}

#[test]
fn agent_delivery_verification_execution_contract_served_models_must_be_independent() {
    let protocol = protocol();
    let case = protocol.cases().nth(2).unwrap();
    let mut runtime = ScriptedRuntime::new(ScriptMode::SameServedModel);
    let outcome = execute_case(case, protocol.budget(), &mut runtime);
    assert_eq!(outcome.status, CaseStatus::StructuralFailure);
    assert_eq!(runtime.stages.len(), 2);
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
fn agent_delivery_verification_execution_contract_calibration_opens_before_exact_holdout() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::CalibrationOpenNoEvidence);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::NoEvidence
    );
    assert_eq!(
        runtime.calibration.map(|value| value.0),
        Some(CalibrationDecision::OpenHoldout)
    );
    assert_eq!(
        runtime.holdout.map(|value| value.0),
        Some(HoldoutDecisionResult::NoEvidence)
    );
    assert_eq!(runtime.begun_cases, (1..=32).collect::<Vec<_>>());
    assert_eq!(runtime.stages.len(), 68);
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
    assert_eq!(runtime.stages.len(), 16);
}

#[test]
fn agent_delivery_verification_execution_contract_five_zero_holdout_is_uplift() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::HoldoutUplift);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::EvidenceOfUplift
    );
    let (_, counts) = runtime.holdout.unwrap();
    assert_eq!(counts.treatment_only_wins, 5);
    assert_eq!(counts.control_only_losses, 0);
}

#[test]
fn agent_delivery_verification_execution_contract_control_only_loss_is_regression() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::HoldoutRegression);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::Regression
    );
    let (_, counts) = runtime.holdout.unwrap();
    assert_eq!(counts.control_only_losses, 1);
}

#[test]
fn agent_delivery_verification_execution_contract_failure_stops_campaign_without_replacement() {
    let protocol = protocol();
    let mut runtime = ScriptedRuntime::new(ScriptMode::InvalidVerifier);
    let disposition = execute_fixed_campaign(&protocol, &mut runtime).unwrap();
    assert_eq!(
        disposition,
        DeliveryVerificationCampaignDispositionV1::Inconclusive
    );
    assert_eq!(runtime.begun_cases, vec![1]);
    assert_eq!(
        runtime.cases[0].status,
        CaseStatus::TreatmentExecutionFailure
    );
    assert_eq!(runtime.stages.len(), 2);
}
