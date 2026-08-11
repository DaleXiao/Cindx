use super::*;

const TRACKED_PROTOCOL: &[u8] =
    include_bytes!("../../../../../benchmarks/agent/delivery-verification-protocol-v1.json");
const TRACKED_SUITE: &[u8] =
    include_bytes!("../../../../../benchmarks/agent/delivery-verification-v1.json");

fn tracked() -> ValidatedProtocol<'static> {
    parse_and_validate_protocol(TRACKED_PROTOCOL, TRACKED_SUITE)
        .expect("tracked delivery verification authority should validate")
}

fn encoded(mut value: Value) -> Vec<u8> {
    canonicalize_json(&mut value);
    let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn suite_value() -> Value {
    serde_json::from_slice(TRACKED_SUITE).unwrap()
}

fn protocol_value() -> Value {
    serde_json::from_slice(TRACKED_PROTOCOL).unwrap()
}

fn counts(
    complete_cases: usize,
    control_failures: usize,
    treatment_only_wins: usize,
    control_only_losses: usize,
) -> MatchedPairCounts {
    MatchedPairCounts {
        complete_cases,
        control_failures,
        treatment_only_wins,
        control_only_losses,
        structural_failures: 0,
        treatment_execution_failures: 0,
    }
}

#[test]
fn agent_delivery_verification_protocol_contract_tracked_authority_is_exact_and_non_authorizing() {
    let protocol = tracked();
    assert_eq!(protocol.protocol_id(), DELIVERY_VERIFICATION_PROTOCOL_ID);
    assert_eq!(protocol.suite_id(), DELIVERY_VERIFICATION_SUITE_ID);
    assert_eq!(protocol.manifest_bytes(), TRACKED_PROTOCOL);
    assert_eq!(protocol.cases().len(), 32);
    assert_eq!(protocol.calibration_cases().count(), 8);
    assert_eq!(protocol.holdout_cases().count(), 24);
    assert_eq!(
        protocol.execute_binary_name(),
        DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME
    );
    assert!(!protocol.execution_authorized());
    assert_eq!(
        protocol.manifest_sha256(),
        "080229aa24f05aa7ac816773aa5b2eb1bba2d00f3d1ca6b757b617ab9e27f19f"
    );
    assert_eq!(
        protocol.suite_sha256(),
        "b9672f7075d896e3c48673607c622604c0f8d6fb2b6df4499281b0741124fbcd"
    );
    eprintln!("{DELIVERY_VERIFICATION_PROTOCOL_SCHEMA}");
}

#[test]
fn agent_delivery_verification_protocol_contract_cases_are_balanced_ordered_and_require_inference_markers(
) {
    let protocol = tracked();
    let inference_markers = [
        "exclude",
        "plus",
        "minus",
        "sum",
        "convert",
        "outrank",
        "priority",
        "effective",
        "when",
        "only",
        "count",
        "divide",
        "average",
        "latest",
        "shortest",
        "preserve",
        "subtract",
        "times",
        "after",
        "at least",
        "exceeds",
        "distinct",
    ];
    let decoy_markers = [
        "draft",
        "future",
        "stale",
        "projection",
        "target",
        "expired",
        "retired",
        "failed",
        "general",
        "unverified",
        "ceiling",
        "default",
        "warm-up",
        "void",
        "pending",
        "forecast",
        "draining",
        "noncompliant",
    ];

    for stratum in STRATA {
        let cases = protocol
            .cases()
            .filter(|case| case.stratum() == stratum)
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 8);
        assert!(cases.iter().all(|case| case.evidence().len() >= 2));
        assert!(
            cases
                .iter()
                .filter(|case| {
                    let text = String::from_utf8(case.model_input_bytes().unwrap())
                        .unwrap()
                        .to_ascii_lowercase();
                    inference_markers.iter().any(|marker| text.contains(marker))
                })
                .count()
                >= 6
        );
        assert!(
            cases
                .iter()
                .filter(|case| {
                    let text = String::from_utf8(case.model_input_bytes().unwrap())
                        .unwrap()
                        .to_ascii_lowercase();
                    decoy_markers.iter().any(|marker| text.contains(marker))
                })
                .count()
                >= 6
        );
    }

    for (index, case) in protocol.cases().enumerate() {
        assert_eq!(case.ordinal(), index + 1);
        assert!(!case.id().is_empty());
        assert_eq!(case.stratum(), STRATA[index % 4]);
        assert_eq!(case.obligations()[0].obligation_ref.len(), 64);
        assert!(!case.objective().is_empty());
    }
}

#[test]
fn agent_delivery_verification_protocol_contract_model_input_excludes_every_oracle_field() {
    let protocol = tracked();
    for case in protocol.cases() {
        let input: Value = serde_json::from_slice(&case.model_input_bytes().unwrap()).unwrap();
        let object = input.as_object().unwrap();
        assert_eq!(object.len(), 3);
        assert!(object.contains_key("objective"));
        assert!(object.contains_key("obligations"));
        assert!(object.contains_key("evidence"));
        let text = serde_json::to_string(&input).unwrap();
        for oracle_key in [
            "oracle",
            "hiddenFromModel",
            "requiredGroups",
            "forbidden",
            "exactJson",
        ] {
            assert!(!text.contains(oracle_key));
        }
    }

    let mut suite: DeliveryVerificationSuite = serde_json::from_slice(TRACKED_SUITE).unwrap();
    suite.cases[0]
        .oracle
        .forbidden
        .push("MODEL_HIDDEN_ORACLE_ONLY_SENTINEL".into());
    let sentinel_case = ValidatedCase {
        case: &suite.cases[0],
        case_sha256: "fixture-case-sha256",
    };
    let input = String::from_utf8(sentinel_case.model_input_bytes().unwrap()).unwrap();
    assert!(!input.contains("MODEL_HIDDEN_ORACLE_ONLY_SENTINEL"));
}

#[test]
fn agent_delivery_verification_protocol_contract_oracle_is_exact_and_externally_predicated() {
    let protocol = tracked();
    let first = protocol.cases().next().unwrap();
    assert_eq!(
        first.evaluate_output(r#"{"mean_ms":118,"service":"api"}"#),
        OracleEvaluation::Passed
    );
    assert_eq!(
        first.evaluate_output(r#"{"service":"api","mean_ms":118,"forecast":181}"#),
        OracleEvaluation::Failed
    );
    assert_eq!(
        first.evaluate_output(r#"{"service":"api","mean_ms":117}"#),
        OracleEvaluation::Failed
    );
    assert_eq!(
        first.evaluate_output(r#"{"mean_ms":999,"mean_ms":118,"service":"api"}"#),
        OracleEvaluation::Failed
    );
    assert!(parse_unique_json(r#"{"outer":{"value":1,"value":2}}"#).is_err());
    assert_eq!(first.evaluate_output("not-json"), OracleEvaluation::Failed);
}

#[test]
fn agent_delivery_verification_protocol_contract_hashes_bind_cases_order_budget_and_hidden_oracle()
{
    let protocol = tracked();
    assert_eq!(protocol.case_sha256s().len(), 32);
    assert_eq!(
        protocol.case_sha256s().first().unwrap(),
        "23c7a85cb4537fb0e3e98f580bd2d274ab393742eb733bb51161516f2225cb5c"
    );
    assert_eq!(
        protocol.case_sha256s().last().unwrap(),
        "536138c08a7a06e0f58986bfd5d4382340135f8d13b828174b34b51afa2f2658"
    );
    assert_eq!(
        protocol.hidden_oracle_sha256(),
        "af47cdf41554956882e5fc42bd432a5c8dabff49573f2ce8f7181878e69d2b2d"
    );
    assert_eq!(
        protocol.budget_sha256(),
        "4b72b261ce347f7ec0ab80328f6ed57050a04260bf81c6f0930d9c08f5c710fe"
    );
    assert_eq!(
        protocol.cases().next().unwrap().case_sha256(),
        protocol.case_sha256s()[0]
    );
}

#[test]
fn agent_delivery_verification_protocol_contract_rejects_unknown_fields_and_noncanonical_bytes() {
    let mut manifest = protocol_value();
    manifest
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), Value::Bool(true));
    assert!(parse_and_validate_protocol(&encoded(manifest), TRACKED_SUITE).is_err());

    let mut suite = suite_value();
    suite["cases"][0]
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), Value::Bool(true));
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(suite)).is_err());

    let mut noncanonical = TRACKED_SUITE.to_vec();
    noncanonical.pop();
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &noncanonical).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_rejects_suite_hash_order_or_oracle_drift() {
    let mut changed = suite_value();
    changed["cases"][0]["objective"] =
        Value::String("Return only JSON with service and a changed mean contract.".into());
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(changed)).is_err());

    let mut reordered = suite_value();
    reordered["cases"].as_array_mut().unwrap().swap(0, 1);
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(reordered)).is_err());

    let mut exposed = suite_value();
    exposed["cases"][0]["oracle"]["hiddenFromModel"] = Value::Bool(false);
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(exposed)).is_err());

    let mut credential = suite_value();
    credential["cases"][0]["evidence"][0]["content"] =
        Value::String("raw marker ghp_fixture_must_fail_closed".into());
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(credential)).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_budget_topology_and_freeze_are_fixed() {
    let protocol = tracked();
    assert_eq!(protocol.budget().max_logical_model_calls_per_case, 4);
    assert_eq!(protocol.budget().max_logical_model_calls_total, 128);
    assert_eq!(protocol.budget().max_physical_model_attempts_total, 128);
    assert_eq!(protocol.budget().transport_retries, 0);
    assert!(protocol.manifest.design.shared_owner_draft);
    assert_eq!(protocol.manifest.design.max_owner_repairs_per_case, 1);
    assert_eq!(protocol.manifest.design.max_verifier_rechecks_per_case, 1);
    assert_eq!(
        protocol
            .manifest
            .calibration_gate
            .prompt_model_threshold_changes_after_calibration,
        "invalidate_protocol"
    );
    assert!(
        protocol
            .manifest
            .calibration_gate
            .calibration_results_excluded_from_holdout
    );
    assert!(
        protocol
            .manifest
            .holdout_decision
            .calibration_results_excluded
    );

    let mut changed = protocol_value();
    changed["budget"]["transport_retries"] = Value::from(1);
    assert!(parse_and_validate_protocol(&encoded(changed), TRACKED_SUITE).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_calibration_gate_is_fail_closed() {
    assert_eq!(
        calibration_decision(counts(8, 2, 1, 0)),
        CalibrationDecision::OpenHoldout
    );
    assert_eq!(
        calibration_decision(counts(8, 1, 1, 0)),
        CalibrationDecision::TerminalFutility
    );
    assert_eq!(
        calibration_decision(counts(8, 2, 1, 1)),
        CalibrationDecision::TerminalFutility
    );
    assert_eq!(
        calibration_decision(counts(7, 2, 1, 0)),
        CalibrationDecision::Inconclusive
    );
    let mut structural = counts(7, 2, 1, 0);
    structural.structural_failures = 1;
    assert_eq!(
        calibration_decision(structural),
        CalibrationDecision::Inconclusive
    );
    let mut execution = counts(8, 2, 1, 0);
    execution.treatment_execution_failures = 1;
    assert_eq!(
        calibration_decision(execution),
        CalibrationDecision::Inconclusive
    );
    assert_eq!(
        calibration_decision(counts(9, 2, 1, 0)),
        CalibrationDecision::Invalid
    );
}

#[test]
fn agent_delivery_verification_protocol_contract_holdout_requires_five_zero_and_no_censor() {
    assert_eq!(
        holdout_decision(counts(24, 5, 5, 0)),
        HoldoutDecisionResult::EvidenceOfUplift
    );
    assert_eq!(
        holdout_decision(counts(24, 4, 4, 0)),
        HoldoutDecisionResult::NoEvidence
    );
    assert_eq!(
        holdout_decision(counts(24, 7, 6, 1)),
        HoldoutDecisionResult::Regression
    );
    assert_eq!(
        holdout_decision(counts(23, 5, 5, 0)),
        HoldoutDecisionResult::Inconclusive
    );

    let mut censored = counts(24, 5, 5, 0);
    censored.treatment_execution_failures = 1;
    assert_eq!(
        holdout_decision(censored),
        HoldoutDecisionResult::Inconclusive
    );
}
