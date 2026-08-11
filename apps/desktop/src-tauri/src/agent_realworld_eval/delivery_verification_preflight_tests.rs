use super::*;
use std::fs;

const RUNNER_BYTES: &[u8] = b"provider-free exact delivery execute fixture";

fn digest(label: &str) -> String {
    sha256_hex(label.as_bytes())
}

fn runner_binding() -> DeliveryVerificationRunnerBinary {
    DeliveryVerificationRunnerBinary {
        bytes: RUNNER_BYTES.to_vec(),
        code_directory_sha256: digest("runner code directory"),
    }
}

fn protocol() -> DeliveryVerificationProtocolSnapshot {
    DeliveryVerificationProtocolSnapshot {
        protocol_id: "cindx-delivery-verification-protocol-v4".into(),
        suite_id: "cindx-delivery-verification-v3".into(),
        manifest_sha256: digest("manifest"),
        suite_sha256: digest("suite"),
        case_sha256: vec![digest("case-1"), digest("case-2")],
        case_order_sha256: digest("case-order"),
        seeded_candidates_sha256: digest("seeded-candidates"),
        model_inputs_sha256: digest("model-inputs"),
        output_contracts_sha256: digest("output-contracts"),
        budget_sha256: digest("budget"),
        hidden_oracle_sha256: digest("hidden-oracle"),
        execution_authorized: false,
    }
}

fn source() -> DeliveryVerificationSourceBindingReceipt {
    DeliveryVerificationSourceBindingReceipt {
        head: "0123456789abcdef0123456789abcdef01234567".into(),
        tree: "89abcdef0123456789abcdef0123456789abcdef".into(),
    }
}

fn provider_config() -> ProviderConfig {
    ProviderConfig {
        provider_id: "openai-compatible".into(),
        provider_resource: "resource-a".into(),
        base_url: "https://provider.example/v1".into(),
        api_key: "fixture-secret-never-serialized".into(),
        executor_model: "owner-model".into(),
        reviewer_model: "reviewer-model".into(),
        context_window_tokens: 131_072,
        ..ProviderConfig::default()
    }
}

fn receipt() -> DeliveryVerificationPreflightReceipt {
    build_receipt(
        &protocol(),
        source(),
        provider_binding(&provider_config()).unwrap(),
        &runner_binding(),
        Path::new("/private/tmp/cindx-delivery-output"),
        1_800_000_000_000,
    )
    .unwrap()
}

fn rehash(receipt: &mut DeliveryVerificationPreflightReceipt) {
    receipt.receipt_sha256 = receipt_digest(receipt).unwrap();
}

#[test]
fn agent_delivery_verification_protocol_contract_preflight_is_provider_free_and_non_authorizing() {
    let protocol = protocol();
    let receipt = receipt();
    validate_receipt(&protocol, &receipt).unwrap();
    assert_eq!(receipt.schema, PREFLIGHT_SCHEMA);
    assert_eq!(receipt.provider_calls_performed, 0);
    assert!(receipt.online_runner_frozen);
    assert!(!receipt.execution_authorized);
    assert!(!receipt.online_execution_requires_new_frozen_runner);
    assert!(receipt.online_execution_requires_explicit_authorization);
    assert_eq!(receipt.case_sha256, protocol.case_sha256);
    assert_eq!(receipt.case_order_sha256, protocol.case_order_sha256);
    assert_eq!(
        receipt.seeded_candidates_sha256,
        protocol.seeded_candidates_sha256
    );
    assert_eq!(receipt.model_inputs_sha256, protocol.model_inputs_sha256);
    assert_eq!(
        receipt.output_contracts_sha256,
        protocol.output_contracts_sha256
    );
    assert_eq!(receipt.hidden_oracle_sha256, protocol.hidden_oracle_sha256);
    assert_eq!(
        receipt.protocol_id,
        "cindx-delivery-verification-protocol-v4"
    );
    assert_eq!(receipt.suite_id, "cindx-delivery-verification-v3");
    assert_eq!(
        receipt.runner_binary,
        DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME
    );
    assert_eq!(receipt.runner_sha256, sha256_hex(RUNNER_BYTES));
    assert_eq!(receipt.runner_bytes, RUNNER_BYTES.len() as u64);
    assert_eq!(
        receipt.runner_code_directory_sha256,
        runner_binding().code_directory_sha256
    );
    assert_eq!(
        OUTPUT_ROOT_ENV,
        "CINDX_DELIVERY_VERIFICATION_V4_OUTPUT_ROOT"
    );
    assert_eq!(
        RECEIPT_ENV,
        "CINDX_DELIVERY_VERIFICATION_V4_PREFLIGHT_RECEIPT"
    );
    assert_eq!(
        RECEIPT_HASH_DOMAIN,
        b"cindx.agent-eval.delivery-verification-preflight.v4\0"
    );
    assert_eq!(CASES_HASH_DOMAIN, b"cindx.delivery-verification-cases.v4\0");
    eprintln!("{PREFLIGHT_SCHEMA}");
}

#[test]
fn agent_delivery_verification_protocol_contract_preflight_binds_source_cases_budgets_and_output() {
    let protocol = protocol();
    let receipt = receipt();
    assert_eq!(receipt.source, source());
    assert_eq!(
        receipt.source_commit_sha256,
        sha256_hex(source().head.as_bytes())
    );
    assert_eq!(receipt.manifest_sha256, protocol.manifest_sha256);
    assert_eq!(receipt.suite_sha256, protocol.suite_sha256);
    assert_eq!(
        receipt.cases_sha256,
        cases_digest(&protocol.case_sha256).unwrap()
    );
    assert_eq!(receipt.case_order_sha256, protocol.case_order_sha256);
    assert_eq!(
        receipt.seeded_candidates_sha256,
        protocol.seeded_candidates_sha256
    );
    assert_eq!(receipt.model_inputs_sha256, protocol.model_inputs_sha256);
    assert_eq!(
        receipt.output_contracts_sha256,
        protocol.output_contracts_sha256
    );
    assert_eq!(receipt.budget_sha256, protocol.budget_sha256);
    assert_eq!(
        receipt.output_root_sha256,
        path_sha256(Path::new("/private/tmp/cindx-delivery-output"))
    );
}

#[test]
fn agent_delivery_verification_protocol_contract_provider_binding_is_exact_redacted_and_resource_bound(
) {
    let config = provider_config();
    let binding = provider_binding(&config).unwrap();
    assert_eq!(binding.owner_role, "executor");
    assert_eq!(binding.verifier_role, "reviewer");
    assert_ne!(binding.owner_model_sha256, binding.verifier_model_sha256);

    let encoded = serde_json::to_string(&binding).unwrap();
    for secret in [
        config.api_key.as_str(),
        config.provider_id.as_str(),
        config.provider_resource.as_str(),
        config.base_url.as_str(),
        config.executor_model.as_str(),
        config.reviewer_model.as_str(),
    ] {
        assert!(!encoded.contains(secret));
    }

    let mut changed = config.clone();
    changed.provider_resource = "resource-b".into();
    assert_ne!(
        binding.provider_config_sha256,
        provider_binding(&changed).unwrap().provider_config_sha256
    );
    assert_ne!(
        binding.provider_identity_sha256,
        provider_binding(&changed).unwrap().provider_identity_sha256
    );

    changed = config.clone();
    changed.executor_model = "owner-model-v2".into();
    assert_ne!(
        binding.model_binding_sha256,
        provider_binding(&changed).unwrap().model_binding_sha256
    );
    assert_eq!(
        PROVIDER_CONFIG_HASH_DOMAIN,
        b"cindx.delivery-verification-provider-config.v4\0"
    );
    assert_eq!(
        PROVIDER_IDENTITY_HASH_DOMAIN,
        b"cindx.delivery-verification-provider-identity.v4\0"
    );
    assert_eq!(
        MODEL_BINDING_HASH_DOMAIN,
        b"cindx.delivery-verification-model-binding.v4\0"
    );
}

#[test]
fn agent_delivery_verification_protocol_contract_provider_binding_fails_closed() {
    let mut config = provider_config();
    config.api_key.clear();
    assert!(provider_binding(&config).is_err());

    config = provider_config();
    config.reviewer_model = config.executor_model.clone();
    assert!(provider_binding(&config).is_err());

    config = provider_config();
    config.executor_model = " owner-model".into();
    assert!(provider_binding(&config).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_preflight_rejects_authority_tamper_even_when_rehashed(
) {
    let protocol = protocol();

    let mut runner = receipt();
    runner.online_runner_frozen = false;
    rehash(&mut runner);
    assert!(validate_receipt(&protocol, &runner).is_err());

    let mut runner_digest = receipt();
    runner_digest.runner_sha256 = digest("changed-runner");
    rehash(&mut runner_digest);
    assert!(validate_snapshot(
        &protocol,
        &runner_digest,
        &source(),
        &provider_binding(&provider_config()).unwrap(),
        &runner_binding(),
        Path::new("/private/tmp/cindx-delivery-output"),
    )
    .is_err());

    let mut runner_code_directory = receipt();
    runner_code_directory.runner_code_directory_sha256 = digest("changed code directory");
    rehash(&mut runner_code_directory);
    assert!(validate_snapshot(
        &protocol,
        &runner_code_directory,
        &source(),
        &provider_binding(&provider_config()).unwrap(),
        &runner_binding(),
        Path::new("/private/tmp/cindx-delivery-output"),
    )
    .is_err());

    let mut authorization = receipt();
    authorization.execution_authorized = true;
    rehash(&mut authorization);
    assert!(validate_receipt(&protocol, &authorization).is_err());

    let mut calls = receipt();
    calls.provider_calls_performed = 1;
    rehash(&mut calls);
    assert!(validate_receipt(&protocol, &calls).is_err());

    let mut seeded_candidates = receipt();
    seeded_candidates.seeded_candidates_sha256 = digest("changed-seeded-candidates");
    rehash(&mut seeded_candidates);
    assert!(validate_receipt(&protocol, &seeded_candidates).is_err());

    let mut runner_required = receipt();
    runner_required.online_execution_requires_new_frozen_runner = true;
    rehash(&mut runner_required);
    assert!(validate_receipt(&protocol, &runner_required).is_err());

    let mut consumed_schema = receipt();
    consumed_schema.schema = "cindx.agent-eval.delivery-verification-preflight.v3".into();
    rehash(&mut consumed_schema);
    assert!(validate_receipt(&protocol, &consumed_schema).is_err());

    let mut consumed_protocol = receipt();
    consumed_protocol.protocol_id = "cindx-delivery-verification-protocol-v3".into();
    rehash(&mut consumed_protocol);
    assert!(validate_receipt(&protocol, &consumed_protocol).is_err());

    let mut consumed_runner = receipt();
    consumed_runner.runner_binary = "cindx-delivery-verification-v3-execute".into();
    rehash(&mut consumed_runner);
    assert!(validate_receipt(&protocol, &consumed_runner).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_preflight_rejects_protocol_and_receipt_drift() {
    let mut authorizing = protocol();
    authorizing.execution_authorized = true;
    assert!(build_receipt(
        &authorizing,
        source(),
        provider_binding(&provider_config()).unwrap(),
        &runner_binding(),
        Path::new("/private/tmp/cindx-delivery-output"),
        1_800_000_000_000,
    )
    .is_err());

    let protocol = protocol();
    let mut changed = protocol.clone();
    changed.case_sha256[0] = digest("replaced-case");
    assert!(validate_receipt(&changed, &receipt()).is_err());

    changed = protocol.clone();
    changed.budget_sha256 = digest("replaced-budget");
    assert!(validate_receipt(&changed, &receipt()).is_err());

    changed = protocol.clone();
    changed.hidden_oracle_sha256 = digest("replaced-oracle");
    assert!(validate_receipt(&changed, &receipt()).is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_preflight_encoding_is_canonical_and_secret_free() {
    let receipt = receipt();
    let source = source();
    let provider = provider_binding(&provider_config()).unwrap();
    let output_root = Path::new("/private/tmp/cindx-delivery-output");
    let encoded = encode_receipt(&receipt).unwrap();
    assert_eq!(encoded.last(), Some(&b'\n'));
    let decoded = parse_and_validate_receipt(
        &protocol(),
        &encoded,
        &source,
        &provider,
        &runner_binding(),
        output_root,
    )
    .unwrap();
    assert_eq!(decoded, receipt);
    let compact = serde_json::to_vec(&receipt).unwrap();
    assert!(parse_and_validate_receipt(
        &protocol(),
        &compact,
        &source,
        &provider,
        &runner_binding(),
        output_root,
    )
    .is_err());
    let text = String::from_utf8(encoded).unwrap();
    assert!(!text.contains("fixture-secret-never-serialized"));
    assert!(!text.contains("owner-model"));
    assert!(!text.contains("reviewer-model"));
    assert!(!text.contains("resource-a"));
}

#[test]
fn agent_delivery_verification_protocol_contract_preflight_snapshot_rejects_provider_and_output_drift(
) {
    let protocol = protocol();
    let receipt = receipt();
    let provider = provider_binding(&provider_config()).unwrap();
    validate_snapshot(
        &protocol,
        &receipt,
        &source(),
        &provider,
        &runner_binding(),
        Path::new("/private/tmp/cindx-delivery-output"),
    )
    .unwrap();

    let mut changed_config = provider_config();
    changed_config.reviewer_model = "reviewer-model-v2".into();
    assert!(validate_snapshot(
        &protocol,
        &receipt,
        &source(),
        &provider_binding(&changed_config).unwrap(),
        &runner_binding(),
        Path::new("/private/tmp/cindx-delivery-output"),
    )
    .is_err());
    assert!(validate_snapshot(
        &protocol,
        &receipt,
        &source(),
        &provider,
        &runner_binding(),
        Path::new("/private/tmp/replaced-output"),
    )
    .is_err());
}

#[test]
fn agent_delivery_verification_protocol_contract_preflight_receipt_is_private_new_file_only() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("preflight.json");
    let receipt = receipt();
    write_receipt(&path, &receipt).unwrap();
    assert!(write_receipt(&path, &receipt).is_err());
    let decoded: DeliveryVerificationPreflightReceipt =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(decoded, receipt);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
