use super::preflight::*;
use super::*;
use agent_application::{CollaborationLearningArmOrderV1, CollaborationLearningSplitV1};

const MANIFEST_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../benchmarks/agent/collaboration-successor-protocol-v1.json"
));
const SUITE_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../benchmarks/agent/collaboration-successor-v1.json"
));
const V7_SUITE_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../benchmarks/agent/workflow-gepa-v7.json"
));

fn protocol() -> ValidatedProtocol<'static> {
    parse_and_validate_protocol(MANIFEST_BYTES, SUITE_BYTES).expect("tracked successor protocol")
}

fn provider_config() -> ProviderConfig {
    ProviderConfig {
        provider_id: "fixture-provider".into(),
        provider_resource: "fixture-resource".into(),
        base_url: "https://provider.invalid/v1".into(),
        api_key: "fixture-secret-that-must-not-be-hashed".into(),
        model: "owner-model".into(),
        conductor_model: "reasoning-model".into(),
        planner_model: "planner-model".into(),
        executor_model: "specialist-model".into(),
        reviewer_model: "verifier-model".into(),
        summarizer_model: "utility-model".into(),
        embedding_model: "embedding-model".into(),
        collaboration_policy: "auto_router".into(),
        context_window_tokens: 128_000,
        agent_system_prompt: "fixture system prompt".into(),
        ..ProviderConfig::default()
    }
}

fn observed_binding_for_cell<'a>(
    protocol: &'a ValidatedProtocol<'_>,
    receipt: &'a SuccessorPreflightReceipt,
    cell_index: usize,
) -> ObservedPairBinding<'a> {
    let cell = &protocol.manifest.matrix.cells[cell_index];
    let materialized = &receipt.cells[cell_index];
    ObservedPairBinding {
        split: match cell.split.as_str() {
            "train" => CollaborationLearningSplitV1::Train,
            "holdout" => CollaborationLearningSplitV1::Holdout,
            _ => unreachable!("validated successor split"),
        },
        replicate: u16::try_from(cell.replicate).unwrap(),
        arm_order: match cell.arm_order.as_str() {
            "direct_first" => CollaborationLearningArmOrderV1::DirectFirst,
            "workflow_first" => CollaborationLearningArmOrderV1::WorkflowFirst,
            _ => unreachable!("validated successor arm order"),
        },
        source_commit_sha256: &receipt.source_commit_sha256,
        suite_sha256: &receipt.suite_sha256,
        case_sha256: &cell.case_input_sha256,
        prestate_sha256: &materialized.workspace_prestate_sha256,
        provider_sha256: &receipt.provider.provider_identity_sha256,
        model_pool_sha256: &receipt.provider.capture_model_pool_sha256,
        provider_config_sha256: &receipt.provider.provider_config_sha256,
        role_model_sha256: &receipt.provider.role_model_sha256,
        ordered_model_pool_sha256: &receipt.provider.ordered_model_pool_sha256,
        ordered_model_member_sha256: &receipt.provider.ordered_model_member_sha256,
        budget_sha256: &receipt.outcome_budget_sha256,
        cohort_sha256: &receipt.cohort_sha256,
        workflow_policy_sha256: match cell.workflow_policy.as_str() {
            "baseline" => &protocol.manifest.policies.baseline_workflow_policy_sha256,
            "candidate" => &protocol.manifest.policies.candidate_workflow_policy_sha256,
            _ => unreachable!("validated successor workflow policy"),
        },
    }
}

#[test]
fn agent_collaboration_successor_protocol_contract_freezes_three_pairs_six_runs_and_executable_product_budget(
) {
    println!("{PROTOCOL_SCHEMA}");
    let protocol = protocol();
    assert_eq!(protocol.manifest.matrix.pair_count, 3);
    assert_eq!(protocol.manifest.matrix.run_count, 6);
    assert_eq!(protocol.manifest.policies.candidate_count, 1);
    assert_eq!(protocol.manifest.admission.candidate_budget, 1);
    assert!(
        !protocol
            .manifest
            .stop_contract
            .started_physical_run_provider_retry
    );
    assert_eq!(
        protocol.manifest.run_budget,
        freeze_run_budget(workflow_gepa_product_budget())
    );
    assert_eq!(
        protocol.manifest.campaign_budget,
        campaign_budget(&protocol.manifest.run_budget, 6).unwrap()
    );
}

#[test]
fn agent_collaboration_successor_protocol_contract_binds_new_case_inputs_and_full_contracts() {
    let protocol = protocol();
    for cell in &protocol.manifest.matrix.cells {
        let case = protocol
            .suite
            .cases
            .iter()
            .find(|case| case.id == cell.case_id)
            .unwrap();
        assert_eq!(cell.case_input_sha256, case_input_sha256(case));
        assert_eq!(
            cell.case_contract_sha256,
            case_contract_sha256(SUITE_BYTES, &cell.case_id).unwrap()
        );
    }

    let v7: RealworldSuite = serde_json::from_slice(V7_SUITE_BYTES).unwrap();
    let successor_ids = protocol
        .suite
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    let successor_inputs = protocol
        .suite
        .cases
        .iter()
        .map(case_input_sha256)
        .collect::<BTreeSet<_>>();
    assert!(v7
        .cases
        .iter()
        .all(|case| !successor_ids.contains(case.id.as_str())));
    assert!(v7
        .cases
        .iter()
        .all(|case| !successor_inputs.contains(&case_input_sha256(case))));

    let mut unknown_top: serde_json::Value = serde_json::from_slice(SUITE_BYTES).unwrap();
    unknown_top["ignored_protocol_override"] = serde_json::json!(true);
    let error =
        parse_and_validate_protocol(MANIFEST_BYTES, &serde_json::to_vec(&unknown_top).unwrap())
            .err()
            .expect("unknown top-level fields must fail closed");
    assert!(error.contains("unknown field ignored_protocol_override"));

    let mut unknown_nested: serde_json::Value = serde_json::from_slice(SUITE_BYTES).unwrap();
    unknown_nested["cases"][0]["verification"]["ignored_check"] = serde_json::json!("pass");
    let error = parse_and_validate_protocol(
        MANIFEST_BYTES,
        &serde_json::to_vec(&unknown_nested).unwrap(),
    )
    .err()
    .expect("unknown nested fields must fail closed");
    assert!(error.contains("unknown field ignored_check"));
}

#[test]
fn agent_collaboration_successor_protocol_contract_rejects_manifest_tampering() {
    let mut manifest: serde_json::Value = serde_json::from_slice(MANIFEST_BYTES).unwrap();
    manifest["admission"]["candidate_budget"] = serde_json::json!(2);
    let tampered = serde_json::to_vec(&manifest).unwrap();
    assert!(parse_and_validate_protocol(&tampered, SUITE_BYTES).is_err());

    let mut manifest: serde_json::Value = serde_json::from_slice(MANIFEST_BYTES).unwrap();
    manifest["matrix"]["cells"][0]["case_contract_sha256"] = serde_json::json!("0".repeat(64));
    let tampered = serde_json::to_vec(&manifest).unwrap();
    assert!(parse_and_validate_protocol(&tampered, SUITE_BYTES).is_err());
}

#[test]
fn agent_collaboration_successor_protocol_contract_matches_capture_without_secret_derivation() {
    let config = provider_config();
    let binding = provider_binding(&config).unwrap();
    assert!(binding.credential_present);
    assert_eq!(
        binding.provider_identity_sha256,
        sha256_hex(format!("{}\0{}", config.provider_id, config.base_url).as_bytes())
    );
    let capture_pool = super::super::configured_models(&config)
        .into_values()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    assert_eq!(
        binding.capture_model_pool_sha256,
        sha256_hex(&serde_json::to_vec(&capture_pool).unwrap())
    );
    assert_eq!(binding.ordered_model_member_sha256.len(), 3);

    let encoded = serde_json::to_string(&binding).unwrap();
    assert!(!encoded.contains(&config.api_key));
    assert!(!encoded.contains(&config.base_url));
    assert!(!encoded.contains("specialist-model"));

    let mut other_credential = config.clone();
    other_credential.api_key = "different-secret".into();
    assert_eq!(
        provider_binding(&other_credential)
            .unwrap()
            .provider_config_sha256,
        binding.provider_config_sha256
    );

    let mut missing_credential = config;
    missing_credential.api_key.clear();
    assert!(provider_binding(&missing_credential).is_err());
}

#[test]
fn agent_collaboration_successor_protocol_contract_binds_the_full_model_catalog() {
    let config = provider_config();
    let binding = provider_binding(&config).unwrap();
    let mut changed = config;
    changed.reviewer_model = "different-verifier".into();
    let changed = provider_binding(&changed).unwrap();
    assert_ne!(
        binding.provider_config_sha256,
        changed.provider_config_sha256
    );
    assert_ne!(
        binding.capture_model_pool_sha256,
        changed.capture_model_pool_sha256
    );
    assert_ne!(
        binding.ordered_model_pool_sha256,
        changed.ordered_model_pool_sha256
    );

    let mut incomplete = provider_config();
    incomplete.reviewer_model = incomplete.executor_model.clone();
    incomplete.summarizer_model = incomplete.executor_model.clone();
    assert!(provider_binding(&incomplete).is_err());
}

#[test]
fn agent_collaboration_successor_protocol_contract_materializes_without_authorizing_execution() {
    let protocol = protocol();
    let cells = materialize_cells(&protocol).unwrap();
    assert_eq!(cells.len(), 3);
    assert!(cells
        .iter()
        .all(|cell| cell.workspace_prestate_sha256.len() == 64));
    let receipt = build_receipt(
        &protocol,
        SourceBindingReceipt {
            head: "1".repeat(40),
            tree: "2".repeat(40),
        },
        provider_binding(&provider_config()).unwrap(),
        cells,
        Path::new("/private/tmp/cindx-successor-output"),
        123,
    )
    .unwrap();
    assert_eq!(receipt.app_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(receipt.provider_calls_performed, 0);
    assert!(!receipt.execution_authorized);
    assert!(receipt.online_execution_requires_explicit_authorization);
    assert_eq!(
        receipt.receipt_sha256,
        receipt_payload_sha256(&receipt).unwrap()
    );
}

#[test]
fn agent_collaboration_successor_protocol_contract_validates_pair_bindings_and_tampering() {
    let protocol = protocol();
    let receipt = build_receipt(
        &protocol,
        SourceBindingReceipt {
            head: "1".repeat(40),
            tree: "2".repeat(40),
        },
        provider_binding(&provider_config()).unwrap(),
        materialize_cells(&protocol).unwrap(),
        Path::new("/private/tmp/cindx-successor-output"),
        123,
    )
    .unwrap();
    assert!(validate_observed_pair_binding(
        &protocol,
        &receipt,
        1,
        observed_binding_for_cell(&protocol, &receipt, 0),
    )
    .is_ok());

    let foreign_suite_sha256 = "0".repeat(64);
    let mut foreign_pair = observed_binding_for_cell(&protocol, &receipt, 0);
    foreign_pair.suite_sha256 = &foreign_suite_sha256;
    assert!(validate_observed_pair_binding(&protocol, &receipt, 1, foreign_pair).is_err());

    let mut swapped_roles = receipt.provider.role_model_sha256.clone();
    let planner = swapped_roles.get("planner").unwrap().clone();
    let executor = swapped_roles.get("executor").unwrap().clone();
    swapped_roles.insert("planner".into(), executor);
    swapped_roles.insert("executor".into(), planner);
    let mut swapped_role_pair = observed_binding_for_cell(&protocol, &receipt, 0);
    swapped_role_pair.role_model_sha256 = &swapped_roles;
    assert!(validate_observed_pair_binding(&protocol, &receipt, 1, swapped_role_pair).is_err());

    let mut reordered_members = receipt.provider.ordered_model_member_sha256.clone();
    reordered_members.swap(0, 1);
    let mut reordered_pair = observed_binding_for_cell(&protocol, &receipt, 0);
    reordered_pair.ordered_model_member_sha256 = &reordered_members;
    assert!(validate_observed_pair_binding(&protocol, &receipt, 1, reordered_pair).is_err());

    let mut tampered_receipt = receipt.clone();
    tampered_receipt.cells[0].case_input_sha256 = "0".repeat(64);
    tampered_receipt.receipt_sha256 = receipt_payload_sha256(&tampered_receipt).unwrap();
    assert!(validate_observed_pair_binding(
        &protocol,
        &tampered_receipt,
        1,
        observed_binding_for_cell(&protocol, &tampered_receipt, 0),
    )
    .is_err());

    let mut arbitrary_source_digest = receipt;
    arbitrary_source_digest.source_commit_sha256 = "0".repeat(64);
    arbitrary_source_digest.receipt_sha256 =
        receipt_payload_sha256(&arbitrary_source_digest).unwrap();
    assert!(validate_observed_pair_binding(
        &protocol,
        &arbitrary_source_digest,
        1,
        observed_binding_for_cell(&protocol, &arbitrary_source_digest, 0),
    )
    .is_err());
}

#[test]
fn agent_collaboration_successor_protocol_contract_rejects_unsafe_external_paths() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let repo = repo.canonicalize().unwrap();
    let external = temp.path().join("receipt.json");
    let expected = temp.path().canonicalize().unwrap().join("receipt.json");
    assert_eq!(
        validate_new_external_path(&external, &repo, "receipt").unwrap(),
        expected
    );
    assert!(validate_new_external_path(&repo.join("receipt.json"), &repo, "receipt").is_err());
    fs::write(&external, b"existing").unwrap();
    assert!(validate_new_external_path(&external, &repo, "receipt").is_err());
    assert!(write_new_private_file_atomically(&external, b"replacement").is_err());
    assert_eq!(fs::read(&external).unwrap(), b"existing");
}

#[test]
fn agent_collaboration_successor_protocol_contract_has_no_execution_or_provider_construction() {
    let sources = [
        include_str!("collaboration_successor_protocol.rs"),
        include_str!("collaboration_successor_preflight.rs"),
    ];
    for forbidden in [
        "execute_case(",
        "build_evaluation_app",
        "OpenAiCompatibleProvider",
        "model_provider::",
        "--execute",
    ] {
        assert!(
            sources.iter().all(|source| !source.contains(forbidden)),
            "forbidden preflight path: {forbidden}"
        );
    }
    let manifest = std::str::from_utf8(MANIFEST_BYTES).unwrap();
    for dynamic in [
        "\"source_head\"",
        "\"source_tree\"",
        "\"provider_identity_sha256\"",
        "\"provider_config_sha256\"",
        "\"app_version\"",
    ] {
        assert!(
            !manifest.contains(dynamic),
            "dynamic manifest field: {dynamic}"
        );
    }
}
