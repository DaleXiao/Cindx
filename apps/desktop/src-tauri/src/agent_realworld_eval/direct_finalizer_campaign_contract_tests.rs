use super::*;

fn frozen_suite_bytes() -> &'static [u8] {
    include_bytes!("../../../../../benchmarks/agent/direct-finalizer-gepa-v1.json")
}

fn valid_suite() -> DirectFinalizerCampaignSuite {
    parse_and_validate_direct_finalizer_suite(frozen_suite_bytes()).unwrap()
}

#[test]
fn frozen_suite_satisfies_the_bounded_campaign_contract() {
    let suite = valid_suite();
    assert_eq!(suite.train_cases().count(), 6);
    assert_eq!(suite.holdout_cases().count(), 8);
    assert_eq!(suite.gate_a_cases().len(), 4);
    assert!(suite
        .gate_a_cases()
        .iter()
        .all(|case| case.split == DirectFinalizerCaseSplit::Train));
    assert_eq!(
        suite
            .train_cases()
            .filter(|case| verify_direct_finalizer_output(case, &case.actor_draft).passed)
            .count(),
        2
    );
    assert_eq!(
        suite
            .holdout_cases()
            .filter(|case| verify_direct_finalizer_output(case, &case.actor_draft).passed)
            .count(),
        3
    );
}

#[test]
fn suite_rejects_duplicate_overlap_and_invalid_gate_a_contracts() {
    let mut duplicate = valid_suite();
    duplicate.cases[1].id = duplicate.cases[0].id.clone();
    let encoded = serde_json::to_vec(&duplicate).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("duplicate"));

    let mut holdout_gate = valid_suite();
    let holdout_id = holdout_gate.holdout_cases().next().unwrap().id.clone();
    holdout_gate.gate_a_case_ids[0] = holdout_id;
    let encoded = serde_json::to_vec(&holdout_gate).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("not in the train split"));

    let mut duplicate_gate = valid_suite();
    duplicate_gate.gate_a_case_ids[1] = duplicate_gate.gate_a_case_ids[0].clone();
    let encoded = serde_json::to_vec(&duplicate_gate).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("4 unique"));
}

#[test]
fn suite_rejects_wrong_counts_classes_and_empty_verification_terms() {
    let mut wrong_count = valid_suite();
    wrong_count.cases.pop();
    let encoded = serde_json::to_vec(&wrong_count).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("exactly 8 holdout"));

    let mut one_class = valid_suite();
    for case in &mut one_class.cases {
        case.task_class = "one_class".to_string();
    }
    let encoded = serde_json::to_vec(&one_class).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("at least 2 task classes in each split"));

    let mut empty_group = valid_suite();
    empty_group.cases[0].required_any_groups[0].clear();
    let encoded = serde_json::to_vec(&empty_group).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("required group 0 is empty"));

    let mut no_contract = valid_suite();
    no_contract.cases[0].required_any_groups.clear();
    no_contract.cases[0].forbidden_terms.clear();
    no_contract.cases[0].exact_json = None;
    let encoded = serde_json::to_vec(&no_contract).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("no deterministic verification contract"));

    let mut no_preservation_controls = valid_suite();
    for case in &mut no_preservation_controls.cases {
        case.actor_draft = "invalid frozen draft".to_string();
    }
    let encoded = serde_json::to_vec(&no_preservation_controls).unwrap();
    assert!(parse_and_validate_direct_finalizer_suite(&encoded)
        .unwrap_err()
        .contains("preservation and correction"));
}

#[test]
fn deterministic_verifier_is_case_insensitive_and_reports_only_rule_indexes() {
    let suite = valid_suite();
    let case = suite
        .cases
        .iter()
        .find(|case| case.id == "train-test-failure")
        .unwrap();
    let passed = verify_direct_finalizer_output(
        case,
        "ONE FAILED: RETRY_STOPS_AFTER_BUDGET. The other 47 passed.",
    );
    assert!(passed.passed);
    assert_eq!(passed.required_groups_passed, 2);
    assert_eq!(passed.forbidden_terms_absent, 2);

    let failed = verify_direct_finalizer_output(case, "All tests passed");
    assert!(!failed.passed);
    assert_eq!(
        failed.failures,
        vec![
            DirectFinalizerVerificationFailure::RequiredGroupMissing { group_index: 0 },
            DirectFinalizerVerificationFailure::RequiredGroupMissing { group_index: 1 },
            DirectFinalizerVerificationFailure::ForbiddenTermPresent { term_index: 1 },
        ]
    );
    let serialized = serde_json::to_string(&failed).unwrap();
    assert!(!serialized.contains("retry_stops_after_budget"));
    assert!(!serialized.contains("all tests passed"));
}

#[test]
fn exact_json_requires_the_complete_output_and_strict_value_equality() {
    let suite = valid_suite();
    let case = suite
        .cases
        .iter()
        .find(|case| case.id == "train-json-contract")
        .unwrap();
    let passed =
        verify_direct_finalizer_output(case, "  {\"failed_checks\":2,\"status\":\"blocked\"}\n");
    assert!(passed.passed);
    assert_eq!(passed.exact_json_passed, Some(true));

    let wrapped = verify_direct_finalizer_output(
        case,
        "```json\n{\"status\":\"blocked\",\"failed_checks\":2}\n```",
    );
    assert_eq!(wrapped.exact_json_passed, Some(false));
    assert_eq!(
        wrapped.failures,
        vec![DirectFinalizerVerificationFailure::ExactJsonInvalid]
    );

    let mismatch =
        verify_direct_finalizer_output(case, "{\"status\":\"blocked\",\"failed_checks\":3}");
    assert_eq!(mismatch.exact_json_passed, Some(false));
    assert_eq!(
        mismatch.failures,
        vec![DirectFinalizerVerificationFailure::ExactJsonMismatch]
    );
}

fn sample_campaign_receipt() -> DirectFinalizerCampaignReceipt {
    DirectFinalizerCampaignReceipt {
        schema: DIRECT_FINALIZER_CAMPAIGN_RECEIPT_SCHEMA.to_string(),
        suite_id: DIRECT_FINALIZER_GEPA_SUITE_ID.to_string(),
        suite_version: DIRECT_FINALIZER_GEPA_SUITE_VERSION,
        suite_sha256: "suite-sha".to_string(),
        dataset_sha256: "dataset-sha".to_string(),
        cohort_sha256: "cohort-sha".to_string(),
        source_commit: "0".repeat(40),
        provider_id: "provider".to_string(),
        models: DirectFinalizerCampaignModels {
            producer_model: "producer".to_string(),
            reviewer_model: "reviewer".to_string(),
            gepa_model: "gepa".to_string(),
        },
        parent_profile: DirectFinalizerProfileReceipt {
            profile_id: "stable".to_string(),
            profile_sha256: "stable-sha".to_string(),
        },
        candidate_profile: None,
        call_count: 0,
        gate_a: DirectFinalizerGateReceipt {
            protocol: "gate-a".to_string(),
            status: "not_run".to_string(),
            passed: false,
            evaluated_case_ids: Vec::new(),
            blocker_codes: vec!["not_run".to_string()],
        },
        gepa: DirectFinalizerGepaReceipt {
            attempts: 0,
            response_sha256: None,
            decision: None,
        },
        promotion: None,
        paired_evidence_sha256: None,
        snapshot_artifact_sha256: None,
        pairs: Vec::new(),
    }
}

#[test]
fn sanitized_campaign_receipt_has_no_raw_output_field_and_is_private() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("campaign.json");
    write_sanitized_direct_finalizer_campaign(&path, &sample_campaign_receipt()).unwrap();
    let encoded = std::fs::read_to_string(&path).unwrap();
    let value = serde_json::from_str::<Value>(&encoded).unwrap();
    assert_eq!(
        value["schema"],
        Value::String(DIRECT_FINALIZER_CAMPAIGN_RECEIPT_SCHEMA.to_string())
    );
    assert!(!encoded.contains("\"output\""));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn sanitized_campaign_receipt_records_the_exact_gepa_decision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("campaign.json");
    let mut receipt = sample_campaign_receipt();
    receipt.gepa = DirectFinalizerGepaReceipt {
        attempts: 1,
        response_sha256: Some("a".repeat(64)),
        decision: Some(orchestrator::DirectFinalizerGepaDecision::Promote),
    };

    write_sanitized_direct_finalizer_campaign(&path, &receipt).unwrap();
    let value = serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value["gepa"]["decision"], Value::String("promote".into()));
    assert_eq!(value["gepa"]["attempts"], Value::Number(1.into()));
}

#[test]
fn sanitized_campaign_writer_rejects_relative_paths_and_unknown_schema() {
    let relative_error = write_sanitized_direct_finalizer_campaign(
        Path::new("campaign.json"),
        &sample_campaign_receipt(),
    )
    .unwrap_err();
    assert!(relative_error.contains("must be absolute"));

    let directory = tempfile::tempdir().unwrap();
    let mut receipt = sample_campaign_receipt();
    receipt.schema = "unknown".to_string();
    let schema_error = write_sanitized_direct_finalizer_campaign(
        &directory.path().join("campaign.json"),
        &receipt,
    )
    .unwrap_err();
    assert!(schema_error.contains("unsupported"));
}

#[test]
fn sanitized_campaign_writer_requires_candidate_identity_after_gate_a_runs() {
    let directory = tempfile::tempdir().unwrap();
    let mut receipt = sample_campaign_receipt();
    receipt.gate_a.status = "blocked".to_string();
    receipt.gate_a.blocker_codes = vec!["candidate_regression".to_string()];

    let error = write_sanitized_direct_finalizer_campaign(
        &directory.path().join("campaign.json"),
        &receipt,
    )
    .unwrap_err();

    assert!(error.contains("requires a candidate profile"));
}
