use super::collaboration_successor_protocol::{
    clean_source_head_tree, now_millis, validate_new_external_path,
    write_new_private_file_atomically,
};
use super::delivery_verification_protocol::{
    parse_and_validate_protocol, ValidatedProtocol, DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
    DELIVERY_VERIFICATION_PROTOCOL_RELATIVE_PATH, DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH,
};
use super::delivery_verification_runner_binary::{
    read_delivery_execute_sibling, DeliveryVerificationRunnerBinary, MAX_RUNNER_BYTES,
};
use crate::configuration_models::ProviderConfig;
use agent_core::ModelRole;
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const OUTPUT_ROOT_ENV: &str = "CINDX_DELIVERY_VERIFICATION_V4_OUTPUT_ROOT";
const RECEIPT_ENV: &str = "CINDX_DELIVERY_VERIFICATION_V4_PREFLIGHT_RECEIPT";
pub(super) const PREFLIGHT_SCHEMA: &str = "cindx.agent-eval.delivery-verification-preflight.v4";
const RECEIPT_HASH_DOMAIN: &[u8] = b"cindx.agent-eval.delivery-verification-preflight.v4\0";
const CASES_HASH_DOMAIN: &[u8] = b"cindx.delivery-verification-cases.v4\0";
const PROVIDER_CONFIG_HASH_DOMAIN: &[u8] = b"cindx.delivery-verification-provider-config.v4\0";
const PROVIDER_IDENTITY_HASH_DOMAIN: &[u8] = b"cindx.delivery-verification-provider-identity.v4\0";
const MODEL_BINDING_HASH_DOMAIN: &[u8] = b"cindx.delivery-verification-model-binding.v4\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationSourceBindingReceipt {
    pub(super) head: String,
    pub(super) tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationProviderBindingReceipt {
    pub(super) credential_present: bool,
    pub(super) provider_config_sha256: String,
    pub(super) provider_identity_sha256: String,
    pub(super) provider_id_sha256: String,
    pub(super) provider_resource_sha256: String,
    pub(super) base_url_sha256: String,
    pub(super) owner_role: String,
    pub(super) owner_model_sha256: String,
    pub(super) verifier_role: String,
    pub(super) verifier_model_sha256: String,
    pub(super) model_binding_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryVerificationProtocolSnapshot {
    pub(super) protocol_id: String,
    pub(super) suite_id: String,
    pub(super) manifest_sha256: String,
    pub(super) suite_sha256: String,
    pub(super) case_sha256: Vec<String>,
    pub(super) case_order_sha256: String,
    pub(super) seeded_candidates_sha256: String,
    pub(super) model_inputs_sha256: String,
    pub(super) output_contracts_sha256: String,
    pub(super) budget_sha256: String,
    pub(super) hidden_oracle_sha256: String,
    pub(super) execution_authorized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationPreflightReceipt {
    pub(super) schema: String,
    pub(super) app_version: String,
    pub(super) protocol_id: String,
    pub(super) suite_id: String,
    pub(super) created_at_ms: u64,
    pub(super) source: DeliveryVerificationSourceBindingReceipt,
    pub(super) source_commit_sha256: String,
    pub(super) manifest_sha256: String,
    pub(super) suite_sha256: String,
    pub(super) case_sha256: Vec<String>,
    pub(super) cases_sha256: String,
    pub(super) case_order_sha256: String,
    pub(super) seeded_candidates_sha256: String,
    pub(super) model_inputs_sha256: String,
    pub(super) output_contracts_sha256: String,
    pub(super) budget_sha256: String,
    pub(super) hidden_oracle_sha256: String,
    pub(super) provider: DeliveryVerificationProviderBindingReceipt,
    pub(super) runner_binary: String,
    pub(super) runner_sha256: String,
    pub(super) runner_bytes: u64,
    pub(super) runner_code_directory_sha256: String,
    pub(super) output_root_sha256: String,
    pub(super) provider_calls_performed: u64,
    pub(super) online_runner_frozen: bool,
    pub(super) execution_authorized: bool,
    pub(super) online_execution_requires_new_frozen_runner: bool,
    pub(super) online_execution_requires_explicit_authorization: bool,
    pub(super) receipt_sha256: String,
}

pub(super) fn run_preflight() -> Result<(), String> {
    if std::env::args_os().len() != 1 {
        return Err("delivery verification preflight accepts no command-line arguments".into());
    }
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))?;
    let (head, tree) = clean_source_head_tree(&repo_root)?;
    let manifest_path = repo_root.join(DELIVERY_VERIFICATION_PROTOCOL_RELATIVE_PATH);
    let suite_path = repo_root.join(DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH);
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let suite_bytes = fs::read(&suite_path)
        .map_err(|error| format!("failed to read {}: {error}", suite_path.display()))?;
    let protocol = parse_and_validate_protocol(&manifest_bytes, &suite_bytes)?;
    let protocol_snapshot = protocol_snapshot(&protocol);
    let runner = read_delivery_execute_sibling()?;
    let (output_root, receipt_path) = preflight_paths(&repo_root)?;
    let provider = provider_binding(&crate::configuration_persistence::load_provider_config())?;
    let receipt = build_receipt(
        &protocol_snapshot,
        DeliveryVerificationSourceBindingReceipt { head, tree },
        provider,
        &runner,
        &output_root,
        now_millis()?,
    )?;

    let current_manifest = fs::read(&manifest_path)
        .map_err(|error| format!("failed to re-read {}: {error}", manifest_path.display()))?;
    let current_suite = fs::read(&suite_path)
        .map_err(|error| format!("failed to re-read {}: {error}", suite_path.display()))?;
    if current_manifest != manifest_bytes || current_suite != suite_bytes {
        return Err("delivery verification protocol changed during preflight".into());
    }
    let current_runner = read_delivery_execute_sibling()?;
    if current_runner != runner {
        return Err("delivery verification execute binary changed during preflight".into());
    }
    validate_current_authority(
        &protocol_snapshot,
        &receipt,
        &repo_root,
        &crate::configuration_persistence::load_provider_config(),
        &current_runner,
        &output_root,
    )?;
    let revalidated_receipt_path =
        validate_new_external_path(&receipt_path, &repo_root, "preflight receipt")?;
    if revalidated_receipt_path != receipt_path {
        return Err("delivery verification receipt path changed during preflight".into());
    }
    write_receipt(&revalidated_receipt_path, &receipt)?;
    eprintln!(
        "[delivery-verification-preflight] receipt={} digest={} runner={} provider_calls=0 online_runner_frozen=true execution_authorized=false",
        receipt_path.display(),
        receipt.receipt_sha256,
        receipt.runner_sha256,
    );
    Ok(())
}

pub(super) fn protocol_snapshot(
    protocol: &ValidatedProtocol<'_>,
) -> DeliveryVerificationProtocolSnapshot {
    DeliveryVerificationProtocolSnapshot {
        protocol_id: protocol.protocol_id().to_string(),
        suite_id: protocol.suite_id().to_string(),
        manifest_sha256: protocol.manifest_sha256().to_string(),
        suite_sha256: protocol.suite_sha256().to_string(),
        case_sha256: protocol.case_sha256s().to_vec(),
        case_order_sha256: protocol.case_order_sha256().to_string(),
        seeded_candidates_sha256: protocol.seeded_candidates_sha256().to_string(),
        model_inputs_sha256: protocol.model_inputs_sha256().to_string(),
        output_contracts_sha256: protocol.output_contracts_sha256().to_string(),
        budget_sha256: protocol.budget_sha256().to_string(),
        hidden_oracle_sha256: protocol.hidden_oracle_sha256().to_string(),
        execution_authorized: protocol.execution_authorized(),
    }
}

pub(super) fn provider_binding(
    config: &ProviderConfig,
) -> Result<DeliveryVerificationProviderBindingReceipt, String> {
    validate_identifier(&config.provider_id, "provider id")?;
    validate_identifier(&config.base_url, "provider base URL")?;
    if !config.provider_resource.is_empty() {
        validate_identifier(&config.provider_resource, "provider resource")?;
    }
    let credential_present = !config.api_key.trim().is_empty();
    if !credential_present {
        return Err("delivery verification preflight requires a configured credential".into());
    }

    let owner_model = config.model_for_role(&ModelRole::Executor);
    let verifier_model = config.model_for_role(&ModelRole::Reviewer);
    validate_identifier(&owner_model, "Executor Owner model")?;
    validate_identifier(&verifier_model, "Reviewer model")?;
    if owner_model == verifier_model {
        return Err(
            "delivery verification preflight requires model-distinct Owner and Reviewer".into(),
        );
    }

    let provider_identity = ProviderIdentityHashInput {
        provider_id: &config.provider_id,
        provider_resource: &config.provider_resource,
        base_url: &config.base_url,
    };
    let model_binding = ModelBindingHashInput {
        owner_role: "executor",
        owner_model: &owner_model,
        verifier_role: "reviewer",
        verifier_model: &verifier_model,
    };
    let provider_config = ProviderConfigHashInput {
        provider: provider_identity,
        credential_present,
        models: model_binding,
        context_window_tokens: config.context_window_tokens,
    };

    Ok(DeliveryVerificationProviderBindingReceipt {
        credential_present,
        provider_config_sha256: domain_hash_json(
            PROVIDER_CONFIG_HASH_DOMAIN,
            &provider_config,
            "provider config",
        )?,
        provider_identity_sha256: domain_hash_json(
            PROVIDER_IDENTITY_HASH_DOMAIN,
            &provider_identity,
            "provider identity",
        )?,
        provider_id_sha256: sha256_hex(config.provider_id.as_bytes()),
        provider_resource_sha256: sha256_hex(config.provider_resource.as_bytes()),
        base_url_sha256: sha256_hex(config.base_url.as_bytes()),
        owner_role: "executor".into(),
        owner_model_sha256: sha256_hex(owner_model.as_bytes()),
        verifier_role: "reviewer".into(),
        verifier_model_sha256: sha256_hex(verifier_model.as_bytes()),
        model_binding_sha256: domain_hash_json(
            MODEL_BINDING_HASH_DOMAIN,
            &model_binding,
            "model binding",
        )?,
    })
}

pub(super) fn build_receipt(
    protocol: &DeliveryVerificationProtocolSnapshot,
    source: DeliveryVerificationSourceBindingReceipt,
    provider: DeliveryVerificationProviderBindingReceipt,
    runner: &DeliveryVerificationRunnerBinary,
    output_root: &Path,
    created_at_ms: u64,
) -> Result<DeliveryVerificationPreflightReceipt, String> {
    validate_protocol_snapshot(protocol)?;
    validate_source(&source)?;
    validate_provider_receipt(&provider)?;
    validate_runner(runner)?;
    if created_at_ms == 0 {
        return Err("delivery verification preflight timestamp must be non-zero".into());
    }

    let mut receipt = DeliveryVerificationPreflightReceipt {
        schema: PREFLIGHT_SCHEMA.into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        protocol_id: protocol.protocol_id.clone(),
        suite_id: protocol.suite_id.clone(),
        created_at_ms,
        source_commit_sha256: sha256_hex(source.head.as_bytes()),
        source,
        manifest_sha256: protocol.manifest_sha256.clone(),
        suite_sha256: protocol.suite_sha256.clone(),
        case_sha256: protocol.case_sha256.clone(),
        cases_sha256: cases_digest(&protocol.case_sha256)?,
        case_order_sha256: protocol.case_order_sha256.clone(),
        seeded_candidates_sha256: protocol.seeded_candidates_sha256.clone(),
        model_inputs_sha256: protocol.model_inputs_sha256.clone(),
        output_contracts_sha256: protocol.output_contracts_sha256.clone(),
        budget_sha256: protocol.budget_sha256.clone(),
        hidden_oracle_sha256: protocol.hidden_oracle_sha256.clone(),
        provider,
        runner_binary: DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME.into(),
        runner_sha256: sha256_hex(&runner.bytes),
        runner_bytes: u64::try_from(runner.bytes.len()).unwrap_or(u64::MAX),
        runner_code_directory_sha256: runner.code_directory_sha256.clone(),
        output_root_sha256: path_sha256(output_root),
        provider_calls_performed: 0,
        online_runner_frozen: true,
        execution_authorized: false,
        online_execution_requires_new_frozen_runner: false,
        online_execution_requires_explicit_authorization: true,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt_digest(&receipt)?;
    validate_receipt(protocol, &receipt)?;
    Ok(receipt)
}

pub(super) fn validate_receipt(
    protocol: &DeliveryVerificationProtocolSnapshot,
    receipt: &DeliveryVerificationPreflightReceipt,
) -> Result<(), String> {
    validate_protocol_snapshot(protocol)?;
    validate_source(&receipt.source)?;
    validate_provider_receipt(&receipt.provider)?;
    if receipt.schema != PREFLIGHT_SCHEMA
        || receipt.app_version != env!("CARGO_PKG_VERSION")
        || receipt.protocol_id != protocol.protocol_id
        || receipt.suite_id != protocol.suite_id
        || receipt.created_at_ms == 0
        || receipt.source_commit_sha256 != sha256_hex(receipt.source.head.as_bytes())
        || receipt.manifest_sha256 != protocol.manifest_sha256
        || receipt.suite_sha256 != protocol.suite_sha256
        || receipt.case_sha256 != protocol.case_sha256
        || receipt.cases_sha256 != cases_digest(&protocol.case_sha256)?
        || receipt.case_order_sha256 != protocol.case_order_sha256
        || receipt.seeded_candidates_sha256 != protocol.seeded_candidates_sha256
        || receipt.model_inputs_sha256 != protocol.model_inputs_sha256
        || receipt.output_contracts_sha256 != protocol.output_contracts_sha256
        || receipt.budget_sha256 != protocol.budget_sha256
        || receipt.hidden_oracle_sha256 != protocol.hidden_oracle_sha256
        || receipt.runner_binary != DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME
        || !is_sha256(&receipt.runner_sha256)
        || receipt.runner_bytes == 0
        || receipt.runner_bytes > MAX_RUNNER_BYTES
        || !is_sha256(&receipt.runner_code_directory_sha256)
        || !is_sha256(&receipt.output_root_sha256)
        || receipt.provider_calls_performed != 0
        || !receipt.online_runner_frozen
        || receipt.execution_authorized
        || receipt.online_execution_requires_new_frozen_runner
        || !receipt.online_execution_requires_explicit_authorization
        || receipt.receipt_sha256 != receipt_digest(receipt)?
    {
        return Err("delivery verification preflight receipt is invalid".into());
    }
    Ok(())
}

#[allow(dead_code)]
pub(super) fn parse_and_validate_receipt(
    protocol: &DeliveryVerificationProtocolSnapshot,
    bytes: &[u8],
    source: &DeliveryVerificationSourceBindingReceipt,
    provider: &DeliveryVerificationProviderBindingReceipt,
    runner: &DeliveryVerificationRunnerBinary,
    output_root: &Path,
) -> Result<DeliveryVerificationPreflightReceipt, String> {
    let receipt: DeliveryVerificationPreflightReceipt = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid delivery verification preflight JSON: {error}"))?;
    if encode_receipt(&receipt)? != bytes {
        return Err("delivery verification preflight is not canonical JSON".into());
    }
    validate_snapshot(protocol, &receipt, source, provider, runner, output_root)?;
    Ok(receipt)
}

pub(super) fn validate_current_authority(
    protocol: &DeliveryVerificationProtocolSnapshot,
    receipt: &DeliveryVerificationPreflightReceipt,
    repo_root: &Path,
    config: &ProviderConfig,
    runner: &DeliveryVerificationRunnerBinary,
    output_root: &Path,
) -> Result<(), String> {
    let (head, tree) = clean_source_head_tree(repo_root)?;
    let output_root = validate_new_external_path(output_root, repo_root, "output root")?;
    validate_snapshot(
        protocol,
        receipt,
        &DeliveryVerificationSourceBindingReceipt { head, tree },
        &provider_binding(config)?,
        runner,
        &output_root,
    )
}

pub(super) fn validate_snapshot(
    protocol: &DeliveryVerificationProtocolSnapshot,
    receipt: &DeliveryVerificationPreflightReceipt,
    source: &DeliveryVerificationSourceBindingReceipt,
    provider: &DeliveryVerificationProviderBindingReceipt,
    runner: &DeliveryVerificationRunnerBinary,
    output_root: &Path,
) -> Result<(), String> {
    validate_receipt(protocol, receipt)?;
    validate_source(source)?;
    validate_provider_receipt(provider)?;
    validate_runner(runner)?;
    if &receipt.source != source
        || &receipt.provider != provider
        || receipt.runner_sha256 != sha256_hex(&runner.bytes)
        || receipt.runner_bytes != u64::try_from(runner.bytes.len()).unwrap_or(u64::MAX)
        || receipt.runner_code_directory_sha256 != runner.code_directory_sha256
        || receipt.output_root_sha256 != path_sha256(output_root)
    {
        return Err("delivery verification preflight no longer matches current authority".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Serialize)]
struct ProviderIdentityHashInput<'a> {
    provider_id: &'a str,
    provider_resource: &'a str,
    base_url: &'a str,
}

#[derive(Clone, Copy, Serialize)]
struct ModelBindingHashInput<'a> {
    owner_role: &'a str,
    owner_model: &'a str,
    verifier_role: &'a str,
    verifier_model: &'a str,
}

#[derive(Serialize)]
struct ProviderConfigHashInput<'a> {
    provider: ProviderIdentityHashInput<'a>,
    credential_present: bool,
    models: ModelBindingHashInput<'a>,
    context_window_tokens: u64,
}

fn validate_protocol_snapshot(
    protocol: &DeliveryVerificationProtocolSnapshot,
) -> Result<(), String> {
    if protocol.protocol_id.trim().is_empty()
        || protocol.suite_id.trim().is_empty()
        || !is_sha256(&protocol.manifest_sha256)
        || !is_sha256(&protocol.suite_sha256)
        || protocol.case_sha256.is_empty()
        || protocol.case_sha256.iter().any(|value| !is_sha256(value))
        || !is_sha256(&protocol.case_order_sha256)
        || !is_sha256(&protocol.seeded_candidates_sha256)
        || !is_sha256(&protocol.model_inputs_sha256)
        || !is_sha256(&protocol.output_contracts_sha256)
        || !is_sha256(&protocol.budget_sha256)
        || !is_sha256(&protocol.hidden_oracle_sha256)
        || protocol.execution_authorized
    {
        return Err("delivery verification protocol snapshot is invalid or authorizing".into());
    }
    Ok(())
}

fn validate_source(source: &DeliveryVerificationSourceBindingReceipt) -> Result<(), String> {
    validate_git_object_id(&source.head, "source HEAD")?;
    validate_git_object_id(&source.tree, "source tree")
}

fn validate_provider_receipt(
    provider: &DeliveryVerificationProviderBindingReceipt,
) -> Result<(), String> {
    if !provider.credential_present
        || provider.owner_role != "executor"
        || provider.verifier_role != "reviewer"
        || provider.owner_model_sha256 == provider.verifier_model_sha256
        || [
            &provider.provider_config_sha256,
            &provider.provider_identity_sha256,
            &provider.provider_id_sha256,
            &provider.provider_resource_sha256,
            &provider.base_url_sha256,
            &provider.owner_model_sha256,
            &provider.verifier_model_sha256,
            &provider.model_binding_sha256,
        ]
        .into_iter()
        .any(|value| !is_sha256(value))
    {
        return Err("delivery verification provider binding is invalid".into());
    }
    Ok(())
}

fn validate_runner(runner: &DeliveryVerificationRunnerBinary) -> Result<(), String> {
    if runner.bytes.is_empty()
        || u64::try_from(runner.bytes.len()).unwrap_or(u64::MAX) > MAX_RUNNER_BYTES
        || !is_sha256(&runner.code_directory_sha256)
    {
        return Err("delivery verification execute binary size is outside its bound".into());
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 2_048
        || value.chars().any(char::is_control)
    {
        return Err(format!("delivery verification {label} is invalid"));
    }
    Ok(())
}

fn cases_digest(case_sha256: &[String]) -> Result<String, String> {
    domain_hash_json(CASES_HASH_DOMAIN, case_sha256, "case binding")
}

fn receipt_digest(receipt: &DeliveryVerificationPreflightReceipt) -> Result<String, String> {
    let mut payload = receipt.clone();
    payload.receipt_sha256.clear();
    domain_hash_json(RECEIPT_HASH_DOMAIN, &payload, "preflight receipt")
}

fn domain_hash_json(
    domain: &[u8],
    value: &(impl Serialize + ?Sized),
    label: &str,
) -> Result<String, String> {
    let mut bytes = domain.to_vec();
    bytes.extend(
        serde_json::to_vec(value)
            .map_err(|error| format!("failed to hash delivery verification {label}: {error}"))?,
    );
    Ok(sha256_hex(&bytes))
}

fn path_sha256(path: &Path) -> String {
    sha256_hex(path.as_os_str().as_encoded_bytes())
}

fn validate_git_object_id(value: &str, label: &str) -> Result<(), String> {
    if !(40..=64).contains(&value.len())
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(format!("{label} is not a full lowercase Git object id"));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn required_new_external_path(
    variable: &str,
    repo_root: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{variable} is required"))?;
    validate_new_external_path(&path, repo_root, label)
}

fn encode_receipt(receipt: &DeliveryVerificationPreflightReceipt) -> Result<Vec<u8>, String> {
    let mut encoded = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("failed to encode delivery verification preflight: {error}"))?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn write_receipt(
    receipt_path: &Path,
    receipt: &DeliveryVerificationPreflightReceipt,
) -> Result<(), String> {
    write_new_private_file_atomically(receipt_path, &encode_receipt(receipt)?)
}

fn preflight_paths(repo_root: &Path) -> Result<(PathBuf, PathBuf), String> {
    let output_root = required_new_external_path(OUTPUT_ROOT_ENV, repo_root, "output root")?;
    let receipt_path = required_new_external_path(RECEIPT_ENV, repo_root, "preflight receipt")?;
    if receipt_path == output_root || receipt_path.starts_with(&output_root) {
        return Err("delivery verification receipt must be separate from output root".into());
    }
    Ok((output_root, receipt_path))
}

#[cfg(test)]
#[path = "delivery_verification_preflight_tests.rs"]
mod tests;
