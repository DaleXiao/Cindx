use super::verification::case_input_sha256;
use super::workflow_gepa_campaign_execution::workflow_gepa_product_budget;
use super::{validate_relative_path, PermissionPolicy, RealworldCase, RealworldSuite};
use agent_application::{
    CollaborationLearningConfigV1, CollaborationLearningPolicyV1, CollaborationRepairV1,
    CollaborationSpecialistInvocationV1, CollaborationVerificationV1,
    COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS, COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
};
use agent_runtime::RunBudget;
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[cfg(test)]
use crate::configuration_models::ProviderConfig;
#[cfg(test)]
use std::{fs, path::Path};

const PROTOCOL_SCHEMA: &str = "cindx.collaboration-successor-protocol.v1";
const PROTOCOL_ID: &str = "cindx-collaboration-successor-protocol-v1";
const SUITE_SCHEMA: &str = "cindx.collaboration-successor-suite.v1";
const SUITE_ID: &str = "cindx-collaboration-successor-v1";
const PROTOCOL_RELATIVE_PATH: &str = "benchmarks/agent/collaboration-successor-protocol-v1.json";
const SUITE_RELATIVE_PATH: &str = "benchmarks/agent/collaboration-successor-v1.json";
const CASE_CONTRACT_HASH_DOMAIN: &[u8] = b"cindx.collaboration-successor-case-contract.v1\0";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuccessorProtocolManifest {
    schema: String,
    id: String,
    version: u32,
    suite: FrozenSuite,
    matrix: FrozenMatrix,
    policies: FrozenPolicies,
    run_budget: FrozenRunBudget,
    campaign_budget: FrozenCampaignBudget,
    admission: FrozenAdmission,
    stop_contract: FrozenStopContract,
    controls: FrozenControls,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenSuite {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenMatrix {
    pair_count: usize,
    run_count: usize,
    cells: Vec<FrozenCell>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenCell {
    ordinal: usize,
    case_id: String,
    split: String,
    purpose: String,
    replicate: u32,
    arm_order: String,
    workflow_policy: String,
    case_input_sha256: String,
    case_contract_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenPolicies {
    direct_policy_sha256: String,
    baseline_workflow_context_budget_bps: u16,
    baseline_workflow_policy_sha256: String,
    candidate_count: usize,
    candidate_changed_axis: String,
    candidate_workflow_context_budget_bps: u16,
    candidate_workflow_policy_sha256: String,
    repair: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenRunBudget {
    max_duration_ms: u64,
    model_call_timeout_ms: u64,
    tool_call_timeout_ms: u64,
    initial_model_calls: usize,
    max_model_calls: usize,
    model_calls_per_extension: usize,
    initial_tool_calls: usize,
    max_tool_calls: usize,
    tool_calls_per_extension: usize,
    no_progress_timeout_ms: u64,
    max_identical_actions: usize,
    initial_agent_turns: usize,
    max_agent_turns: usize,
    agent_turns_per_extension: usize,
    max_repair_attempts: usize,
    terminal_model_call_reserve: usize,
    terminal_time_reserve_ms: u64,
    max_total_tokens: u64,
    max_physical_model_attempts: usize,
    terminal_token_reserve: u64,
    terminal_physical_model_attempt_reserve: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenCampaignBudget {
    runs: usize,
    max_duration_ms: u64,
    max_model_calls: usize,
    max_tool_calls: usize,
    max_agent_turns: usize,
    max_physical_model_attempts: usize,
    max_total_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenAdmission {
    min_train_pairs: u16,
    min_holdout_pairs: u16,
    minimum_uplift_bps: u16,
    max_resource_regression_bps: u16,
    max_position_imbalance: u16,
    candidate_budget: u16,
    config_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenStopContract {
    baseline_non_positive: String,
    any_censor: String,
    safety_or_preservation_failure: String,
    resource_regression: String,
    candidate_requires: String,
    holdout_requires: String,
    started_physical_run_provider_retry: bool,
    no_uplift_action: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenControls {
    execution_authorized: bool,
    online_execution_requires_explicit_authorization: bool,
    outcomes_must_be_externally_verified: bool,
    incomplete_evidence_fails_closed: bool,
    holdout_is_training_ineligible: bool,
}

struct ValidatedProtocol<'a> {
    manifest: SuccessorProtocolManifest,
    suite: RealworldSuite,
    manifest_bytes: &'a [u8],
    suite_bytes: &'a [u8],
}

fn parse_and_validate_protocol<'a>(
    manifest_bytes: &'a [u8],
    suite_bytes: &'a [u8],
) -> Result<ValidatedProtocol<'a>, String> {
    let manifest: SuccessorProtocolManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("invalid collaboration successor manifest JSON: {error}"))?;
    validate_successor_suite_fields(suite_bytes)?;
    let suite: RealworldSuite = serde_json::from_slice(suite_bytes)
        .map_err(|error| format!("invalid collaboration successor suite JSON: {error}"))?;
    validate_manifest(&manifest, suite_bytes)?;
    validate_successor_suite(&suite)?;
    validate_matrix(&manifest, &suite, suite_bytes)?;
    Ok(ValidatedProtocol {
        manifest,
        suite,
        manifest_bytes,
        suite_bytes,
    })
}

fn validate_successor_suite_fields(suite_bytes: &[u8]) -> Result<(), String> {
    let suite: serde_json::Value = serde_json::from_slice(suite_bytes)
        .map_err(|error| format!("invalid collaboration successor suite JSON: {error}"))?;
    validate_object_fields(
        &suite,
        &[
            "schema",
            "id",
            "version",
            "description",
            "default_replicates",
            "per_run_timeout_seconds",
            "treatments",
            "execution_order",
            "cases",
        ],
        "suite",
    )?;
    let suite_object = suite
        .as_object()
        .ok_or_else(|| "collaboration successor suite must be an object".to_string())?;
    let execution_order = suite_object
        .get("execution_order")
        .ok_or_else(|| "collaboration successor suite omitted execution_order".to_string())?;
    validate_object_fields(
        execution_order,
        &["protocol", "base_treatments"],
        "execution_order",
    )?;
    let cases = suite_object
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "collaboration successor suite cases must be an array".to_string())?;
    for (case_index, case) in cases.iter().enumerate() {
        validate_object_fields(
            case,
            &[
                "id",
                "category",
                "objective",
                "campaign_split",
                "expected_execution_mode",
                "seed_memory_prompt",
                "index_workspace",
                "files",
                "permission_policy",
                "verification",
                "memory_effect",
            ],
            &format!("cases[{case_index}]"),
        )?;
        let case_object = case
            .as_object()
            .ok_or_else(|| format!("successor cases[{case_index}] must be an object"))?;
        let files = case_object
            .get("files")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("successor cases[{case_index}].files must be an array"))?;
        for (file_index, file) in files.iter().enumerate() {
            validate_object_fields(
                file,
                &["path", "content"],
                &format!("cases[{case_index}].files[{file_index}]"),
            )?;
        }
        let verification = case_object.get("verification").ok_or_else(|| {
            format!("collaboration successor cases[{case_index}] omitted verification")
        })?;
        validate_successor_verification_fields(verification, case_index)?;
    }
    Ok(())
}

fn validate_successor_verification_fields(
    verification: &serde_json::Value,
    case_index: usize,
) -> Result<(), String> {
    validate_object_fields(
        verification,
        &[
            "direct_output_contains",
            "output_contains",
            "output_not_contains",
            "json_files",
            "exact_files",
            "immutable_files",
            "file_contains",
            "commands",
            "required_tools_any",
            "required_tools_all",
            "allowed_tools",
            "browser_target_receipt",
            "minimum_denied_permissions",
        ],
        &format!("cases[{case_index}].verification"),
    )?;
    let verification_object = verification
        .as_object()
        .ok_or_else(|| format!("successor cases[{case_index}].verification must be an object"))?;
    for (field, allowed_fields) in [
        ("json_files", &["path", "equals"][..]),
        ("exact_files", &["path", "content"][..]),
        ("file_contains", &["path", "values"][..]),
        ("commands", &["program", "args", "stdout_contains"][..]),
    ] {
        let Some(entries) = verification_object.get(field) else {
            continue;
        };
        let entries = entries.as_array().ok_or_else(|| {
            format!("successor cases[{case_index}].verification.{field} must be an array")
        })?;
        for (entry_index, entry) in entries.iter().enumerate() {
            validate_object_fields(
                entry,
                allowed_fields,
                &format!("cases[{case_index}].verification.{field}[{entry_index}]"),
            )?;
        }
    }
    if let Some(browser) = verification_object.get("browser_target_receipt") {
        if !browser.is_null() {
            validate_object_fields(
                browser,
                &["tools_all", "tools_any", "minimum_artifacts"],
                &format!("cases[{case_index}].verification.browser_target_receipt"),
            )?;
        }
    }
    Ok(())
}

fn validate_object_fields(
    value: &serde_json::Value,
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("collaboration successor {context} must be an object"))?;
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(format!(
            "collaboration successor {context} contains unknown field {field}"
        ));
    }
    Ok(())
}

fn validate_manifest(
    manifest: &SuccessorProtocolManifest,
    suite_bytes: &[u8],
) -> Result<(), String> {
    if manifest.schema != PROTOCOL_SCHEMA || manifest.id != PROTOCOL_ID || manifest.version != 1 {
        return Err("collaboration successor manifest identity is invalid".into());
    }
    if manifest.suite.path != SUITE_RELATIVE_PATH
        || manifest.suite.sha256 != sha256_hex(suite_bytes)
    {
        return Err("collaboration successor suite binding is invalid".into());
    }
    validate_policies(&manifest.policies)?;
    let expected_run_budget = freeze_run_budget(workflow_gepa_product_budget());
    if manifest.run_budget != expected_run_budget {
        return Err("successor run budget differs from workflow_gepa_product_budget".into());
    }
    let expected_campaign_budget = campaign_budget(&expected_run_budget, 6)?;
    if manifest.campaign_budget != expected_campaign_budget {
        return Err("successor campaign budget is not exactly six run budgets".into());
    }
    validate_admission(&manifest.admission)?;
    validate_stop_contract(&manifest.stop_contract)?;
    let controls = &manifest.controls;
    if controls.execution_authorized
        || !controls.online_execution_requires_explicit_authorization
        || !controls.outcomes_must_be_externally_verified
        || !controls.incomplete_evidence_fails_closed
        || !controls.holdout_is_training_ineligible
    {
        return Err("successor fail-closed controls are invalid".into());
    }
    Ok(())
}

fn validate_admission(admission: &FrozenAdmission) -> Result<(), String> {
    let expected = CollaborationLearningConfigV1::freeze(1, 1, 1, 2_500, 1, 1)
        .map_err(|error| error.to_string())?;
    if admission.min_train_pairs != 1
        || admission.min_holdout_pairs != 1
        || admission.minimum_uplift_bps != 1
        || admission.max_resource_regression_bps != 2_500
        || admission.max_position_imbalance != 1
        || admission.candidate_budget != 1
        || admission.config_sha256 != expected.digest()
    {
        return Err("successor admission config is not the frozen one-candidate gate".into());
    }
    Ok(())
}

fn validate_stop_contract(stop: &FrozenStopContract) -> Result<(), String> {
    if stop.baseline_non_positive != "terminal_freeze"
        || stop.any_censor != "terminal_freeze"
        || stop.safety_or_preservation_failure != "terminal_freeze"
        || stop.resource_regression != "terminal_freeze"
        || stop.candidate_requires != "valid_positive_baseline"
        || stop.holdout_requires != "candidate_train_pass"
        || stop.started_physical_run_provider_retry
        || stop.no_uplift_action != "freeze_collaboration_type"
    {
        return Err("successor terminal stop contract is invalid".into());
    }
    Ok(())
}

fn validate_policies(policies: &FrozenPolicies) -> Result<(), String> {
    let direct = CollaborationLearningPolicyV1::seed(
        CollaborationSpecialistInvocationV1::DirectOwnerOnly,
        0,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .map_err(|error| error.to_string())?;
    let baseline = CollaborationLearningPolicyV1::seed(
        CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
        COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .map_err(|error| error.to_string())?;
    let candidate = CollaborationLearningPolicyV1::candidate(
        &baseline,
        CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
        COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .map_err(|error| error.to_string())?;
    if policies.direct_policy_sha256 != direct.policy_sha256
        || policies.baseline_workflow_context_budget_bps != COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS
        || policies.baseline_workflow_policy_sha256 != baseline.policy_sha256
        || policies.candidate_count != 1
        || policies.candidate_changed_axis != "context_budget_bps"
        || policies.candidate_workflow_context_budget_bps
            != COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS
        || policies.candidate_workflow_policy_sha256 != candidate.policy_sha256
        || policies.repair != "fail_fast"
    {
        return Err("successor policies are not the frozen baseline and sole candidate".into());
    }
    Ok(())
}

fn validate_successor_suite(suite: &RealworldSuite) -> Result<(), String> {
    if suite.schema != SUITE_SCHEMA
        || suite.id != SUITE_ID
        || suite.version != 1
        || suite.default_replicates != 1
        || suite.per_run_timeout_seconds != 600
        || suite.treatments.as_slice() != ["pro"]
        || suite.execution_order.protocol != "successor_fixed_pairs_v1"
        || suite.execution_order.base_treatments.as_slice() != ["pro"]
        || suite.cases.len() != 3
    {
        return Err("collaboration successor suite identity or shape is invalid".into());
    }
    let expected = [
        (
            "coding-compact-maintenance-windows",
            "coding",
            "train",
            "csv1_1/",
        ),
        (
            "research-assign-retention-controls",
            "research",
            "train",
            "csv1_2/",
        ),
        ("coding-drain-fair-queues", "coding", "holdout", "csv1_3/"),
    ];
    let mut all_paths = BTreeSet::new();
    for (case, (id, category, split, prefix)) in suite.cases.iter().zip(expected) {
        validate_successor_case(case, id, category, split, prefix, &mut all_paths)?;
    }
    Ok(())
}

fn validate_successor_case(
    case: &RealworldCase,
    expected_id: &str,
    expected_category: &str,
    expected_split: &str,
    path_prefix: &str,
    all_paths: &mut BTreeSet<String>,
) -> Result<(), String> {
    if case.id != expected_id
        || case.category != expected_category
        || case.campaign_split.as_deref() != Some(expected_split)
        || case.objective.trim().is_empty()
        || case.objective.contains("workflow")
        || case.objective.contains("specialist")
        || case.expected_execution_mode.is_some()
        || case.seed_memory_prompt.is_some()
        || case.index_workspace
        || case.memory_effect.is_some()
        || case.files.len() < 3
        || !matches!(case.permission_policy, PermissionPolicy::AllowOnce)
    {
        return Err(format!(
            "successor case {} identity or route blindness is invalid",
            case.id
        ));
    }
    let mut case_paths = BTreeSet::new();
    for fixture in &case.files {
        validate_relative_path(&fixture.path)?;
        if !fixture.path.starts_with(path_prefix)
            || !case_paths.insert(fixture.path.clone())
            || !all_paths.insert(fixture.path.clone())
        {
            return Err(format!(
                "successor case {} has an invalid fixture path",
                case.id
            ));
        }
    }
    let verification = &case.verification;
    if verification.direct_output_contains.len() != 1
        || !verification.output_contains.is_empty()
        || !verification.output_not_contains.is_empty()
        || !verification.exact_files.is_empty()
        || !verification.file_contains.is_empty()
        || verification.commands.len() != 4
        || verification.commands.iter().any(|command| {
            command.program != "node"
                || command.args.is_empty()
                || command.stdout_contains.trim().is_empty()
        })
        || verification.required_tools_all.as_slice() != ["shell.run"]
        || verification.required_tools_any.is_empty()
        || verification
            .required_tools_any
            .iter()
            .any(|tool| !tool.starts_with("file."))
        || verification.browser_target_receipt.is_some()
        || verification.minimum_denied_permissions != 0
    {
        return Err(format!(
            "successor case {} verification is incomplete",
            case.id
        ));
    }
    for path in &verification.immutable_files {
        validate_relative_path(path)?;
        if !case_paths.contains(path) {
            return Err(format!(
                "successor case {} mutates immutable evidence",
                case.id
            ));
        }
    }
    for output in &verification.json_files {
        validate_relative_path(&output.path)?;
        if case_paths.contains(&output.path) {
            return Err(format!(
                "successor case {} pre-seeds its JSON output",
                case.id
            ));
        }
    }
    if verification.immutable_files.len() < 2
        || !verification
            .immutable_files
            .iter()
            .any(|path| path.ends_with("/check.mjs"))
        || !verification
            .immutable_files
            .iter()
            .any(|path| path.ends_with("/spec.md") || path.ends_with("/policy.txt"))
    {
        return Err(format!(
            "successor case {} lacks immutable evidence",
            case.id
        ));
    }
    if expected_category == "research" && verification.json_files.len() != 1
        || expected_category == "coding" && !verification.json_files.is_empty()
    {
        return Err(format!(
            "successor case {} has the wrong output contract",
            case.id
        ));
    }
    Ok(())
}

fn validate_matrix(
    manifest: &SuccessorProtocolManifest,
    suite: &RealworldSuite,
    suite_bytes: &[u8],
) -> Result<(), String> {
    if manifest.matrix.pair_count != 3
        || manifest.matrix.run_count != 6
        || manifest.matrix.cells.len() != 3
    {
        return Err("successor matrix must contain exactly three pairs and six runs".into());
    }
    let expected = [
        (
            1,
            "coding-compact-maintenance-windows",
            "train",
            "baseline",
            "direct_first",
            "baseline",
        ),
        (
            2,
            "research-assign-retention-controls",
            "train",
            "candidate",
            "workflow_first",
            "candidate",
        ),
        (
            3,
            "coding-drain-fair-queues",
            "holdout",
            "sealed_holdout",
            "direct_first",
            "candidate",
        ),
    ];
    for (cell, expected) in manifest.matrix.cells.iter().zip(expected) {
        if (
            cell.ordinal,
            cell.case_id.as_str(),
            cell.split.as_str(),
            cell.purpose.as_str(),
            cell.arm_order.as_str(),
            cell.workflow_policy.as_str(),
        ) != expected
            || cell.replicate != 1
            || !suite.cases.iter().any(|case| {
                case.id == cell.case_id
                    && case.campaign_split.as_deref() == Some(&cell.split)
                    && case_input_sha256(case) == cell.case_input_sha256
            })
            || case_contract_sha256(suite_bytes, &cell.case_id)? != cell.case_contract_sha256
        {
            return Err(format!("successor matrix cell {} is invalid", cell.ordinal));
        }
    }
    Ok(())
}

fn case_contract_sha256(suite_bytes: &[u8], case_id: &str) -> Result<String, String> {
    let suite: serde_json::Value = serde_json::from_slice(suite_bytes)
        .map_err(|error| format!("failed to parse suite contract: {error}"))?;
    let case = suite
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .and_then(|cases| {
            cases
                .iter()
                .find(|case| case.get("id").and_then(serde_json::Value::as_str) == Some(case_id))
        })
        .ok_or_else(|| format!("suite is missing full contract for {case_id}"))?;
    let mut bytes = CASE_CONTRACT_HASH_DOMAIN.to_vec();
    write_canonical_json(case, &mut bytes)?;
    Ok(sha256_hex(&bytes))
}

fn write_canonical_json(value: &serde_json::Value, output: &mut Vec<u8>) -> Result<(), String> {
    match value {
        serde_json::Value::Null => output.extend_from_slice(b"null"),
        serde_json::Value::Bool(value) => {
            output.extend_from_slice(if *value { b"true" } else { b"false" })
        }
        serde_json::Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        serde_json::Value::String(value) => output.extend(
            serde_json::to_vec(value)
                .map_err(|error| format!("failed to canonicalize JSON string: {error}"))?,
        ),
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        serde_json::Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                output.extend(
                    serde_json::to_vec(key)
                        .map_err(|error| format!("failed to canonicalize JSON key: {error}"))?,
                );
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn freeze_run_budget(budget: RunBudget) -> FrozenRunBudget {
    FrozenRunBudget {
        max_duration_ms: duration_millis(budget.max_duration),
        model_call_timeout_ms: duration_millis(budget.model_call_timeout),
        tool_call_timeout_ms: duration_millis(budget.tool_call_timeout),
        initial_model_calls: budget.initial_model_calls,
        max_model_calls: budget.max_model_calls,
        model_calls_per_extension: budget.model_calls_per_extension,
        initial_tool_calls: budget.initial_tool_calls,
        max_tool_calls: budget.max_tool_calls,
        tool_calls_per_extension: budget.tool_calls_per_extension,
        no_progress_timeout_ms: duration_millis(budget.no_progress_timeout),
        max_identical_actions: budget.max_identical_actions,
        initial_agent_turns: budget.initial_agent_turns,
        max_agent_turns: budget.max_agent_turns,
        agent_turns_per_extension: budget.agent_turns_per_extension,
        max_repair_attempts: budget.max_repair_attempts,
        terminal_model_call_reserve: budget.terminal_model_call_reserve,
        terminal_time_reserve_ms: duration_millis(budget.terminal_time_reserve),
        max_total_tokens: budget.max_total_tokens,
        max_physical_model_attempts: budget.max_physical_model_attempts,
        terminal_token_reserve: budget.terminal_token_reserve,
        terminal_physical_model_attempt_reserve: budget.terminal_physical_model_attempt_reserve,
    }
}

fn campaign_budget(run: &FrozenRunBudget, runs: usize) -> Result<FrozenCampaignBudget, String> {
    let factor = u64::try_from(runs).map_err(|_| "campaign run count is too large")?;
    Ok(FrozenCampaignBudget {
        runs,
        max_duration_ms: run
            .max_duration_ms
            .checked_mul(factor)
            .ok_or_else(|| "campaign duration overflowed".to_string())?,
        max_model_calls: run
            .max_model_calls
            .checked_mul(runs)
            .ok_or_else(|| "campaign model calls overflowed".to_string())?,
        max_tool_calls: run
            .max_tool_calls
            .checked_mul(runs)
            .ok_or_else(|| "campaign tool calls overflowed".to_string())?,
        max_agent_turns: run
            .max_agent_turns
            .checked_mul(runs)
            .ok_or_else(|| "campaign agent turns overflowed".to_string())?,
        max_physical_model_attempts: run
            .max_physical_model_attempts
            .checked_mul(runs)
            .ok_or_else(|| "campaign physical attempts overflowed".to_string())?,
        max_total_tokens: run
            .max_total_tokens
            .checked_mul(factor)
            .ok_or_else(|| "campaign token budget overflowed".to_string())?,
    })
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[path = "collaboration_successor_preflight.rs"]
mod preflight;
pub(super) use preflight::run_preflight;
#[allow(unused_imports)]
pub(super) use preflight::validate_observed_pair;
pub(super) use preflight::{provider_binding, ProviderBindingReceipt};

#[path = "collaboration_successor_execution.rs"]
pub(super) mod execution;

#[path = "collaboration_successor_authorization.rs"]
#[allow(dead_code)]
mod authorization;

#[path = "collaboration_successor_execution_journal.rs"]
#[allow(dead_code)]
mod execution_journal;

#[path = "collaboration_successor_runner.rs"]
mod runner;
pub(super) use runner::{run_authorize, run_execute};

#[cfg(test)]
#[path = "collaboration_successor_protocol_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "collaboration_successor_execution_tests.rs"]
mod execution_tests;
