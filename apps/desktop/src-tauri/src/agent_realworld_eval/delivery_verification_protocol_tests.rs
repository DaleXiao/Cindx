use super::*;

const TRACKED_PROTOCOL: &[u8] =
    include_bytes!("../../../../../benchmarks/agent/delivery-verification-protocol-v3.json");
const CONSUMED_V2_PROTOCOL: &[u8] =
    include_bytes!("../../../../../benchmarks/agent/delivery-verification-protocol-v2.json");
const CONSUMED_V1_PROTOCOL: &[u8] =
    include_bytes!("../../../../../benchmarks/agent/delivery-verification-protocol-v1.json");
const TRACKED_SUITE: &[u8] =
    include_bytes!("../../../../../benchmarks/agent/delivery-verification-v3.json");
const CONSUMED_V1_SUITE: &[u8] =
    include_bytes!("../../../../../benchmarks/agent/delivery-verification-v1.json");

fn tracked() -> ValidatedProtocol<'static> {
    parse_and_validate_protocol(TRACKED_PROTOCOL, TRACKED_SUITE)
        .expect("tracked delivery verification v3 authority should validate")
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

#[allow(clippy::too_many_arguments)]
fn counts(
    complete_cases: usize,
    control_failures: usize,
    treatment_only_wins: usize,
    control_only_losses: usize,
    unsupported_claim_wins: usize,
    omitted_obligation_wins: usize,
    contradiction_wins: usize,
    preservation_losses: usize,
) -> MatchedPairCounts {
    MatchedPairCounts {
        complete_cases,
        control_failures,
        treatment_only_wins,
        control_only_losses,
        structural_failures: 0,
        treatment_execution_failures: 0,
        unsupported_claim_wins,
        omitted_obligation_wins,
        contradiction_wins,
        preservation_losses,
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
        "8da322b9336ec69d9f37bf12ce8d7698af55572d8d4173eddc45e6c2080cc8e8"
    );
    assert_eq!(
        protocol.suite_sha256(),
        "5bd95ed736641f0120ceab038a01249ee246b4394e403fa5386ebe0e5de88bc7"
    );
    assert_eq!(
        protocol.manifest.instrumentation.request_mode,
        "non_streaming"
    );
    assert_eq!(
        protocol
            .manifest
            .instrumentation
            .reservation_digest_authorities,
        ["semantic_request_sha256", "wire_payload_sha256"]
    );
    assert_eq!(
        protocol.manifest.instrumentation.execution_journal_schema,
        crate::agent_realworld_eval::delivery_verification_execution_journal::DELIVERY_EXECUTION_JOURNAL_SCHEMA
    );
    assert_eq!(
        sha256_hex(CONSUMED_V1_PROTOCOL),
        "080229aa24f05aa7ac816773aa5b2eb1bba2d00f3d1ca6b757b617ab9e27f19f"
    );
    assert_eq!(
        sha256_hex(CONSUMED_V2_PROTOCOL),
        "3b1b758330403410486c28650f012b278859f680b55de23bc7aad0e09f6c4982"
    );
    assert_eq!(
        sha256_hex(CONSUMED_V1_SUITE),
        "b9672f7075d896e3c48673607c622604c0f8d6fb2b6df4499281b0741124fbcd"
    );
    eprintln!("{DELIVERY_VERIFICATION_PROTOCOL_SCHEMA}");
}

#[test]
fn agent_delivery_verification_protocol_contract_cases_are_balanced_ordered_and_seeded() {
    let protocol = tracked();
    for stratum in STRATA {
        let cases = protocol
            .cases()
            .filter(|case| case.stratum() == stratum)
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 8);
        assert!(cases.iter().all(|case| case.evidence().len() >= 2));
        assert!(cases.iter().all(|case| !case.seeded_candidate().is_empty()));
    }

    for (index, case) in protocol.cases().enumerate() {
        assert_eq!(case.ordinal(), index + 1);
        assert!(!case.id().is_empty());
        assert_eq!(case.stratum(), STRATA[index % 4]);
        assert_eq!(case.obligations()[0].obligation_ref.len(), 64);
        assert!(!case.objective().is_empty());
        let expected = if case.stratum() == DeliveryVerificationStratum::Preservation {
            OracleEvaluation::Passed
        } else {
            OracleEvaluation::Failed
        };
        assert_eq!(case.evaluate_output(case.seeded_candidate()), expected);
    }
}

#[test]
fn agent_delivery_verification_protocol_contract_model_input_exposes_contract_not_oracle_values() {
    let protocol = tracked();
    for case in protocol.cases() {
        let input: Value = serde_json::from_slice(&case.model_input_bytes().unwrap()).unwrap();
        let object = input.as_object().unwrap();
        assert_eq!(object.len(), 5);
        for key in [
            "objective",
            "outputContract",
            "obligations",
            "evidence",
            "seededCandidate",
        ] {
            assert!(object.contains_key(key));
        }
        let text = serde_json::to_string(&input).unwrap();
        for hidden_key in [
            "oracle",
            "hiddenFromModel",
            "hiddenMetadataFromModel",
            "expectedFindingKind",
            "exactJson",
        ] {
            assert!(!text.contains(hidden_key));
        }
    }

    let mut suite: DeliveryVerificationSuite = serde_json::from_slice(TRACKED_SUITE).unwrap();
    suite.cases[0].oracle.exact_json["marketable_crates"] = Value::from(999_999);
    let sentinel_case = ValidatedCase {
        case: &suite.cases[0],
        case_sha256: "fixture-case-sha256",
    };
    let input = String::from_utf8(sentinel_case.model_input_bytes().unwrap()).unwrap();
    assert!(!input.contains("999999"));
}

#[test]
fn agent_delivery_verification_protocol_contract_oracle_is_unique_json_and_exact() {
    let protocol = tracked();
    let first = protocol.cases().next().unwrap();
    assert_eq!(
        first.evaluate_output(r#"{"marketable_crates":212,"grove":"Lark-Field"}"#),
        OracleEvaluation::Passed
    );
    assert_eq!(
        first.evaluate_output(first.seeded_candidate()),
        OracleEvaluation::Failed
    );
    assert_eq!(
        first.evaluate_output(r#"{"grove":"Lark-Field","marketable_crates":211}"#),
        OracleEvaluation::Failed
    );
    assert_eq!(
        first.evaluate_output(
            r#"{"grove":"Lark-Field","marketable_crates":999,"marketable_crates":212}"#
        ),
        OracleEvaluation::Failed
    );
    assert!(parse_unique_json(r#"{"outer":{"value":1,"value":2}}"#).is_err());
    assert_eq!(first.evaluate_output("not-json"), OracleEvaluation::Failed);
}

#[test]
fn agent_delivery_verification_protocol_contract_hashes_bind_seeds_inputs_contracts_and_oracle() {
    let protocol = tracked();
    assert_eq!(protocol.case_sha256s().len(), 32);
    assert_eq!(
        protocol.case_sha256s().first().unwrap(),
        "f26d8834e291f3a0f7e1f949a216d387f46f66c509bc2629f02d4e771652ba47"
    );
    assert_eq!(
        protocol.case_sha256s().last().unwrap(),
        "4b57671a7ba7dcd5444c66080df979b3ba1577ee43a0b250a4f77dcaa9766bb0"
    );
    assert_eq!(
        protocol.case_order_sha256(),
        "e2073c98fd878e5e50fda7d076b743b132dee705e40ea6ce26de8d7b31bb8bdc"
    );
    assert_eq!(
        protocol.hidden_oracle_sha256(),
        "1993e95ea7227d9ce65cd1c920d2f7698545341910b6f50d50070094220ec449"
    );
    assert_eq!(
        protocol.seeded_candidates_sha256(),
        "55c5d576ffe2fd06d504c186e4227998b9695b28ab58c546c566e61ce1b954df"
    );
    assert_eq!(
        protocol.model_inputs_sha256(),
        "6c0b7692fa283c056ef5cabe93c3fd116fbffde3ec33a84b10e76539d75d53ed"
    );
    assert_eq!(
        protocol.output_contracts_sha256(),
        "991d14b5f0d9b6c5e1269f8ad1c8001688769eba4040125eaf4f1b0eae24eb92"
    );
    assert_eq!(
        protocol.budget_sha256(),
        "eb3ae14a6e0cd452b106f8c5bd85ad2575e204b0aee08ce1ce23b4a7ef007e11"
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
fn agent_delivery_verification_protocol_contract_rejects_suite_seed_contract_or_oracle_drift() {
    let mut changed = suite_value();
    changed["cases"][0]["objective"] = Value::String("changed objective".into());
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(changed)).is_err());

    let mut reordered = suite_value();
    reordered["cases"].as_array_mut().unwrap().swap(0, 1);
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(reordered)).is_err());

    let mut exposed = suite_value();
    exposed["cases"][0]["oracle"]["hiddenFromModel"] = Value::Bool(false);
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(exposed)).is_err());

    let mut seed_drift = suite_value();
    seed_drift["cases"][0]["seed"]["candidate"] = Value::String("{}".into());
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(seed_drift)).is_err());

    let mut contract_drift = suite_value();
    contract_drift["cases"][0]["outputContract"]["properties"][0]["name"] =
        Value::String("wrong_key".into());
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(contract_drift)).is_err());

    let mut credential = suite_value();
    credential["cases"][0]["evidence"][0]["content"] =
        Value::String("raw marker ghp_fixture_must_fail_closed".into());
    assert!(parse_and_validate_protocol(TRACKED_PROTOCOL, &encoded(credential)).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_budget_topology_and_freeze_are_fixed() {
    let protocol = tracked();
    assert_eq!(protocol.budget().max_logical_model_calls_per_case, 3);
    assert_eq!(protocol.budget().max_logical_model_calls_total, 96);
    assert_eq!(protocol.budget().max_physical_model_attempts_total, 96);
    assert_eq!(protocol.budget().transport_retries, 0);
    assert!(protocol.manifest.design.seeded_control);
    assert_eq!(protocol.manifest.design.calibration_seeded_defects, 6);
    assert_eq!(protocol.manifest.design.calibration_clean_sentinels, 2);
    assert_eq!(protocol.manifest.design.holdout_seeded_defects, 18);
    assert_eq!(protocol.manifest.design.holdout_clean_sentinels, 6);
    assert_eq!(protocol.manifest.design.max_owner_repairs_per_case, 1);
    assert_eq!(protocol.manifest.design.max_verifier_rechecks_per_case, 1);
    assert_eq!(
        protocol
            .manifest
            .calibration_gate
            .prompt_model_threshold_changes_after_calibration,
        "invalidate_protocol"
    );
    assert!(protocol.manifest.holdout_decision.finite_frozen_suite_only);

    let mut changed = protocol_value();
    changed["budget"]["transport_retries"] = Value::from(1);
    assert!(parse_and_validate_protocol(&encoded(changed), TRACKED_SUITE).is_err());

    let mut changed = protocol_value();
    changed["instrumentation"]["case_telemetry"][0] = Value::String("omitted".into());
    assert!(parse_and_validate_protocol(&encoded(changed), TRACKED_SUITE).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_calibration_gate_is_stratified_and_fail_closed() {
    assert_eq!(
        calibration_decision(counts(8, 6, 4, 0, 2, 1, 1, 0)),
        CalibrationDecision::OpenHoldout
    );
    assert_eq!(
        calibration_decision(counts(8, 6, 3, 0, 1, 1, 1, 0)),
        CalibrationDecision::TerminalFutility
    );
    assert_eq!(
        calibration_decision(counts(8, 6, 4, 0, 0, 2, 2, 0)),
        CalibrationDecision::TerminalFutility
    );
    assert_eq!(
        calibration_decision(counts(8, 6, 4, 1, 2, 1, 1, 1)),
        CalibrationDecision::TerminalFutility
    );
    assert_eq!(
        calibration_decision(counts(8, 5, 4, 0, 2, 1, 1, 0)),
        CalibrationDecision::Invalid
    );
    assert_eq!(
        calibration_decision(counts(7, 6, 4, 0, 2, 1, 1, 0)),
        CalibrationDecision::Inconclusive
    );
    let mut structural = counts(7, 6, 4, 0, 2, 1, 1, 0);
    structural.structural_failures = 1;
    assert_eq!(
        calibration_decision(structural),
        CalibrationDecision::Inconclusive
    );
}

#[test]
fn agent_delivery_verification_protocol_contract_holdout_requires_joint_repair_and_preservation() {
    assert_eq!(
        holdout_decision(counts(24, 18, 13, 0, 5, 4, 4, 0)),
        HoldoutDecisionResult::SeededRepairEffective
    );
    assert_eq!(
        holdout_decision(counts(24, 18, 12, 0, 4, 4, 4, 0)),
        HoldoutDecisionResult::NotEffective
    );
    assert_eq!(
        holdout_decision(counts(24, 18, 13, 0, 6, 4, 3, 0)),
        HoldoutDecisionResult::NotEffective
    );
    assert_eq!(
        holdout_decision(counts(24, 18, 13, 1, 5, 4, 4, 1)),
        HoldoutDecisionResult::PreservationRegression
    );
    assert_eq!(
        holdout_decision(counts(23, 18, 13, 0, 5, 4, 4, 0)),
        HoldoutDecisionResult::Inconclusive
    );
    assert_eq!(
        holdout_decision(counts(24, 17, 13, 0, 5, 4, 4, 0)),
        HoldoutDecisionResult::Invalid
    );
}
