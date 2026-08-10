use super::execution_journal::ArmKindV1;
use super::preflight::{ProviderBindingReceipt, SuccessorPreflightReceipt};
use super::{FrozenCampaignBudget, FrozenRunBudget, ValidatedProtocol};
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const PREFLIGHT_SCHEMA: &str = "cindx.collaboration-successor-preflight.v1";
const PREFLIGHT_HASH_DOMAIN: &[u8] = b"cindx.collaboration-successor-preflight.v1\0";
const AUTHORIZATION_SCHEMA: &str = "cindx.collaboration-successor-authorization.v1";
const AUTHORIZATION_HASH_DOMAIN: &[u8] = b"cindx.collaboration-successor-authorization.v1\0";
const CREDENTIAL_HASH_DOMAIN: &[u8] = b"cindx.collaboration-successor-credential-fingerprint.v1\0";
const TOMBSTONE_SCHEMA: &str = "cindx.collaboration-successor-consumed.v1";
const TOMBSTONE_HASH_DOMAIN: &[u8] = b"cindx.collaboration-successor-consumed.v1\0";
pub(super) const JOURNAL_SCHEMA: &str = "cindx.collaboration-successor-execution-journal.v1";
pub(super) const JOURNAL_HASH_DOMAIN: &[u8] =
    b"cindx.collaboration-successor-execution-journal.v1\0";
const TOMBSTONE_FILE_NAME: &str = "successor-authorization-consumed.json";
pub(super) const JOURNAL_FILE_NAME: &str = "successor-execution-journal.json";
const RECOVERY_FILE_NAME: &str = "successor-execution-recovery.json";
const MAX_PRIVATE_FILE_BYTES: u64 = 2 * 1024 * 1024;
const AUTHORIZATION_TTL_MS: u64 = 15 * 60 * 1_000;
pub(super) const PAIR_COUNT: usize = 3;
const RUN_COUNT: usize = 6;

pub(super) struct AuthorizationIssue<'a, 'protocol> {
    pub(super) protocol: &'a ValidatedProtocol<'protocol>,
    pub(super) preflight: &'a SuccessorPreflightReceipt,
    pub(super) authorization_path: &'a Path,
    pub(super) output_root: &'a Path,
    pub(super) runner_bytes: &'a [u8],
    pub(super) issued_at_ms: u64,
    pub(super) expires_at_ms: u64,
    pub(super) nonce: &'a str,
    pub(super) credential: &'a str,
}

pub(super) struct AuthorizationValidation<'a, 'protocol> {
    pub(super) protocol: &'a ValidatedProtocol<'protocol>,
    pub(super) preflight: &'a SuccessorPreflightReceipt,
    pub(super) authorization_path: &'a Path,
    pub(super) output_root: &'a Path,
    pub(super) runner_bytes: &'a [u8],
    pub(super) credential: &'a str,
    pub(super) now_ms: u64,
}

#[derive(Clone)]
pub(super) struct ValidatedAuthorizationV1 {
    pub(super) authorization: AuthorizationV1,
    pub(super) output_root: PathBuf,
}

impl ValidatedAuthorizationV1 {
    pub(super) fn authorization(&self) -> &AuthorizationV1 {
        &self.authorization
    }
}

impl AuthorizationV1 {
    pub(super) fn pair_count(&self) -> usize {
        self.pair_count
    }

    pub(super) fn run_count(&self) -> usize {
        self.run_count
    }

    pub(super) fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub(super) fn runner_sha256(&self) -> &str {
        &self.runner_sha256
    }

    pub(super) fn validity_window_ms(&self) -> Option<u64> {
        self.expires_at_ms.checked_sub(self.issued_at_ms)
    }

    pub(super) fn authorization_sha256(&self) -> &str {
        &self.authorization_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationV1 {
    schema: String,
    issued_at_ms: u64,
    expires_at_ms: u64,
    nonce: String,
    authorization_path_sha256: String,
    preflight_receipt_sha256: String,
    preflight_schema: String,
    app_version: String,
    source_head: String,
    source_tree: String,
    source_commit_sha256: String,
    manifest_sha256: String,
    suite_sha256: String,
    cohort_sha256: String,
    provider: AuthorizationProviderV1,
    pub(super) run_budget: AuthorizationRunBudgetV1,
    run_budget_sha256: String,
    outcome_budget_sha256: String,
    pub(super) campaign_budget: AuthorizationCampaignBudgetV1,
    campaign_budget_sha256: String,
    pair_count: usize,
    run_count: usize,
    pub(super) cells: Vec<AuthorizationCellV1>,
    pub(super) output_root_sha256: String,
    runner_sha256: String,
    credential_fingerprint_sha256: String,
    authorization_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationProviderV1 {
    credential_present: bool,
    provider_config_sha256: String,
    provider_identity_sha256: String,
    capture_model_pool_sha256: String,
    role_model_sha256: BTreeMap<String, String>,
    ordered_model_pool_sha256: String,
    ordered_model_member_sha256: Vec<String>,
}

impl From<&ProviderBindingReceipt> for AuthorizationProviderV1 {
    fn from(value: &ProviderBindingReceipt) -> Self {
        Self {
            credential_present: value.credential_present,
            provider_config_sha256: value.provider_config_sha256.clone(),
            provider_identity_sha256: value.provider_identity_sha256.clone(),
            capture_model_pool_sha256: value.capture_model_pool_sha256.clone(),
            role_model_sha256: value.role_model_sha256.clone(),
            ordered_model_pool_sha256: value.ordered_model_pool_sha256.clone(),
            ordered_model_member_sha256: value.ordered_model_member_sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationRunBudgetV1 {
    pub(super) max_duration_ms: u64,
    model_call_timeout_ms: u64,
    tool_call_timeout_ms: u64,
    initial_model_calls: usize,
    pub(super) max_model_calls: usize,
    model_calls_per_extension: usize,
    initial_tool_calls: usize,
    pub(super) max_tool_calls: usize,
    tool_calls_per_extension: usize,
    no_progress_timeout_ms: u64,
    max_identical_actions: usize,
    initial_agent_turns: usize,
    pub(super) max_agent_turns: usize,
    agent_turns_per_extension: usize,
    max_repair_attempts: usize,
    terminal_model_call_reserve: usize,
    terminal_time_reserve_ms: u64,
    pub(super) max_total_tokens: u64,
    pub(super) max_physical_model_attempts: usize,
    terminal_token_reserve: u64,
    terminal_physical_model_attempt_reserve: usize,
}

impl From<&FrozenRunBudget> for AuthorizationRunBudgetV1 {
    fn from(value: &FrozenRunBudget) -> Self {
        Self {
            max_duration_ms: value.max_duration_ms,
            model_call_timeout_ms: value.model_call_timeout_ms,
            tool_call_timeout_ms: value.tool_call_timeout_ms,
            initial_model_calls: value.initial_model_calls,
            max_model_calls: value.max_model_calls,
            model_calls_per_extension: value.model_calls_per_extension,
            initial_tool_calls: value.initial_tool_calls,
            max_tool_calls: value.max_tool_calls,
            tool_calls_per_extension: value.tool_calls_per_extension,
            no_progress_timeout_ms: value.no_progress_timeout_ms,
            max_identical_actions: value.max_identical_actions,
            initial_agent_turns: value.initial_agent_turns,
            max_agent_turns: value.max_agent_turns,
            agent_turns_per_extension: value.agent_turns_per_extension,
            max_repair_attempts: value.max_repair_attempts,
            terminal_model_call_reserve: value.terminal_model_call_reserve,
            terminal_time_reserve_ms: value.terminal_time_reserve_ms,
            max_total_tokens: value.max_total_tokens,
            max_physical_model_attempts: value.max_physical_model_attempts,
            terminal_token_reserve: value.terminal_token_reserve,
            terminal_physical_model_attempt_reserve: value.terminal_physical_model_attempt_reserve,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationCampaignBudgetV1 {
    pub(super) runs: usize,
    pub(super) max_duration_ms: u64,
    pub(super) max_model_calls: usize,
    pub(super) max_tool_calls: usize,
    pub(super) max_agent_turns: usize,
    pub(super) max_physical_model_attempts: usize,
    pub(super) max_total_tokens: u64,
}

impl From<&FrozenCampaignBudget> for AuthorizationCampaignBudgetV1 {
    fn from(value: &FrozenCampaignBudget) -> Self {
        Self {
            runs: value.runs,
            max_duration_ms: value.max_duration_ms,
            max_model_calls: value.max_model_calls,
            max_tool_calls: value.max_tool_calls,
            max_agent_turns: value.max_agent_turns,
            max_physical_model_attempts: value.max_physical_model_attempts,
            max_total_tokens: value.max_total_tokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationCellV1 {
    pub(super) ordinal: usize,
    case_id: String,
    split: String,
    purpose: String,
    replicate: u32,
    arm_order: String,
    workflow_policy: String,
    case_input_sha256: String,
    case_contract_sha256: String,
    workspace_prestate_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConsumedTombstoneV1 {
    schema: String,
    authorization: AuthorizationV1,
    authorization_sha256: String,
    authorization_path_sha256: String,
    output_root_sha256: String,
    runner_sha256: String,
    consumed_at_ms: u64,
    pub(super) tombstone_sha256: String,
}

pub(super) fn issue_authorization(
    input: AuthorizationIssue<'_, '_>,
) -> Result<AuthorizationV1, String> {
    validate_preflight(input.protocol, input.preflight, input.output_root)?;
    if input.issued_at_ms < input.preflight.created_at_ms
        || input.issued_at_ms.checked_add(AUTHORIZATION_TTL_MS) != Some(input.expires_at_ms)
    {
        return Err("successor authorization validity window is invalid".into());
    }
    if input.runner_bytes.is_empty() {
        return Err("successor authorization requires exact runner bytes".into());
    }
    require_nonce(input.nonce)?;
    if input.credential.is_empty() {
        return Err("successor authorization requires the exact provider credential".into());
    }
    let authorization_path = canonical_bound_path(input.authorization_path, false)?;
    let output_root = canonical_bound_path(input.output_root, false)?;
    if output_root.exists() {
        return Err("successor output root must be new".into());
    }
    let cells = input
        .protocol
        .manifest
        .matrix
        .cells
        .iter()
        .zip(&input.preflight.cells)
        .map(|(cell, materialized)| AuthorizationCellV1 {
            ordinal: cell.ordinal,
            case_id: cell.case_id.clone(),
            split: cell.split.clone(),
            purpose: cell.purpose.clone(),
            replicate: cell.replicate,
            arm_order: cell.arm_order.clone(),
            workflow_policy: cell.workflow_policy.clone(),
            case_input_sha256: cell.case_input_sha256.clone(),
            case_contract_sha256: cell.case_contract_sha256.clone(),
            workspace_prestate_sha256: materialized.workspace_prestate_sha256.clone(),
        })
        .collect();
    let mut authorization = AuthorizationV1 {
        schema: AUTHORIZATION_SCHEMA.into(),
        issued_at_ms: input.issued_at_ms,
        expires_at_ms: input.expires_at_ms,
        nonce: input.nonce.into(),
        authorization_path_sha256: path_sha256(&authorization_path),
        preflight_receipt_sha256: input.preflight.receipt_sha256.clone(),
        preflight_schema: input.preflight.schema.clone(),
        app_version: input.preflight.app_version.clone(),
        source_head: input.preflight.source.head.clone(),
        source_tree: input.preflight.source.tree.clone(),
        source_commit_sha256: input.preflight.source_commit_sha256.clone(),
        manifest_sha256: input.preflight.manifest_sha256.clone(),
        suite_sha256: input.preflight.suite_sha256.clone(),
        cohort_sha256: input.preflight.cohort_sha256.clone(),
        provider: AuthorizationProviderV1::from(&input.preflight.provider),
        run_budget: AuthorizationRunBudgetV1::from(&input.protocol.manifest.run_budget),
        run_budget_sha256: input.preflight.run_budget_sha256.clone(),
        outcome_budget_sha256: input.preflight.outcome_budget_sha256.clone(),
        campaign_budget: AuthorizationCampaignBudgetV1::from(
            &input.protocol.manifest.campaign_budget,
        ),
        campaign_budget_sha256: input.preflight.campaign_budget_sha256.clone(),
        pair_count: input.protocol.manifest.matrix.pair_count,
        run_count: input.protocol.manifest.matrix.run_count,
        cells,
        output_root_sha256: path_sha256(&output_root),
        runner_sha256: sha256_hex(input.runner_bytes),
        credential_fingerprint_sha256: credential_fingerprint(input.nonce, input.credential),
        authorization_sha256: String::new(),
    };
    authorization.authorization_sha256 = authorization_digest(&authorization)?;
    authorization.validate_static()?;
    Ok(authorization)
}

pub(super) fn write_authorization_new(
    path: &Path,
    authorization: &AuthorizationV1,
) -> Result<(), String> {
    authorization.validate_static()?;
    if path_sha256(&canonical_bound_path(path, false)?) != authorization.authorization_path_sha256 {
        return Err("successor authorization path does not match its binding".into());
    }
    super::preflight::write_new_private_file_atomically(
        path,
        &canonical_json(authorization, "successor authorization")?,
    )
}

pub(super) fn load_and_validate_authorization(
    input: AuthorizationValidation<'_, '_>,
) -> Result<ValidatedAuthorizationV1, String> {
    let authorization_path = canonical_bound_path(input.authorization_path, true)?;
    let bytes = read_private_file(&authorization_path, "successor authorization")?;
    let authorization: AuthorizationV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid successor authorization JSON: {error}"))?;
    if canonical_json(&authorization, "successor authorization")? != bytes {
        return Err("successor authorization is not canonical JSON".into());
    }
    authorization.validate_static()?;
    if input.now_ms < authorization.issued_at_ms || input.now_ms >= authorization.expires_at_ms {
        return Err("successor authorization is expired or not yet valid".into());
    }
    let expected = issue_authorization(AuthorizationIssue {
        protocol: input.protocol,
        preflight: input.preflight,
        authorization_path: &authorization_path,
        output_root: input.output_root,
        runner_bytes: input.runner_bytes,
        issued_at_ms: authorization.issued_at_ms,
        expires_at_ms: authorization.expires_at_ms,
        nonce: &authorization.nonce,
        credential: input.credential,
    })?;
    if authorization != expected {
        return Err("successor authorization binding is stale or tampered".into());
    }
    Ok(ValidatedAuthorizationV1 {
        authorization,
        output_root: canonical_bound_path(input.output_root, false)?,
    })
}

pub(super) fn consume_authorization_once(
    validated: &ValidatedAuthorizationV1,
    consumed_at_ms: u64,
) -> Result<ConsumedTombstoneV1, String> {
    if consumed_at_ms < validated.authorization.issued_at_ms
        || consumed_at_ms >= validated.authorization.expires_at_ms
    {
        return Err(
            "successor authorization cannot be consumed outside its validity window".into(),
        );
    }
    let mut tombstone = ConsumedTombstoneV1 {
        schema: TOMBSTONE_SCHEMA.into(),
        authorization: validated.authorization.clone(),
        authorization_sha256: validated.authorization.authorization_sha256.clone(),
        authorization_path_sha256: validated.authorization.authorization_path_sha256.clone(),
        output_root_sha256: validated.authorization.output_root_sha256.clone(),
        runner_sha256: validated.authorization.runner_sha256.clone(),
        consumed_at_ms,
        tombstone_sha256: String::new(),
    };
    tombstone.tombstone_sha256 = tombstone_digest(&tombstone)?;
    fs::create_dir(&validated.output_root)
        .map_err(|error| format!("failed to consume successor output root: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(&validated.output_root, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("failed to secure consumed successor output root: {error}"))?;
    super::preflight::write_new_private_file_atomically(
        &validated.output_root.join(TOMBSTONE_FILE_NAME),
        &canonical_json(&tombstone, "successor consumed tombstone")?,
    )?;
    Ok(tombstone)
}

impl AuthorizationV1 {
    pub(super) fn validate_static(&self) -> Result<(), String> {
        if self.schema != AUTHORIZATION_SCHEMA
            || self.preflight_schema != PREFLIGHT_SCHEMA
            || self.pair_count != PAIR_COUNT
            || self.run_count != RUN_COUNT
            || self.cells.len() != PAIR_COUNT
            || self.campaign_budget.runs != RUN_COUNT
            || !self.provider.credential_present
            || self.issued_at_ms.checked_add(AUTHORIZATION_TTL_MS) != Some(self.expires_at_ms)
        {
            return Err("successor authorization shape is invalid".into());
        }
        require_nonce(&self.nonce)?;
        for (label, value) in [
            ("authorization path", &self.authorization_path_sha256),
            ("preflight receipt", &self.preflight_receipt_sha256),
            ("source commit", &self.source_commit_sha256),
            ("manifest", &self.manifest_sha256),
            ("suite", &self.suite_sha256),
            ("cohort", &self.cohort_sha256),
            ("run budget", &self.run_budget_sha256),
            ("outcome budget", &self.outcome_budget_sha256),
            ("campaign budget", &self.campaign_budget_sha256),
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
        if self.authorization_sha256 != authorization_digest(self)? {
            return Err("successor authorization digest is invalid".into());
        }
        if self.cells.iter().enumerate().any(|(index, cell)| {
            cell.ordinal != index + 1
                || !matches!(cell.arm_order.as_str(), "direct_first" | "workflow_first")
                || !matches!(cell.workflow_policy.as_str(), "baseline" | "candidate")
        }) {
            return Err("successor authorization cells are invalid".into());
        }
        let expected = self
            .run_budget
            .maximum_charge()?
            .checked_mul(RUN_COUNT as u64)?;
        if !expected.matches_campaign(&self.campaign_budget) {
            return Err("successor authorization campaign budget is not six run budgets".into());
        }
        Ok(())
    }
}

impl AuthorizationCellV1 {
    pub(super) fn execution_order(&self) -> [ArmKindV1; 2] {
        if self.arm_order == "workflow_first" {
            [ArmKindV1::Workflow, ArmKindV1::Direct]
        } else {
            [ArmKindV1::Direct, ArmKindV1::Workflow]
        }
    }
}

impl ConsumedTombstoneV1 {
    pub(super) fn validate_for(&self, authorization: &AuthorizationV1) -> Result<(), String> {
        if self.schema != TOMBSTONE_SCHEMA
            || &self.authorization != authorization
            || self.authorization_sha256 != authorization.authorization_sha256
            || self.authorization_path_sha256 != authorization.authorization_path_sha256
            || self.output_root_sha256 != authorization.output_root_sha256
            || self.runner_sha256 != authorization.runner_sha256
            || self.consumed_at_ms < authorization.issued_at_ms
            || self.consumed_at_ms >= authorization.expires_at_ms
            || self.tombstone_sha256 != tombstone_digest(self)?
        {
            return Err("successor consumed tombstone is invalid".into());
        }
        Ok(())
    }
}

fn validate_preflight(
    protocol: &ValidatedProtocol<'_>,
    preflight: &SuccessorPreflightReceipt,
    output_root: &Path,
) -> Result<(), String> {
    if preflight.schema != PREFLIGHT_SCHEMA
        || preflight.provider_calls_performed != 0
        || preflight.execution_authorized
        || !preflight.online_execution_requires_explicit_authorization
        || protocol.manifest.matrix.pair_count != PAIR_COUNT
        || protocol.manifest.matrix.run_count != RUN_COUNT
        || preflight.cells.len() != PAIR_COUNT
        || preflight.manifest_sha256 != sha256_hex(protocol.manifest_bytes)
        || preflight.suite_sha256 != sha256_hex(protocol.suite_bytes)
        || preflight.source_commit_sha256 != sha256_hex(preflight.source.head.as_bytes())
        || preflight.receipt_sha256 != preflight_digest(preflight)?
        || preflight.output_root_sha256 != path_sha256(&canonical_bound_path(output_root, false)?)
    {
        return Err("successor preflight binding is invalid for authorization".into());
    }
    for (index, (cell, materialized)) in protocol
        .manifest
        .matrix
        .cells
        .iter()
        .zip(&preflight.cells)
        .enumerate()
    {
        if cell.ordinal != index + 1
            || materialized.ordinal != cell.ordinal
            || materialized.case_id != cell.case_id
            || materialized.split != cell.split
            || materialized.purpose != cell.purpose
            || materialized.case_input_sha256 != cell.case_input_sha256
            || materialized.case_contract_sha256 != cell.case_contract_sha256
        {
            return Err("successor preflight cells do not match the frozen protocol".into());
        }
    }
    Ok(())
}

fn credential_fingerprint(nonce: &str, credential: &str) -> String {
    let mut bytes = CREDENTIAL_HASH_DOMAIN.to_vec();
    bytes.extend_from_slice(nonce.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(credential.as_bytes());
    sha256_hex(&bytes)
}

fn authorization_digest(value: &AuthorizationV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.authorization_sha256.clear();
    domain_digest(
        AUTHORIZATION_HASH_DOMAIN,
        &payload,
        "successor authorization",
    )
}

fn tombstone_digest(value: &ConsumedTombstoneV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.tombstone_sha256.clear();
    domain_digest(
        TOMBSTONE_HASH_DOMAIN,
        &payload,
        "successor consumed tombstone",
    )
}

fn preflight_digest(value: &SuccessorPreflightReceipt) -> Result<String, String> {
    let mut payload = value.clone();
    payload.receipt_sha256.clear();
    domain_digest(PREFLIGHT_HASH_DOMAIN, &payload, "successor preflight")
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

pub(super) fn canonical_json<T: Serialize>(value: &T, label: &str) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| format!("failed to encode {label}: {error}"))
}

pub(super) fn path_sha256(path: &Path) -> String {
    sha256_hex(path.as_os_str().as_encoded_bytes())
}

pub(super) fn canonical_bound_path(path: &Path, must_exist: bool) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err("successor control-plane path must be normalized and absolute".into());
    }
    if path.exists() {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("failed to inspect successor path: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("successor control-plane path must not be a symlink".into());
        }
        return path
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize successor path: {error}"));
    }
    if must_exist {
        return Err("successor control-plane path does not exist".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "successor control-plane path has no parent".to_string())?
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize successor path parent: {error}"))?;
    let name = path
        .file_name()
        .ok_or_else(|| "successor control-plane path has no file name".to_string())?;
    Ok(parent.join(name))
}

pub(super) fn read_private_file(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_PRIVATE_FILE_BYTES
    {
        return Err(format!("{label} is not a bounded private regular file"));
    }
    #[cfg(unix)]
    if metadata.mode() & 0o777 != 0o600 {
        return Err(format!("{label} permissions must be 0600"));
    }
    fs::read(path).map_err(|error| format!("failed to read {label}: {error}"))
}

pub(super) fn require_private_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{label} is not a private directory"));
    }
    #[cfg(unix)]
    if metadata.mode() & 0o777 != 0o700 {
        return Err(format!("{label} permissions must be 0700"));
    }
    Ok(())
}

pub(super) fn read_consumed_tombstone(root: &Path) -> Result<ConsumedTombstoneV1, String> {
    let bytes = read_private_file(
        &root.join(TOMBSTONE_FILE_NAME),
        "successor consumed tombstone",
    )?;
    let tombstone: ConsumedTombstoneV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid successor consumed tombstone JSON: {error}"))?;
    if canonical_json(&tombstone, "successor consumed tombstone")? != bytes {
        return Err("successor consumed tombstone is not canonical JSON".into());
    }
    tombstone.validate_for(&tombstone.authorization.clone())?;
    Ok(tombstone)
}

#[derive(Serialize)]
struct RootOnlyRecoveryV1<'a> {
    schema: &'a str,
    disposition: &'a str,
    reason: &'a str,
    output_root_sha256: String,
}

pub(super) fn write_root_only_recovery(root: &Path) -> Result<(), String> {
    let path = root.join(RECOVERY_FILE_NAME);
    if path.exists() {
        let bytes = read_private_file(&path, "successor root-only recovery")?;
        let expected = canonical_json(
            &RootOnlyRecoveryV1 {
                schema: "cindx.collaboration-successor-root-recovery.v1",
                disposition: "censored",
                reason: "interrupted_before_journal",
                output_root_sha256: path_sha256(root),
            },
            "successor root-only recovery",
        )?;
        return if bytes == expected {
            Ok(())
        } else {
            Err("successor root-only recovery is tampered".into())
        };
    }
    super::preflight::write_new_private_file_atomically(
        &path,
        &canonical_json(
            &RootOnlyRecoveryV1 {
                schema: "cindx.collaboration-successor-root-recovery.v1",
                disposition: "censored",
                reason: "interrupted_before_journal",
                output_root_sha256: path_sha256(root),
            },
            "successor root-only recovery",
        )?,
    )
}

pub(super) fn require_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(format!("{label} must be a lowercase SHA-256 digest"));
    }
    Ok(())
}

fn require_nonce(value: &str) -> Result<(), String> {
    require_sha256(value, "successor authorization nonce")
}
