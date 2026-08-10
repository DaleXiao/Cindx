use super::super::collaboration_learning_capture::CollaborationLearningFrozenCaptureAuthority;
use super::super::materialize_case;
use super::super::verification::case_input_sha256;
use super::super::workflow_gepa_campaign_execution::{
    campaign_workspace_sha256, workflow_gepa_product_budget, MatchedRoutePairRun,
};
use super::{
    parse_and_validate_protocol, ValidatedProtocol, PROTOCOL_ID, PROTOCOL_RELATIVE_PATH,
    PROTOCOL_SCHEMA, SUITE_RELATIVE_PATH,
};
use crate::configuration_models::ProviderConfig;
use agent_application::{
    CollaborationLearningArmOrderV1, CollaborationLearningPairV1, CollaborationLearningSplitV1,
};
use agent_core::ModelRole;
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const OUTPUT_ROOT_ENV: &str = "CINDX_COLLABORATION_SUCCESSOR_OUTPUT_ROOT";
const RECEIPT_ENV: &str = "CINDX_COLLABORATION_SUCCESSOR_PREFLIGHT_RECEIPT";
const PREFLIGHT_RECEIPT_SCHEMA: &str = "cindx.collaboration-successor-preflight.v1";
const RECEIPT_HASH_DOMAIN: &[u8] = b"cindx.collaboration-successor-preflight.v1\0";
const COHORT_HASH_DOMAIN: &[u8] = b"cindx.collaboration-successor-cohort.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceBindingReceipt {
    pub(super) head: String,
    pub(super) tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct ProviderBindingReceipt {
    pub(super) credential_present: bool,
    pub(super) provider_config_sha256: String,
    pub(super) provider_identity_sha256: String,
    pub(super) capture_model_pool_sha256: String,
    pub(super) role_model_sha256: BTreeMap<String, String>,
    pub(super) ordered_model_pool_sha256: String,
    pub(super) ordered_model_member_sha256: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MaterializedCellReceipt {
    pub(super) ordinal: usize,
    pub(super) case_id: String,
    pub(super) split: String,
    pub(super) purpose: String,
    pub(super) case_input_sha256: String,
    pub(super) case_contract_sha256: String,
    pub(super) workspace_prestate_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SuccessorPreflightReceipt {
    pub(super) schema: String,
    pub(super) app_version: String,
    pub(super) protocol_id: String,
    pub(super) suite_id: String,
    pub(super) created_at_ms: u64,
    pub(super) source: SourceBindingReceipt,
    pub(super) source_commit_sha256: String,
    pub(super) manifest_sha256: String,
    pub(super) suite_sha256: String,
    pub(super) provider: ProviderBindingReceipt,
    pub(super) run_budget_sha256: String,
    pub(super) outcome_budget_sha256: String,
    pub(super) campaign_budget_sha256: String,
    pub(super) cohort_sha256: String,
    pub(super) output_root_sha256: String,
    pub(super) cells: Vec<MaterializedCellReceipt>,
    pub(super) provider_calls_performed: u64,
    pub(super) execution_authorized: bool,
    pub(super) online_execution_requires_explicit_authorization: bool,
    pub(super) receipt_sha256: String,
}

pub(super) struct ObservedPairBinding<'a> {
    pub(super) split: CollaborationLearningSplitV1,
    pub(super) replicate: u16,
    pub(super) arm_order: CollaborationLearningArmOrderV1,
    pub(super) source_commit_sha256: &'a str,
    pub(super) suite_sha256: &'a str,
    pub(super) case_sha256: &'a str,
    pub(super) prestate_sha256: &'a str,
    pub(super) provider_sha256: &'a str,
    pub(super) model_pool_sha256: &'a str,
    pub(super) provider_config_sha256: &'a str,
    pub(super) role_model_sha256: &'a BTreeMap<String, String>,
    pub(super) ordered_model_pool_sha256: &'a str,
    pub(super) ordered_model_member_sha256: &'a [String],
    pub(super) budget_sha256: &'a str,
    pub(super) cohort_sha256: &'a str,
    pub(super) workflow_policy_sha256: &'a str,
}

pub(in super::super) fn run_preflight() -> Result<(), String> {
    if std::env::args_os().len() != 1 {
        return Err("collaboration successor preflight accepts no command-line arguments".into());
    }
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))?;
    let source = clean_source_binding(&repo_root)?;
    let manifest_path = repo_root.join(PROTOCOL_RELATIVE_PATH);
    let suite_path = repo_root.join(SUITE_RELATIVE_PATH);
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let suite_bytes = fs::read(&suite_path)
        .map_err(|error| format!("failed to read {}: {error}", suite_path.display()))?;
    let protocol = parse_and_validate_protocol(&manifest_bytes, &suite_bytes)?;

    let output_root = required_new_external_path(OUTPUT_ROOT_ENV, &repo_root, "output root")?;
    let receipt_path = required_new_external_path(RECEIPT_ENV, &repo_root, "preflight receipt")?;
    if receipt_path == output_root || receipt_path.starts_with(&output_root) {
        return Err("preflight receipt must be separate from the reserved output root".into());
    }

    let config = crate::configuration_persistence::load_provider_config();
    let provider = provider_binding(&config)?;
    let cells = materialize_cells(&protocol)?;
    let receipt = build_receipt(
        &protocol,
        source,
        provider,
        cells,
        &output_root,
        now_millis()?,
    )?;
    let mut encoded = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| format!("failed to encode preflight receipt: {error}"))?;
    encoded.push(b'\n');
    if clean_source_binding(&repo_root)? != receipt.source {
        return Err("source changed while successor preflight was running".into());
    }
    write_new_private_file_atomically(&receipt_path, &encoded)?;
    eprintln!(
        "[collaboration-successor-preflight] receipt={} digest={} provider_calls=0 execution_authorized=false",
        receipt_path.display(),
        receipt.receipt_sha256
    );
    Ok(())
}

pub(in super::super) fn provider_binding(
    config: &ProviderConfig,
) -> Result<ProviderBindingReceipt, String> {
    let credential_present = !config.api_key.trim().is_empty();
    if !config.is_ready() || !credential_present {
        return Err("configured provider is required for a successor preflight binding".into());
    }
    let role_models = BTreeMap::from([
        ("conductor", config.model_for_conductor()),
        ("default", config.model.clone()),
        ("executor", config.model_for_role(&ModelRole::Executor)),
        ("planner", config.model_for_role(&ModelRole::Planner)),
        ("reviewer", config.model_for_role(&ModelRole::Reviewer)),
        ("summarizer", config.model_for_role(&ModelRole::Summarizer)),
    ]);
    if role_models.values().any(|model| model.trim().is_empty()) {
        return Err("successor preflight requires every collaboration model role".into());
    }
    let ordered_pool = crate::collaboration_execution::collaboration_candidate_models(config, 3);
    if ordered_pool.len() != 3 {
        return Err("successor preflight requires exactly three distinct worker models".into());
    }
    let capture_pool = super::super::configured_models(config)
        .into_values()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if capture_pool.is_empty() {
        return Err("successor preflight requires the capture model pool".into());
    }
    let provider_config_bytes = serde_json::to_vec(&serde_json::json!({
        "provider_id": config.provider_id,
        "provider_resource": config.provider_resource,
        "base_url": config.base_url,
        "credential_present": credential_present,
        "role_models": &role_models,
        "collaboration_policy": config.collaboration_policy,
        "context_window_tokens": config.context_window_tokens,
        "agent_system_prompt": config.agent_system_prompt,
    }))
    .map_err(|error| format!("failed to hash provider config: {error}"))?;
    let pool_bytes = serde_json::to_vec(&ordered_pool)
        .map_err(|error| format!("failed to hash model pool: {error}"))?;
    let capture_pool_bytes = serde_json::to_vec(&capture_pool)
        .map_err(|error| format!("failed to hash capture model pool: {error}"))?;
    Ok(ProviderBindingReceipt {
        credential_present,
        provider_config_sha256: sha256_hex(&provider_config_bytes),
        provider_identity_sha256: sha256_hex(
            format!("{}\0{}", config.provider_id, config.base_url).as_bytes(),
        ),
        capture_model_pool_sha256: sha256_hex(&capture_pool_bytes),
        role_model_sha256: role_models
            .into_iter()
            .map(|(role, model)| (role.to_string(), sha256_hex(model.as_bytes())))
            .collect(),
        ordered_model_pool_sha256: sha256_hex(&pool_bytes),
        ordered_model_member_sha256: ordered_pool
            .iter()
            .map(|model| sha256_hex(model.as_bytes()))
            .collect(),
    })
}

pub(super) fn materialize_cells(
    protocol: &ValidatedProtocol<'_>,
) -> Result<Vec<MaterializedCellReceipt>, String> {
    let temp = tempfile::Builder::new()
        .prefix("cindx-collaboration-successor-preflight-")
        .tempdir()
        .map_err(|error| format!("failed to create preflight workspace: {error}"))?;
    protocol
        .manifest
        .matrix
        .cells
        .iter()
        .map(|cell| {
            let case = protocol
                .suite
                .cases
                .iter()
                .find(|case| case.id == cell.case_id)
                .ok_or_else(|| format!("missing successor case {}", cell.case_id))?;
            let direct = temp.path().join(format!("cell-{:02}-direct", cell.ordinal));
            let workflow = temp
                .path()
                .join(format!("cell-{:02}-workflow", cell.ordinal));
            materialize_case(&direct, case)?;
            materialize_case(&workflow, case)?;
            let direct_sha256 = campaign_workspace_sha256(&direct)?;
            let workflow_sha256 = campaign_workspace_sha256(&workflow)?;
            if direct_sha256 != workflow_sha256 {
                return Err(format!(
                    "successor cell {} did not materialize matched workspaces",
                    cell.ordinal
                ));
            }
            Ok(MaterializedCellReceipt {
                ordinal: cell.ordinal,
                case_id: cell.case_id.clone(),
                split: cell.split.clone(),
                purpose: cell.purpose.clone(),
                case_input_sha256: case_input_sha256(case),
                case_contract_sha256: cell.case_contract_sha256.clone(),
                workspace_prestate_sha256: direct_sha256,
            })
        })
        .collect()
}

pub(super) fn validate_current_preflight_authority(
    protocol: &ValidatedProtocol<'_>,
    receipt: &SuccessorPreflightReceipt,
    repo_root: &Path,
    config: &ProviderConfig,
    output_root: &Path,
) -> Result<(), String> {
    validate_preflight_snapshot(
        protocol,
        receipt,
        &clean_source_binding(repo_root)?,
        &provider_binding(config)?,
        &materialize_cells(protocol)?,
        output_root,
    )
}

pub(super) fn validate_preflight_snapshot(
    protocol: &ValidatedProtocol<'_>,
    receipt: &SuccessorPreflightReceipt,
    source: &SourceBindingReceipt,
    provider: &ProviderBindingReceipt,
    cells: &[MaterializedCellReceipt],
    output_root: &Path,
) -> Result<(), String> {
    validate_preflight_authority(protocol, receipt)?;
    if source != &receipt.source
        || provider != &receipt.provider
        || cells != receipt.cells
        || receipt.output_root_sha256 != sha256_hex(output_root.as_os_str().as_encoded_bytes())
    {
        return Err("successor preflight no longer matches the current execution authority".into());
    }
    Ok(())
}

pub(super) fn build_receipt(
    protocol: &ValidatedProtocol<'_>,
    source: SourceBindingReceipt,
    provider: ProviderBindingReceipt,
    cells: Vec<MaterializedCellReceipt>,
    output_root: &Path,
    created_at_ms: u64,
) -> Result<SuccessorPreflightReceipt, String> {
    validate_git_object_id(&source.head, "source HEAD")?;
    validate_git_object_id(&source.tree, "source tree")?;
    let source_commit_sha256 = sha256_hex(source.head.as_bytes());
    let manifest_sha256 = sha256_hex(protocol.manifest_bytes);
    let suite_sha256 = sha256_hex(protocol.suite_bytes);
    let run_budget_sha256 = sha256_json(&protocol.manifest.run_budget)?;
    let outcome_budget_sha256 =
        super::super::outcome_shadow::outcome_budget_sha256(workflow_gepa_product_budget())?;
    let campaign_budget_sha256 = sha256_json(&protocol.manifest.campaign_budget)?;
    let cohort_sha256 = successor_cohort_sha256(
        protocol,
        &source_commit_sha256,
        &manifest_sha256,
        &suite_sha256,
        &provider,
        &run_budget_sha256,
        &outcome_budget_sha256,
    )?;
    let mut receipt = SuccessorPreflightReceipt {
        schema: PREFLIGHT_RECEIPT_SCHEMA.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_id: protocol.manifest.id.clone(),
        suite_id: protocol.suite.id.clone(),
        created_at_ms,
        source,
        source_commit_sha256,
        manifest_sha256,
        suite_sha256,
        provider,
        run_budget_sha256,
        outcome_budget_sha256,
        campaign_budget_sha256,
        cohort_sha256,
        output_root_sha256: sha256_hex(output_root.as_os_str().as_encoded_bytes()),
        cells,
        provider_calls_performed: 0,
        execution_authorized: false,
        online_execution_requires_explicit_authorization: true,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt_payload_sha256(&receipt)?;
    Ok(receipt)
}

fn successor_cohort_sha256(
    protocol: &ValidatedProtocol<'_>,
    source_commit_sha256: &str,
    manifest_sha256: &str,
    suite_sha256: &str,
    provider: &ProviderBindingReceipt,
    run_budget_sha256: &str,
    outcome_budget_sha256: &str,
) -> Result<String, String> {
    let cells = protocol
        .manifest
        .matrix
        .cells
        .iter()
        .map(|cell| {
            (
                cell.ordinal,
                cell.case_id.as_str(),
                cell.split.as_str(),
                cell.purpose.as_str(),
                cell.workflow_policy.as_str(),
                cell.case_input_sha256.as_str(),
                cell.case_contract_sha256.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let provider_payload = (
        provider.provider_identity_sha256.as_str(),
        provider.capture_model_pool_sha256.as_str(),
        provider.provider_config_sha256.as_str(),
        &provider.role_model_sha256,
        provider.ordered_model_pool_sha256.as_str(),
        &provider.ordered_model_member_sha256,
    );
    let payload = (
        PROTOCOL_SCHEMA,
        PROTOCOL_ID,
        env!("CARGO_PKG_VERSION"),
        manifest_sha256,
        suite_sha256,
        source_commit_sha256,
        provider_payload,
        run_budget_sha256,
        outcome_budget_sha256,
        protocol.manifest.admission.config_sha256.as_str(),
        protocol.manifest.policies.direct_policy_sha256.as_str(),
        protocol
            .manifest
            .policies
            .baseline_workflow_policy_sha256
            .as_str(),
        protocol
            .manifest
            .policies
            .candidate_workflow_policy_sha256
            .as_str(),
        cells,
    );
    let mut bytes = COHORT_HASH_DOMAIN.to_vec();
    bytes.extend(
        serde_json::to_vec(&payload)
            .map_err(|error| format!("failed to hash successor cohort: {error}"))?,
    );
    Ok(sha256_hex(&bytes))
}

#[allow(dead_code)]
pub(in super::super) fn validate_observed_pair(
    manifest_bytes: &[u8],
    suite_bytes: &[u8],
    preflight_receipt_bytes: &[u8],
    cell_ordinal: usize,
    matched_pair: &MatchedRoutePairRun,
    pair: &CollaborationLearningPairV1,
) -> Result<(), String> {
    let protocol = parse_and_validate_protocol(manifest_bytes, suite_bytes)?;
    let receipt: SuccessorPreflightReceipt = serde_json::from_slice(preflight_receipt_bytes)
        .map_err(|error| format!("invalid successor preflight receipt JSON: {error}"))?;
    let binding = pair.binding();
    let hashes = binding.hashes();
    let actual_provider = matched_pair
        .successor_provider_binding
        .as_ref()
        .ok_or_else(|| "observed pair lacks its successor provider binding".to_string())?;
    if matched_pair.suite_sha256 != hashes.suite_sha256
        || matched_pair.provider_sha256 != actual_provider.provider_identity_sha256
        || hashes.provider_sha256 != actual_provider.provider_identity_sha256
        || hashes.model_pool_sha256 != actual_provider.capture_model_pool_sha256
    {
        return Err("observed pair disagrees with its successor execution environment".into());
    }
    let capture_authority = CollaborationLearningFrozenCaptureAuthority::new(
        receipt.source_commit_sha256.clone(),
        receipt.cohort_sha256.clone(),
    )?;
    let projected_pair = matched_pair.project_collaboration_learning_pair(&capture_authority)?;
    if projected_pair.digest() != pair.digest() {
        return Err("observed pair was not projected from the supplied matched execution".into());
    }
    validate_observed_pair_binding(
        &protocol,
        &receipt,
        cell_ordinal,
        ObservedPairBinding {
            split: binding.split(),
            replicate: binding.replicate(),
            arm_order: binding.arm_order(),
            source_commit_sha256: &hashes.source_commit_sha256,
            suite_sha256: &hashes.suite_sha256,
            case_sha256: &hashes.case_sha256,
            prestate_sha256: &hashes.prestate_sha256,
            provider_sha256: &hashes.provider_sha256,
            model_pool_sha256: &hashes.model_pool_sha256,
            provider_config_sha256: &actual_provider.provider_config_sha256,
            role_model_sha256: &actual_provider.role_model_sha256,
            ordered_model_pool_sha256: &actual_provider.ordered_model_pool_sha256,
            ordered_model_member_sha256: &actual_provider.ordered_model_member_sha256,
            budget_sha256: &hashes.budget_sha256,
            cohort_sha256: &hashes.cohort_sha256,
            workflow_policy_sha256: pair.workflow_policy_sha256(),
        },
    )
}

pub(super) fn validate_observed_pair_binding(
    protocol: &ValidatedProtocol<'_>,
    receipt: &SuccessorPreflightReceipt,
    cell_ordinal: usize,
    observed: ObservedPairBinding<'_>,
) -> Result<(), String> {
    validate_preflight_authority(protocol, receipt)?;
    let cell = protocol
        .manifest
        .matrix
        .cells
        .iter()
        .find(|cell| cell.ordinal == cell_ordinal)
        .ok_or_else(|| format!("unknown successor cell {cell_ordinal}"))?;
    let materialized = receipt
        .cells
        .iter()
        .find(|candidate| candidate.ordinal == cell_ordinal)
        .ok_or_else(|| format!("preflight omitted successor cell {cell_ordinal}"))?;
    let expected_split = match cell.split.as_str() {
        "train" => CollaborationLearningSplitV1::Train,
        "holdout" => CollaborationLearningSplitV1::Holdout,
        _ => return Err(format!("successor cell {} has invalid split", cell.ordinal)),
    };
    let expected_order = match cell.arm_order.as_str() {
        "direct_first" => CollaborationLearningArmOrderV1::DirectFirst,
        "workflow_first" => CollaborationLearningArmOrderV1::WorkflowFirst,
        _ => {
            return Err(format!(
                "successor cell {} has invalid arm order",
                cell.ordinal
            ))
        }
    };
    let expected_policy_sha256 = match cell.workflow_policy.as_str() {
        "baseline" => &protocol.manifest.policies.baseline_workflow_policy_sha256,
        "candidate" => &protocol.manifest.policies.candidate_workflow_policy_sha256,
        _ => {
            return Err(format!(
                "successor cell {} has invalid workflow policy",
                cell.ordinal
            ))
        }
    };
    if materialized.case_id != cell.case_id
        || materialized.split != cell.split
        || materialized.purpose != cell.purpose
        || materialized.case_input_sha256 != cell.case_input_sha256
        || materialized.case_contract_sha256 != cell.case_contract_sha256
        || observed.split != expected_split
        || observed.replicate != u16::try_from(cell.replicate).unwrap_or_default()
        || observed.arm_order != expected_order
        || observed.source_commit_sha256 != receipt.source_commit_sha256
        || observed.suite_sha256 != receipt.suite_sha256
        || observed.case_sha256 != cell.case_input_sha256
        || observed.prestate_sha256 != materialized.workspace_prestate_sha256
        || observed.provider_sha256 != receipt.provider.provider_identity_sha256
        || observed.model_pool_sha256 != receipt.provider.capture_model_pool_sha256
        || observed.provider_config_sha256 != receipt.provider.provider_config_sha256
        || observed.role_model_sha256 != &receipt.provider.role_model_sha256
        || observed.ordered_model_pool_sha256 != receipt.provider.ordered_model_pool_sha256
        || observed.ordered_model_member_sha256
            != receipt.provider.ordered_model_member_sha256.as_slice()
        || observed.budget_sha256 != receipt.outcome_budget_sha256
        || observed.cohort_sha256 != receipt.cohort_sha256
        || observed.workflow_policy_sha256 != expected_policy_sha256
    {
        return Err(format!(
            "observed pair does not match frozen successor cell {}",
            cell.ordinal
        ));
    }
    Ok(())
}

pub(super) fn validate_preflight_authority(
    protocol: &ValidatedProtocol<'_>,
    receipt: &SuccessorPreflightReceipt,
) -> Result<(), String> {
    validate_git_object_id(&receipt.source.head, "preflight source HEAD")?;
    validate_git_object_id(&receipt.source.tree, "preflight source tree")?;
    let source_commit_sha256 = sha256_hex(receipt.source.head.as_bytes());
    let manifest_sha256 = sha256_hex(protocol.manifest_bytes);
    let suite_sha256 = sha256_hex(protocol.suite_bytes);
    let run_budget_sha256 = sha256_json(&protocol.manifest.run_budget)?;
    let outcome_budget_sha256 =
        super::super::outcome_shadow::outcome_budget_sha256(workflow_gepa_product_budget())?;
    let campaign_budget_sha256 = sha256_json(&protocol.manifest.campaign_budget)?;
    let cohort_sha256 = successor_cohort_sha256(
        protocol,
        &source_commit_sha256,
        &manifest_sha256,
        &suite_sha256,
        &receipt.provider,
        &run_budget_sha256,
        &outcome_budget_sha256,
    )?;
    validate_materialized_cells(protocol, &receipt.cells)?;
    if receipt.schema != PREFLIGHT_RECEIPT_SCHEMA
        || receipt.app_version != env!("CARGO_PKG_VERSION")
        || receipt.protocol_id != protocol.manifest.id
        || receipt.suite_id != protocol.suite.id
        || receipt.source_commit_sha256 != source_commit_sha256
        || receipt.manifest_sha256 != manifest_sha256
        || receipt.suite_sha256 != suite_sha256
        || !receipt.provider.credential_present
        || receipt.run_budget_sha256 != run_budget_sha256
        || receipt.outcome_budget_sha256 != outcome_budget_sha256
        || receipt.campaign_budget_sha256 != campaign_budget_sha256
        || receipt.cohort_sha256 != cohort_sha256
        || receipt.provider_calls_performed != 0
        || receipt.execution_authorized
        || !receipt.online_execution_requires_explicit_authorization
        || receipt.receipt_sha256 != receipt_payload_sha256(receipt)?
    {
        return Err("successor preflight authority is invalid or execution-authorizing".into());
    }
    Ok(())
}

fn validate_materialized_cells(
    protocol: &ValidatedProtocol<'_>,
    materialized: &[MaterializedCellReceipt],
) -> Result<(), String> {
    if materialized.len() != protocol.manifest.matrix.cells.len() {
        return Err("successor preflight materialized the wrong number of cells".into());
    }
    for (cell, observed) in protocol.manifest.matrix.cells.iter().zip(materialized) {
        if observed.ordinal != cell.ordinal
            || observed.case_id != cell.case_id
            || observed.split != cell.split
            || observed.purpose != cell.purpose
            || observed.case_input_sha256 != cell.case_input_sha256
            || observed.case_contract_sha256 != cell.case_contract_sha256
            || !is_sha256(&observed.workspace_prestate_sha256)
        {
            return Err(format!(
                "successor preflight materialization does not match frozen cell {}",
                cell.ordinal
            ));
        }
    }
    Ok(())
}

pub(super) fn receipt_payload_sha256(
    receipt: &SuccessorPreflightReceipt,
) -> Result<String, String> {
    let mut payload = receipt.clone();
    payload.receipt_sha256.clear();
    let mut bytes = RECEIPT_HASH_DOMAIN.to_vec();
    bytes.extend(
        serde_json::to_vec(&payload)
            .map_err(|error| format!("failed to hash preflight receipt: {error}"))?,
    );
    Ok(sha256_hex(&bytes))
}

fn sha256_json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| format!("failed to hash frozen JSON: {error}"))
}

pub(super) fn clean_source_binding(repo_root: &Path) -> Result<SourceBindingReceipt, String> {
    let status = git_output(
        repo_root,
        &["status", "--porcelain", "--untracked-files=all"],
        "worktree status",
    )?;
    if !status.is_empty() {
        return Err("worktree must be clean before successor preflight".into());
    }
    let head = git_output(repo_root, &["rev-parse", "HEAD"], "source HEAD")?;
    let tree = git_output(repo_root, &["rev-parse", "HEAD^{tree}"], "source tree")?;
    validate_git_object_id(&head, "source HEAD")?;
    validate_git_object_id(&tree, "source tree")?;
    Ok(SourceBindingReceipt { head, tree })
}

fn git_output(repo_root: &Path, args: &[&str], label: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !output.status.success() {
        return Err(format!("git failed while resolving {label}"));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| format!("{label} is not UTF-8"))
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

pub(super) fn write_new_private_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "preflight receipt path has no parent".to_string())?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("collaboration-successor-preflight");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| format!("failed to stage preflight receipt: {error}"))?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure preflight receipt: {error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("failed to write preflight receipt: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("failed to sync preflight receipt: {error}"))?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| format!("failed to publish new preflight receipt: {}", error.error))?;
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
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

pub(super) fn validate_new_external_path(
    path: &Path,
    repo_root: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(format!("{label} must be a normalized absolute path"));
    }
    if path.exists() {
        return Err(format!("{label} must not already exist"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} has no parent"))?
        .canonicalize()
        .map_err(|error| format!("failed to resolve {label} parent: {error}"))?;
    if parent.starts_with(repo_root) {
        return Err(format!("{label} must remain outside the repository"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| format!("{label} has no final path component"))?;
    Ok(parent.join(name))
}

pub(super) fn now_millis() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))
}
