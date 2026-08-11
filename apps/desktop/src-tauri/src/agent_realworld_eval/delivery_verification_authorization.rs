use super::delivery_verification_preflight::{
    DeliveryVerificationPreflightReceipt, DeliveryVerificationProtocolSnapshot,
    DeliveryVerificationProviderBindingReceipt,
};
use super::delivery_verification_protocol::{
    ProtocolBudget, ValidatedProtocol, DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
};
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub(super) const DELIVERY_AUTHORIZATION_SCHEMA: &str =
    "cindx.agent-eval.delivery-verification-authorization.v1";
pub(super) const DELIVERY_CONSUMED_SCHEMA: &str =
    "cindx.agent-eval.delivery-verification-authorization-consumed.v1";
pub(super) const DELIVERY_TOMBSTONE_FILE_NAME: &str =
    "delivery-verification-authorization-consumed.json";
pub(super) const DELIVERY_AUTHORIZATION_TTL_MS: u64 = 15 * 60 * 1_000;

const PREFLIGHT_SCHEMA: &str = "cindx.agent-eval.delivery-verification-preflight.v1";
const AUTHORIZATION_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-authorization.v1\0";
const TOMBSTONE_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-authorization-consumed.v1\0";
const PREFLIGHT_HASH_DOMAIN: &[u8] = b"cindx.agent-eval.delivery-verification-preflight.v1\0";
const CASES_HASH_DOMAIN: &[u8] = b"cindx.delivery-verification-cases.v1\0";
const CREDENTIAL_HASH_DOMAIN: &[u8] =
    b"cindx.agent-eval.delivery-verification-credential-fingerprint.v1\0";
const MAX_PRIVATE_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RUNNER_BYTES: u64 = 512 * 1024 * 1024;
const CASE_COUNT: usize = 32;

pub(super) struct DeliveryVerificationAuthorizationIssue<'a, 'protocol> {
    pub(super) protocol: &'a ValidatedProtocol<'protocol>,
    pub(super) preflight: &'a DeliveryVerificationPreflightReceipt,
    pub(super) repo_root: &'a Path,
    pub(super) preflight_path: &'a Path,
    pub(super) authorization_path: &'a Path,
    pub(super) output_root: &'a Path,
    pub(super) runner_bytes: &'a [u8],
    pub(super) issued_at_ms: u64,
    pub(super) expires_at_ms: u64,
    pub(super) nonce: &'a str,
    pub(super) credential: &'a str,
}

pub(super) struct DeliveryVerificationAuthorizationValidation<'a, 'protocol> {
    pub(super) protocol: &'a ValidatedProtocol<'protocol>,
    pub(super) preflight: &'a DeliveryVerificationPreflightReceipt,
    pub(super) repo_root: &'a Path,
    pub(super) preflight_path: &'a Path,
    pub(super) authorization_path: &'a Path,
    pub(super) output_root: &'a Path,
    pub(super) runner_bytes: &'a [u8],
    pub(super) credential: &'a str,
    pub(super) now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidatedDeliveryVerificationAuthorizationV1 {
    pub(super) authorization: DeliveryVerificationAuthorizationV1,
    pub(super) repo_root: PathBuf,
    pub(super) output_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationAuthorizationCaseV1 {
    pub(super) ordinal: usize,
    pub(super) id: String,
    pub(super) split: String,
    pub(super) stratum: String,
    pub(super) case_sha256: String,
    pub(super) model_input_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryVerificationAuthorizationV1 {
    pub(super) schema: String,
    pub(super) issued_at_ms: u64,
    pub(super) expires_at_ms: u64,
    pub(super) nonce: String,
    pub(super) preflight_path_sha256: String,
    pub(super) authorization_path_sha256: String,
    pub(super) preflight_receipt_sha256: String,
    pub(super) preflight_schema: String,
    pub(super) app_version: String,
    pub(super) source_head: String,
    pub(super) source_tree: String,
    pub(super) source_commit_sha256: String,
    pub(super) protocol_id: String,
    pub(super) suite_id: String,
    pub(super) manifest_sha256: String,
    pub(super) suite_sha256: String,
    pub(super) cases_sha256: String,
    pub(super) hidden_oracle_sha256: String,
    pub(super) provider: DeliveryVerificationProviderBindingReceipt,
    pub(super) budget: ProtocolBudget,
    pub(super) budget_sha256: String,
    pub(super) cases: Vec<DeliveryVerificationAuthorizationCaseV1>,
    pub(super) output_root_sha256: String,
    pub(super) runner_sha256: String,
    pub(super) runner_bytes: u64,
    pub(super) credential_fingerprint_sha256: String,
    pub(super) authorization_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConsumedDeliveryVerificationAuthorizationV1 {
    pub(super) schema: String,
    pub(super) authorization: DeliveryVerificationAuthorizationV1,
    pub(super) authorization_sha256: String,
    pub(super) authorization_path_sha256: String,
    pub(super) output_root_sha256: String,
    pub(super) runner_sha256: String,
    pub(super) consumed_at_ms: u64,
    pub(super) tombstone_sha256: String,
}

pub(super) fn issue_delivery_verification_authorization(
    input: DeliveryVerificationAuthorizationIssue<'_, '_>,
) -> Result<DeliveryVerificationAuthorizationV1, String> {
    let paths = validate_issue_paths(
        input.repo_root,
        input.preflight_path,
        input.authorization_path,
        input.output_root,
        false,
    )?;
    let preflight_bytes = read_private_file(&paths.preflight, "delivery preflight receipt")?;
    if encode_preflight(input.preflight)? != preflight_bytes {
        return Err("delivery preflight file does not match the supplied receipt".into());
    }
    build_authorization(
        input.protocol,
        input.preflight,
        &paths,
        input.runner_bytes,
        input.issued_at_ms,
        input.expires_at_ms,
        input.nonce,
        input.credential,
    )
}

pub(super) fn write_delivery_verification_authorization_new(
    repo_root: &Path,
    path: &Path,
    authorization: &DeliveryVerificationAuthorizationV1,
) -> Result<(), String> {
    authorization.validate_static()?;
    let path = canonical_external_path(path, repo_root, false, "delivery authorization")?;
    if path_sha256(&path) != authorization.authorization_path_sha256 {
        return Err("delivery authorization path differs from its binding".into());
    }
    write_new_private_file(
        &path,
        &canonical_json(authorization, "delivery authorization")?,
        "delivery authorization",
    )
}

pub(super) fn load_and_validate_delivery_verification_authorization(
    input: DeliveryVerificationAuthorizationValidation<'_, '_>,
) -> Result<ValidatedDeliveryVerificationAuthorizationV1, String> {
    let paths = validate_issue_paths(
        input.repo_root,
        input.preflight_path,
        input.authorization_path,
        input.output_root,
        true,
    )?;
    let preflight_bytes = read_private_file(&paths.preflight, "delivery preflight receipt")?;
    if encode_preflight(input.preflight)? != preflight_bytes {
        return Err("delivery preflight file does not match the supplied receipt".into());
    }
    let bytes = read_private_file(&paths.authorization, "delivery authorization")?;
    let authorization: DeliveryVerificationAuthorizationV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid delivery authorization JSON: {error}"))?;
    if canonical_json(&authorization, "delivery authorization")? != bytes {
        return Err("delivery authorization is not canonical JSON".into());
    }
    authorization.validate_static()?;
    if input.now_ms < authorization.issued_at_ms || input.now_ms >= authorization.expires_at_ms {
        return Err("delivery authorization is expired or not yet valid".into());
    }
    let expected = build_authorization(
        input.protocol,
        input.preflight,
        &paths,
        input.runner_bytes,
        authorization.issued_at_ms,
        authorization.expires_at_ms,
        &authorization.nonce,
        input.credential,
    )?;
    if authorization != expected {
        return Err("delivery authorization binding is stale or tampered".into());
    }
    Ok(ValidatedDeliveryVerificationAuthorizationV1 {
        authorization,
        repo_root: paths.repo_root,
        output_root: paths.output_root,
    })
}

pub(super) fn consume_delivery_verification_authorization_once(
    validated: &ValidatedDeliveryVerificationAuthorizationV1,
    consumed_at_ms: u64,
) -> Result<ConsumedDeliveryVerificationAuthorizationV1, String> {
    if consumed_at_ms < validated.authorization.issued_at_ms
        || consumed_at_ms >= validated.authorization.expires_at_ms
    {
        return Err("delivery authorization cannot be consumed outside its validity window".into());
    }
    if validated.output_root.exists() {
        return Err("delivery output root has already been consumed".into());
    }
    let mut tombstone = ConsumedDeliveryVerificationAuthorizationV1 {
        schema: DELIVERY_CONSUMED_SCHEMA.into(),
        authorization: validated.authorization.clone(),
        authorization_sha256: validated.authorization.authorization_sha256.clone(),
        authorization_path_sha256: validated.authorization.authorization_path_sha256.clone(),
        output_root_sha256: validated.authorization.output_root_sha256.clone(),
        runner_sha256: validated.authorization.runner_sha256.clone(),
        consumed_at_ms,
        tombstone_sha256: String::new(),
    };
    tombstone.tombstone_sha256 = tombstone_digest(&tombstone)?;

    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(&validated.output_root)
        .map_err(|error| format!("failed to consume delivery output root: {error}"))?;
    require_private_directory(&validated.output_root, "delivery output root")?;
    write_new_private_file(
        &validated.output_root.join(DELIVERY_TOMBSTONE_FILE_NAME),
        &canonical_json(&tombstone, "delivery consumed tombstone")?,
        "delivery consumed tombstone",
    )?;
    Ok(tombstone)
}

impl DeliveryVerificationAuthorizationV1 {
    pub(super) fn validate_static(&self) -> Result<(), String> {
        if self.schema != DELIVERY_AUTHORIZATION_SCHEMA
            || self.preflight_schema != PREFLIGHT_SCHEMA
            || self.app_version != env!("CARGO_PKG_VERSION")
            || self.cases.len() != CASE_COUNT
            || self.issued_at_ms == 0
            || self.issued_at_ms.checked_add(DELIVERY_AUTHORIZATION_TTL_MS)
                != Some(self.expires_at_ms)
            || self.runner_bytes == 0
            || self.runner_bytes > MAX_RUNNER_BYTES
            || self.provider.owner_role != "executor"
            || self.provider.verifier_role != "reviewer"
            || self.provider.owner_model_sha256 == self.provider.verifier_model_sha256
            || !self.provider.credential_present
            || self.budget.transport_retries != 0
            || self.budget.max_logical_model_calls_total != 128
            || self.budget.max_physical_model_attempts_total != 128
        {
            return Err("delivery authorization shape is invalid".into());
        }
        validate_git_id(&self.source_head, "delivery source HEAD")?;
        validate_git_id(&self.source_tree, "delivery source tree")?;
        require_sha256(&self.nonce, "delivery authorization nonce")?;
        for (label, value) in [
            ("preflight path", &self.preflight_path_sha256),
            ("authorization path", &self.authorization_path_sha256),
            ("preflight receipt", &self.preflight_receipt_sha256),
            ("source commit", &self.source_commit_sha256),
            ("manifest", &self.manifest_sha256),
            ("suite", &self.suite_sha256),
            ("cases", &self.cases_sha256),
            ("hidden oracle", &self.hidden_oracle_sha256),
            ("budget", &self.budget_sha256),
            ("output root", &self.output_root_sha256),
            ("runner", &self.runner_sha256),
            (
                "credential fingerprint",
                &self.credential_fingerprint_sha256,
            ),
            ("authorization", &self.authorization_sha256),
        ] {
            require_sha256(value, label)?;
        }
        validate_provider_binding(&self.provider)?;
        for (index, case) in self.cases.iter().enumerate() {
            if case.ordinal != index + 1 || case.id.trim().is_empty() {
                return Err("delivery authorization case order is invalid".into());
            }
            require_sha256(&case.case_sha256, "delivery case")?;
            require_sha256(&case.model_input_sha256, "delivery model input")?;
            if !matches!(case.split.as_str(), "calibration" | "holdout")
                || !matches!(
                    case.stratum.as_str(),
                    "unsupported_claim" | "omitted_obligation" | "contradiction" | "preservation"
                )
            {
                return Err("delivery authorization case classification is invalid".into());
            }
        }
        if self.authorization_sha256 != authorization_digest(self)? {
            return Err("delivery authorization digest is invalid".into());
        }
        Ok(())
    }
}

impl ConsumedDeliveryVerificationAuthorizationV1 {
    pub(super) fn validate_for(
        &self,
        authorization: &DeliveryVerificationAuthorizationV1,
    ) -> Result<(), String> {
        if self.schema != DELIVERY_CONSUMED_SCHEMA
            || &self.authorization != authorization
            || self.authorization_sha256 != authorization.authorization_sha256
            || self.authorization_path_sha256 != authorization.authorization_path_sha256
            || self.output_root_sha256 != authorization.output_root_sha256
            || self.runner_sha256 != authorization.runner_sha256
            || self.consumed_at_ms < authorization.issued_at_ms
            || self.consumed_at_ms >= authorization.expires_at_ms
            || self.tombstone_sha256 != tombstone_digest(self)?
        {
            return Err("delivery consumed authorization tombstone is invalid".into());
        }
        Ok(())
    }
}

struct ControlPaths {
    repo_root: PathBuf,
    preflight: PathBuf,
    authorization: PathBuf,
    output_root: PathBuf,
}

#[allow(clippy::too_many_arguments)]
fn build_authorization(
    protocol: &ValidatedProtocol<'_>,
    preflight: &DeliveryVerificationPreflightReceipt,
    paths: &ControlPaths,
    runner_bytes: &[u8],
    issued_at_ms: u64,
    expires_at_ms: u64,
    nonce: &str,
    credential: &str,
) -> Result<DeliveryVerificationAuthorizationV1, String> {
    validate_frozen_preflight(protocol, preflight, &paths.output_root)?;
    if issued_at_ms < preflight.created_at_ms
        || issued_at_ms == 0
        || issued_at_ms.checked_add(DELIVERY_AUTHORIZATION_TTL_MS) != Some(expires_at_ms)
    {
        return Err("delivery authorization validity window is invalid".into());
    }
    require_sha256(nonce, "delivery authorization nonce")?;
    if credential.trim().is_empty() {
        return Err("delivery authorization requires the exact provider credential".into());
    }
    if runner_bytes.is_empty()
        || u64::try_from(runner_bytes.len()).unwrap_or(u64::MAX) > MAX_RUNNER_BYTES
    {
        return Err("delivery execute binary size is outside its bound".into());
    }
    let cases = protocol
        .cases()
        .map(|case| {
            Ok(DeliveryVerificationAuthorizationCaseV1 {
                ordinal: case.ordinal(),
                id: case.id().to_string(),
                split: format!("{:?}", case.split()).to_ascii_lowercase(),
                stratum: camel_debug_to_snake(&format!("{:?}", case.stratum())),
                case_sha256: case.case_sha256().to_string(),
                model_input_sha256: sha256_hex(&case.model_input_bytes()?),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut authorization = DeliveryVerificationAuthorizationV1 {
        schema: DELIVERY_AUTHORIZATION_SCHEMA.into(),
        issued_at_ms,
        expires_at_ms,
        nonce: nonce.into(),
        preflight_path_sha256: path_sha256(&paths.preflight),
        authorization_path_sha256: path_sha256(&paths.authorization),
        preflight_receipt_sha256: preflight.receipt_sha256.clone(),
        preflight_schema: preflight.schema.clone(),
        app_version: preflight.app_version.clone(),
        source_head: preflight.source.head.clone(),
        source_tree: preflight.source.tree.clone(),
        source_commit_sha256: preflight.source_commit_sha256.clone(),
        protocol_id: preflight.protocol_id.clone(),
        suite_id: preflight.suite_id.clone(),
        manifest_sha256: preflight.manifest_sha256.clone(),
        suite_sha256: preflight.suite_sha256.clone(),
        cases_sha256: preflight.cases_sha256.clone(),
        hidden_oracle_sha256: preflight.hidden_oracle_sha256.clone(),
        provider: preflight.provider.clone(),
        budget: protocol.budget().clone(),
        budget_sha256: preflight.budget_sha256.clone(),
        cases,
        output_root_sha256: path_sha256(&paths.output_root),
        runner_sha256: sha256_hex(runner_bytes),
        runner_bytes: u64::try_from(runner_bytes.len()).unwrap_or(u64::MAX),
        credential_fingerprint_sha256: credential_fingerprint(nonce, credential),
        authorization_sha256: String::new(),
    };
    authorization.authorization_sha256 = authorization_digest(&authorization)?;
    authorization.validate_static()?;
    Ok(authorization)
}

fn validate_frozen_preflight(
    protocol: &ValidatedProtocol<'_>,
    preflight: &DeliveryVerificationPreflightReceipt,
    output_root: &Path,
) -> Result<(), String> {
    let snapshot = DeliveryVerificationProtocolSnapshot {
        protocol_id: protocol.protocol_id().to_string(),
        suite_id: protocol.suite_id().to_string(),
        manifest_sha256: protocol.manifest_sha256().to_string(),
        suite_sha256: protocol.suite_sha256().to_string(),
        case_sha256: protocol.case_sha256s().to_vec(),
        budget_sha256: protocol.budget_sha256().to_string(),
        hidden_oracle_sha256: protocol.hidden_oracle_sha256().to_string(),
        execution_authorized: protocol.execution_authorized(),
    };
    validate_provider_binding(&preflight.provider)?;
    if preflight.schema != PREFLIGHT_SCHEMA
        || preflight.app_version != env!("CARGO_PKG_VERSION")
        || preflight.protocol_id != snapshot.protocol_id
        || preflight.suite_id != snapshot.suite_id
        || preflight.source_commit_sha256 != sha256_hex(preflight.source.head.as_bytes())
        || preflight.manifest_sha256 != snapshot.manifest_sha256
        || preflight.suite_sha256 != snapshot.suite_sha256
        || preflight.case_sha256 != snapshot.case_sha256
        || preflight.cases_sha256 != cases_digest(&snapshot.case_sha256)?
        || preflight.budget_sha256 != snapshot.budget_sha256
        || preflight.hidden_oracle_sha256 != snapshot.hidden_oracle_sha256
        || preflight.output_root_sha256 != path_sha256(output_root)
        || preflight.provider_calls_performed != 0
        || !preflight.online_runner_frozen
        || preflight.execution_authorized
        || preflight.online_execution_requires_new_frozen_runner
        || !preflight.online_execution_requires_explicit_authorization
        || preflight.receipt_sha256 != frozen_preflight_digest(preflight)?
        || snapshot.execution_authorized
        || protocol.execute_binary_name() != DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME
        || protocol.cases().len() != CASE_COUNT
    {
        return Err("delivery preflight is not a frozen non-authorizing runner authority".into());
    }
    validate_git_id(&preflight.source.head, "delivery source HEAD")?;
    validate_git_id(&preflight.source.tree, "delivery source tree")
}

fn validate_provider_binding(
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
        return Err("delivery authorization provider binding is invalid".into());
    }
    Ok(())
}

fn validate_issue_paths(
    repo_root: &Path,
    preflight_path: &Path,
    authorization_path: &Path,
    output_root: &Path,
    authorization_must_exist: bool,
) -> Result<ControlPaths, String> {
    let repo_root = repo_root
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize delivery repository root: {error}"))?;
    let preflight = canonical_external_path(
        preflight_path,
        &repo_root,
        true,
        "delivery preflight receipt",
    )?;
    let authorization = canonical_external_path(
        authorization_path,
        &repo_root,
        authorization_must_exist,
        "delivery authorization",
    )?;
    if !authorization_must_exist && authorization.exists() {
        return Err("delivery authorization already exists".into());
    }
    let output_root =
        canonical_external_path(output_root, &repo_root, false, "delivery output root")?;
    if output_root.exists() {
        return Err("delivery output root must be new".into());
    }
    if preflight == authorization
        || preflight == output_root
        || authorization == output_root
        || preflight.starts_with(&output_root)
        || authorization.starts_with(&output_root)
    {
        return Err("delivery control-plane paths must be separate".into());
    }
    Ok(ControlPaths {
        repo_root,
        preflight,
        authorization,
        output_root,
    })
}

pub(super) fn read_delivery_execute_sibling_bytes() -> Result<Vec<u8>, String> {
    let authorize = std::env::current_exe()
        .map_err(|error| format!("failed to locate delivery authorize binary: {error}"))?;
    let directory = authorize
        .parent()
        .ok_or_else(|| "delivery authorize binary has no parent directory".to_string())?;
    read_exact_binary(
        &directory.join(format!(
            "{}{}",
            DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
            std::env::consts::EXE_SUFFIX
        )),
        DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
    )
}

pub(super) fn read_current_delivery_execute_bytes() -> Result<Vec<u8>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to locate delivery execute binary: {error}"))?;
    read_exact_binary(&executable, DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME)
}

fn read_exact_binary(path: &Path, expected_stem: &str) -> Result<Vec<u8>, String> {
    let expected_name = format!("{}{}", expected_stem, std::env::consts::EXE_SUFFIX);
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err("delivery execute binary has an unexpected fixed name".into());
    }
    let mut file = open_regular_file(path, false, "delivery execute binary")?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to stat delivery execute binary: {error}"))?;
    if metadata.len() == 0 || metadata.len() > MAX_RUNNER_BYTES {
        return Err("delivery execute binary size is outside its bound".into());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read delivery execute binary: {error}"))?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err("delivery execute binary changed while it was read".into());
    }
    Ok(bytes)
}

pub(super) fn read_consumed_delivery_authorization(
    root: &Path,
) -> Result<ConsumedDeliveryVerificationAuthorizationV1, String> {
    let bytes = read_private_file(
        &root.join(DELIVERY_TOMBSTONE_FILE_NAME),
        "delivery consumed tombstone",
    )?;
    let tombstone: ConsumedDeliveryVerificationAuthorizationV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid delivery consumed tombstone JSON: {error}"))?;
    if canonical_json(&tombstone, "delivery consumed tombstone")? != bytes {
        return Err("delivery consumed tombstone is not canonical JSON".into());
    }
    tombstone.validate_for(&tombstone.authorization.clone())?;
    Ok(tombstone)
}

pub(super) fn canonical_external_path(
    path: &Path,
    repo_root: &Path,
    must_exist: bool,
    label: &str,
) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(format!("{label} must be a normalized absolute path"));
    }
    let resolved = if path.exists() {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("failed to inspect {label}: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{label} must not be a symlink"));
        }
        path.canonicalize()
            .map_err(|error| format!("failed to canonicalize {label}: {error}"))?
    } else {
        if must_exist {
            return Err(format!("{label} does not exist"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| format!("{label} has no parent"))?
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize {label} parent: {error}"))?;
        parent.join(
            path.file_name()
                .ok_or_else(|| format!("{label} has no file name"))?,
        )
    };
    if resolved.starts_with(repo_root) {
        return Err(format!("{label} must remain outside the repository"));
    }
    Ok(resolved)
}

pub(super) fn require_private_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{label} is not a private regular directory"));
    }
    #[cfg(unix)]
    if metadata.mode() & 0o777 != 0o700 {
        return Err(format!("{label} permissions must be 0700"));
    }
    Ok(())
}

pub(super) fn read_private_file(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    let mut file = open_regular_file(path, true, label)?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to stat {label}: {error}"))?;
    if metadata.len() > MAX_PRIVATE_FILE_BYTES {
        return Err(format!("{label} exceeds its private file bound"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {label}: {error}"))?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err(format!("{label} changed while it was read"));
    }
    Ok(bytes)
}

fn open_regular_file(path: &Path, private: bool, label: &str) -> Result<File, String> {
    let before = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(format!("{label} must be a regular non-symlink file"));
    }
    #[cfg(unix)]
    if private && before.mode() & 0o777 != 0o600 {
        return Err(format!("{label} permissions must be 0600"));
    }
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| format!("failed to open {label}: {error}"))?;
    let after = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to re-inspect {label}: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("failed to inspect opened {label}: {error}"))?;
    if !after.is_file() || after.file_type().is_symlink() {
        return Err(format!("{label} changed to a non-regular file"));
    }
    #[cfg(unix)]
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || after.dev() != opened.dev()
        || after.ino() != opened.ino()
    {
        return Err(format!("{label} changed while it was opened"));
    }
    Ok(file)
}

pub(super) fn write_new_private_file(path: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} path has no parent"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("delivery");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| format!("failed to stage {label}: {error}"))?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure {label}: {error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("failed to write {label}: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("failed to sync {label}: {error}"))?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| format!("failed to publish new {label}: {}", error.error))?;
    sync_parent(parent);
    Ok(())
}

pub(super) fn replace_private_file(path: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    read_private_metadata(path, label)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} path has no parent"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("delivery");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| format!("failed to stage {label}: {error}"))?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure {label}: {error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("failed to write {label}: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("failed to sync {label}: {error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("failed to replace {label}: {}", error.error))?;
    read_private_metadata(path, label)?;
    sync_parent(parent);
    Ok(())
}

fn read_private_metadata(path: &Path, label: &str) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{label} must be a regular non-symlink file"));
    }
    #[cfg(unix)]
    if metadata.mode() & 0o777 != 0o600 {
        return Err(format!("{label} permissions must be 0600"));
    }
    Ok(metadata)
}

fn sync_parent(parent: &Path) {
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
}

pub(super) fn canonical_json<T: Serialize>(value: &T, label: &str) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| format!("failed to encode {label}: {error}"))
}

fn encode_preflight(value: &DeliveryVerificationPreflightReceipt) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to encode delivery preflight: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn frozen_preflight_digest(
    value: &DeliveryVerificationPreflightReceipt,
) -> Result<String, String> {
    let mut payload = value.clone();
    payload.receipt_sha256.clear();
    domain_digest(PREFLIGHT_HASH_DOMAIN, &payload, "delivery preflight")
}

fn authorization_digest(value: &DeliveryVerificationAuthorizationV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.authorization_sha256.clear();
    domain_digest(
        AUTHORIZATION_HASH_DOMAIN,
        &payload,
        "delivery authorization",
    )
}

fn tombstone_digest(value: &ConsumedDeliveryVerificationAuthorizationV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.tombstone_sha256.clear();
    domain_digest(
        TOMBSTONE_HASH_DOMAIN,
        &payload,
        "delivery consumed tombstone",
    )
}

pub(super) fn domain_digest<T: Serialize>(
    domain: &[u8],
    value: &T,
    label: &str,
) -> Result<String, String> {
    let mut bytes = domain.to_vec();
    bytes.extend(
        serde_json::to_vec(value).map_err(|error| format!("failed to hash {label}: {error}"))?,
    );
    Ok(sha256_hex(&bytes))
}

fn cases_digest(cases: &[String]) -> Result<String, String> {
    domain_digest(CASES_HASH_DOMAIN, &cases, "delivery cases")
}

fn credential_fingerprint(nonce: &str, credential: &str) -> String {
    let mut bytes = CREDENTIAL_HASH_DOMAIN.to_vec();
    bytes.extend_from_slice(nonce.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(credential.as_bytes());
    sha256_hex(&bytes)
}

pub(super) fn path_sha256(path: &Path) -> String {
    sha256_hex(path.as_os_str().as_encoded_bytes())
}

pub(super) fn require_sha256(value: &str, label: &str) -> Result<(), String> {
    if !is_sha256(value) {
        return Err(format!("{label} must be a lowercase SHA-256 digest"));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_git_id(value: &str, label: &str) -> Result<(), String> {
    if !(40..=64).contains(&value.len())
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(format!("{label} is not a full lowercase Git object id"));
    }
    Ok(())
}

fn camel_debug_to_snake(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 4);
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 {
            output.push('_');
        }
        output.push(character.to_ascii_lowercase());
    }
    output
}
