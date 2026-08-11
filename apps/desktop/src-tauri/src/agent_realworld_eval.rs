use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tauri::Manager;

mod collaboration_learning_capture;
#[allow(dead_code)]
mod collaboration_learning_journal;
mod collaboration_successor_protocol;
mod conductor_ownership_suite;
#[allow(dead_code)]
mod delivery_verification;
mod delivery_verification_authorization;
mod delivery_verification_campaign;
mod delivery_verification_execution_journal;
#[cfg(test)]
mod delivery_verification_execution_tests;
mod delivery_verification_preflight;
mod delivery_verification_protocol;
#[allow(dead_code)]
mod delivery_verification_requests;
mod delivery_verification_runner;
#[cfg(test)]
mod delivery_verification_tests;
mod direct_finalizer;
mod direct_finalizer_campaign;
mod direct_finalizer_campaign_contract;
mod direct_finalizer_campaign_evidence;
mod direct_finalizer_campaign_execution;
mod direct_finalizer_campaign_support;
#[path = "direct_finalizer_evaluation_feedback.rs"]
mod direct_finalizer_evaluation_feedback;
mod direct_finalizer_receipts;
#[cfg(test)]
mod direct_finalizer_receipts_tests;
#[cfg(test)]
mod direct_finalizer_tests;
mod execution;
mod http_fixture;
mod memory_receipts;
mod outcome_shadow;
mod receipts;
mod runtime;
mod setup;
#[cfg(test)]
mod tests;
mod tool_receipts;
mod treatments;
mod verification;
mod workflow_gepa_campaign;
mod workflow_gepa_campaign_contract;
mod workflow_gepa_campaign_evidence;
mod workflow_gepa_campaign_execution;
mod workflow_gepa_campaign_journal;
mod workflow_gepa_campaign_suite;
mod workflow_gepa_candidate_probe;
mod workflow_gepa_candidate_search;

use direct_finalizer_receipts::DirectFinalizerExecutionReceipt;
use execution::{execute_case, CaseExecutionInput};
use http_fixture::HttpFixtureReceipt;
use memory_receipts::{
    validate_memory_effect_suite, MemoryEffectCaseContract, MemoryEvaluationReceipt,
};
use outcome_shadow::ShadowOutcomeTraceV1;
pub(crate) use receipts::{model_receipts_from_metadata, ModelReceipt};
use receipts::{ResolvedBudgetReceipt, StrategyReceipt};
use setup::{activate_evaluation_data_root, build_evaluation_app, SetupFailure};
use tool_receipts::ToolAttemptReceipt;
use treatments::{expected_treatments, raw_schema, Treatment, MEMORY_EFFECT_SUITE_SCHEMA};
use verification::{case_input_sha256, PostconditionReceipt};

#[derive(Debug, Deserialize)]
struct RealworldSuite {
    schema: String,
    id: String,
    version: u32,
    description: String,
    default_replicates: u32,
    per_run_timeout_seconds: u64,
    treatments: Vec<String>,
    execution_order: ExecutionOrderContract,
    cases: Vec<RealworldCase>,
}

#[derive(Debug, Deserialize)]
struct ExecutionOrderContract {
    protocol: String,
    base_treatments: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RealworldCase {
    id: String,
    category: String,
    objective: String,
    #[serde(default)]
    campaign_split: Option<String>,
    #[serde(default)]
    expected_execution_mode: Option<String>,
    #[serde(default)]
    seed_memory_prompt: Option<String>,
    #[serde(default)]
    index_workspace: bool,
    files: Vec<FixtureFile>,
    permission_policy: PermissionPolicy,
    verification: VerificationContract,
    #[serde(default)]
    memory_effect: Option<MemoryEffectCaseContract>,
}

#[derive(Debug, Deserialize)]
struct FixtureFile {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PermissionPolicy {
    AllowOnce,
    DenyMutations,
}

#[derive(Debug, Default, Deserialize)]
struct VerificationContract {
    #[serde(default)]
    direct_output_contains: Vec<String>,
    #[serde(default)]
    output_contains: Vec<String>,
    #[serde(default)]
    output_not_contains: Vec<String>,
    #[serde(default)]
    json_files: Vec<JsonFileCheck>,
    #[serde(default)]
    exact_files: Vec<ExactFileCheck>,
    #[serde(default)]
    immutable_files: Vec<String>,
    #[serde(default)]
    file_contains: Vec<FileContainsCheck>,
    #[serde(default)]
    commands: Vec<CommandCheck>,
    #[serde(default)]
    required_tools_any: Vec<String>,
    #[serde(default)]
    required_tools_all: Vec<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    browser_target_receipt: Option<BrowserTargetReceiptContract>,
    #[serde(default)]
    minimum_denied_permissions: usize,
}

#[derive(Debug, Default, Deserialize)]
struct BrowserTargetReceiptContract {
    #[serde(default)]
    tools_all: Vec<String>,
    #[serde(default)]
    tools_any: Vec<String>,
    minimum_artifacts: usize,
}

#[derive(Debug, Deserialize)]
struct JsonFileCheck {
    path: String,
    equals: Value,
}

#[derive(Debug, Deserialize)]
struct ExactFileCheck {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct FileContainsCheck {
    path: String,
    values: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CommandCheck {
    program: String,
    args: Vec<String>,
    stdout_contains: String,
}

#[derive(Debug, Default, Serialize)]
struct RuntimeMetrics {
    latency_ms: u64,
    setup_latency_ms: u64,
    model_calls: usize,
    model_responses: usize,
    tool_calls: usize,
    tool_succeeded: usize,
    tool_failed: usize,
    tool_cancelled: usize,
    tool_denied: usize,
    tool_incomplete: usize,
    tool_superseded: usize,
    tool_invalid: usize,
    permission_requests: usize,
    denied_permissions: usize,
    recovery_events: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    context_tokens_used: u64,
    resident_kib_after: u64,
    workspace_bytes_after: u64,
}

#[derive(Debug, Default, Serialize)]
struct VerificationResult {
    quality_passed: bool,
    answer_passed: bool,
    external_effect_passed: Option<bool>,
    passed_checks: usize,
    total_checks: usize,
    safety_violations: usize,
    failures: Vec<String>,
    expected_browser_target_sha256: Option<String>,
    postcondition_receipts: Vec<PostconditionReceipt>,
}

#[derive(Debug, Serialize)]
struct RawRun {
    execution_index: usize,
    treatment_position: usize,
    replicate: u32,
    case_id: String,
    category: String,
    treatment: Treatment,
    product_mechanism_exercised: bool,
    completed: bool,
    terminal_status: String,
    configured_models: Vec<String>,
    tools_used: Vec<String>,
    fixture_receipt: Option<HttpFixtureReceipt>,
    tool_receipts: Vec<ToolAttemptReceipt>,
    memory_records_after_seed: Option<usize>,
    memory_seed_sha256: Option<String>,
    input_sha256: String,
    output_sha256: String,
    output: String,
    error: Option<String>,
    evidence_error: Option<String>,
    #[serde(skip)]
    outcome_trace: Option<ShadowOutcomeTraceV1>,
    #[serde(skip)]
    outcome_trace_error: Option<String>,
    #[serde(skip)]
    collaboration_learning_events: Vec<Event>,
    setup_failure: Option<SetupFailure>,
    resolved_budget: ResolvedBudgetReceipt,
    strategy_receipt: Option<StrategyReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_finalizer_execution: Option<DirectFinalizerExecutionReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_finalizer_evidence_error: Option<String>,
    memory_evaluation_receipt: Option<MemoryEvaluationReceipt>,
    model_receipts: Vec<ModelReceipt>,
    metrics: RuntimeMetrics,
    verification: VerificationResult,
}

#[derive(Debug, Serialize)]
struct RawReport<'a> {
    schema: &'static str,
    suite_id: &'a str,
    suite_version: u32,
    suite_description: &'a str,
    suite_sha256: String,
    execution_order_protocol: &'a str,
    execution_plan_sha256: &'a str,
    generated_at_ms: u64,
    git_commit: String,
    app_version: &'static str,
    provider_id: String,
    provider_endpoint: String,
    configured_models: BTreeMap<String, String>,
    requested_replicates: u32,
    selected_cases: Vec<String>,
    selected_treatments: Vec<Treatment>,
    runs: &'a [RawRun],
}

#[derive(Debug, Default)]
struct EventMetrics {
    model_calls: usize,
    model_responses: usize,
    tool_calls: usize,
    permission_requests: usize,
    denied_permissions: usize,
    recovery_events: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    tools: BTreeSet<String>,
    tool_receipts: Vec<ToolAttemptReceipt>,
    tool_succeeded: usize,
    tool_failed: usize,
    tool_cancelled: usize,
    tool_denied: usize,
    tool_incomplete: usize,
    tool_superseded: usize,
    tool_invalid: usize,
    resolved_budget: Option<ResolvedBudgetReceipt>,
    strategy_receipt: Option<StrategyReceipt>,
    direct_finalizer_execution: Option<DirectFinalizerExecutionReceipt>,
    direct_finalizer_evidence_error: Option<String>,
    memory_evaluation_receipt: Option<MemoryEvaluationReceipt>,
    model_receipts: Vec<ModelReceipt>,
    evidence_errors: Vec<String>,
    outcome_trace: Option<ShadowOutcomeTraceV1>,
    outcome_trace_error: Option<String>,
    collaboration_learning_events: Vec<Event>,
}

#[derive(Debug, Clone)]
struct ExecutionCell {
    execution_index: usize,
    treatment_position: usize,
    plan_sha256: String,
}

struct FailedRunDetails {
    input_sha256: String,
    error: String,
    started: Instant,
    setup_failure: SetupFailure,
    setup_latency_ms: u64,
}

struct RawReportContext<'a> {
    suite: &'a RealworldSuite,
    suite_bytes: &'a [u8],
    provider: &'a ProviderConfig,
    git_commit: &'a str,
    replicates: u32,
    selected_cases: &'a [String],
    treatments: &'a [Treatment],
    execution: &'a ExecutionCell,
}

struct ProductRun {
    state: AgentState,
    permission_requests: usize,
    denied_permissions: usize,
    error: Option<String>,
}

pub fn run_agent_realworld_eval() -> Result<(), String> {
    install_eval_crypto_provider();
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))?;
    let suite_path = std::env::var_os("CINDX_AGENT_REALWORLD_SUITE")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("benchmarks/agent/realworld-v5.json"));
    let suite_bytes = fs::read(&suite_path)
        .map_err(|error| format!("failed to read {}: {error}", suite_path.display()))?;
    let suite: RealworldSuite = serde_json::from_slice(&suite_bytes)
        .map_err(|error| format!("invalid real-world suite JSON: {error}"))?;
    validate_suite(&suite)?;

    let output_path = PathBuf::from(
        std::env::var_os("CINDX_AGENT_REALWORLD_OUTPUT")
            .ok_or_else(|| "CINDX_AGENT_REALWORLD_OUTPUT is required".to_string())?,
    );
    reject_repo_output_path(&repo_root, &output_path)?;
    let git_commit = required_git_commit()?;
    let replicates = std::env::var("CINDX_AGENT_REALWORLD_REPLICATES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(suite.default_replicates);
    if replicates == 0 {
        return Err("at least one replicate is required".to_string());
    }
    let replicate_indices = selected_replicates(replicates)?;
    let selected_case_ids = selection("CINDX_AGENT_REALWORLD_CASES");
    let selected_treatment_labels = selection("CINDX_AGENT_REALWORLD_TREATMENTS");
    let selected_cases = suite
        .cases
        .iter()
        .filter(|case| {
            selected_case_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&case.id))
        })
        .collect::<Vec<_>>();
    if selected_cases.is_empty() {
        return Err("case selection is empty".to_string());
    }
    let treatments = suite
        .treatments
        .iter()
        .filter(|label| {
            selected_treatment_labels
                .as_ref()
                .is_none_or(|labels| labels.contains(label.as_str()))
        })
        .map(|label| Treatment::parse(label))
        .collect::<Result<Vec<_>, _>>()?;
    if treatments.is_empty() {
        return Err("treatment selection is empty".to_string());
    }
    if replicate_indices.len() != 1 || selected_cases.len() != 1 || treatments.len() != 1 {
        return Err(
            "real-world evidence v2 executes exactly one frozen matrix cell per process"
                .to_string(),
        );
    }
    let execution = required_execution_cell(
        &suite,
        replicate_indices[0],
        selected_cases[0],
        treatments[0],
    )?;
    let frozen_profile = load_evaluation_frozen_profile(&repo_root, treatments[0])?;

    let provider = load_provider_config();
    if !provider.is_ready() {
        return Err(
            "configured provider is required; evaluation will not synthesize results".to_string(),
        );
    }
    let managed_temp = if std::env::var_os("CINDX_AGENT_REALWORLD_TEMP_ROOT").is_none() {
        Some(
            tempfile::Builder::new()
                .prefix("cindx-agent-realworld-")
                .tempdir()
                .map_err(|error| format!("failed to create evaluation workspace: {error}"))?,
        )
    } else {
        None
    };
    let suite_root = match std::env::var_os("CINDX_AGENT_REALWORLD_TEMP_ROOT") {
        Some(value) => {
            let root = PathBuf::from(value);
            reject_repo_output_path(&repo_root, &root)?;
            fs::create_dir_all(&root)
                .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
            root
        }
        None => managed_temp
            .as_ref()
            .expect("managed evaluation tempdir")
            .path()
            .to_path_buf(),
    };
    let evaluation_database = activate_evaluation_data_root(&suite_root)?;
    let bootstrap_root = suite_root.join("bootstrap");
    fs::create_dir_all(&bootstrap_root)
        .map_err(|error| format!("failed to create bootstrap workspace: {error}"))?;
    let sidecars = SidecarConfig::default();
    apply_sidecar_env(&sidecars);
    let app = build_evaluation_app(
        provider.clone(),
        sidecars,
        &bootstrap_root,
        &evaluation_database,
    )?;
    let state = app.state::<AppState>();
    let mut runs = Vec::new();
    let selected_case_names = selected_cases
        .iter()
        .map(|case| case.id.clone())
        .collect::<Vec<_>>();
    let report_context = RawReportContext {
        suite: &suite,
        suite_bytes: &suite_bytes,
        provider: &provider,
        git_commit: &git_commit,
        replicates,
        selected_cases: &selected_case_names,
        treatments: &treatments,
        execution: &execution,
    };

    for replicate in replicate_indices {
        for case in &selected_cases {
            for treatment in &treatments {
                eprintln!(
                    "[agent-realworld] replicate={replicate}/{replicates} case={} treatment={}",
                    case.id,
                    treatment.label()
                );
                let run_root =
                    suite_root.join(format!("r{replicate}-{}-{}", case.id, treatment.label()));
                materialize_case(&run_root, case)?;
                runs.push(interrupted_run(
                    case,
                    *treatment,
                    replicate,
                    provider.model.clone(),
                    &execution,
                ));
                write_raw_report(&output_path, &report_context, &runs)?;
                let run = execute_case(
                    &app,
                    &state,
                    &provider,
                    &evaluation_database,
                    CaseExecutionInput {
                        case,
                        treatment: *treatment,
                        replicate,
                        root: &run_root,
                        execution: &execution,
                        frozen_profile: frozen_profile.as_ref(),
                        project_scope: None,
                        run_budget: None,
                        execution_constraint: None,
                        matched_route_plan_anchor: None,
                        collaboration_learning_policy: None,
                    },
                );
                *runs.last_mut().expect("pending evaluation run") = run;
                write_raw_report(&output_path, &report_context, &runs)?;
            }
        }
    }
    eprintln!(
        "[agent-realworld] private raw evidence: {}",
        output_path.display()
    );
    Ok(())
}

pub fn run_direct_finalizer_gepa_eval() -> Result<(), String> {
    direct_finalizer_campaign::run()
}

pub fn run_workflow_gepa_eval() -> Result<(), String> {
    workflow_gepa_campaign::run()
}

pub fn run_workflow_gepa_candidate_probe() -> Result<(), String> {
    workflow_gepa_candidate_probe::run()
}

pub fn run_collaboration_successor_preflight() -> Result<(), String> {
    collaboration_successor_protocol::run_preflight()
}

pub fn run_collaboration_successor_authorize() -> Result<(), String> {
    collaboration_successor_protocol::run_authorize()
}

pub fn run_collaboration_successor_execute() -> Result<(), String> {
    collaboration_successor_protocol::run_execute()
}

pub fn run_delivery_verification_preflight() -> Result<(), String> {
    delivery_verification_preflight::run_preflight()
}

pub fn run_delivery_verification_authorize() -> Result<(), String> {
    delivery_verification_runner::run_authorize()
}

pub fn run_delivery_verification_execute() -> Result<(), String> {
    delivery_verification_runner::run_execute()
}

fn validate_suite(suite: &RealworldSuite) -> Result<(), String> {
    let expected = expected_treatments(&suite.schema)
        .ok_or_else(|| format!("unsupported suite schema {}", suite.schema))?;
    if !(60..=3600).contains(&suite.per_run_timeout_seconds) {
        return Err("suite per-run timeout must be between 60 and 3600 seconds".to_string());
    }
    if suite.id.trim().is_empty() || suite.version == 0 || suite.default_replicates == 0 {
        return Err("suite identity and replicate count must be non-empty".to_string());
    }
    let treatments = suite
        .treatments
        .iter()
        .map(|label| Treatment::parse(label).map(|treatment| treatment.label()))
        .collect::<Result<Vec<_>, _>>()?;
    if suite.execution_order.protocol != "cyclic_latin_square_v1"
        || treatments != expected
        || suite.execution_order.base_treatments != expected
    {
        return Err(format!(
            "suite must use the frozen cyclic_latin_square_v1 treatment order for {}",
            suite.schema
        ));
    }
    let mut ids = BTreeSet::new();
    let mut categories = BTreeSet::new();
    for case in &suite.cases {
        if case.id.trim().is_empty() || !ids.insert(case.id.as_str()) {
            return Err(format!("empty or duplicate case id {}", case.id));
        }
        if case.objective.trim().is_empty() || case.files.is_empty() {
            return Err(format!("{} must have an objective and fixtures", case.id));
        }
        categories.insert(case.category.as_str());
        let mut fixture_paths = BTreeSet::new();
        for fixture in &case.files {
            validate_relative_path(&fixture.path)?;
            if !fixture_paths.insert(fixture.path.as_str()) {
                return Err(format!(
                    "{} has duplicate fixture path {}",
                    case.id, fixture.path
                ));
            }
        }
        for path in case
            .verification
            .json_files
            .iter()
            .map(|check| check.path.as_str())
            .chain(
                case.verification
                    .exact_files
                    .iter()
                    .map(|check| check.path.as_str()),
            )
            .chain(case.verification.immutable_files.iter().map(String::as_str))
            .chain(
                case.verification
                    .file_contains
                    .iter()
                    .map(|check| check.path.as_str()),
            )
        {
            validate_relative_path(path)?;
        }
        for path in &case.verification.immutable_files {
            if !fixture_paths.contains(path.as_str()) {
                return Err(format!(
                    "{} immutable file {} is not a declared fixture",
                    case.id, path
                ));
            }
        }
        for command in &case.verification.commands {
            if command.program != "node" || command.args.is_empty() {
                return Err(format!(
                    "{} verifier commands must use direct node argv",
                    case.id
                ));
            }
        }
        let browser_fixture = case.objective.contains("{{BROWSER_URL}}");
        match (
            browser_fixture,
            case.verification.browser_target_receipt.as_ref(),
        ) {
            (true, Some(contract)) => {
                if !case
                    .files
                    .iter()
                    .any(|fixture| fixture.path == "site/index.html")
                {
                    return Err(format!(
                        "{} browser fixture must provide site/index.html",
                        case.id
                    ));
                }
                if contract.tools_all.is_empty()
                    || contract.tools_any.is_empty()
                    || contract.minimum_artifacts == 0
                    || contract
                        .tools_all
                        .iter()
                        .chain(&contract.tools_any)
                        .any(|tool| !tool.starts_with("browser."))
                {
                    return Err(format!(
                        "{} browser receipt contract is incomplete",
                        case.id
                    ));
                }
            }
            (true, None) => {
                return Err(format!(
                    "{} browser fixture is missing its target receipt contract",
                    case.id
                ))
            }
            (false, Some(_)) => {
                return Err(format!(
                    "{} declares a browser receipt without a browser fixture",
                    case.id
                ))
            }
            (false, None) => {}
        }
    }
    if conductor_ownership_suite::is_conductor_ownership_suite(suite) {
        conductor_ownership_suite::validate_conductor_ownership_suite(suite)?;
    } else if suite.schema == MEMORY_EFFECT_SUITE_SCHEMA {
        validate_memory_effect_suite(suite)?;
    } else {
        let required = BTreeSet::from([
            "coding",
            "browser",
            "file",
            "long_horizon",
            "rag_memory",
            "permission_safety",
        ]);
        if !required.is_subset(&categories) {
            return Err("suite is missing a required real-world category".to_string());
        }
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("unsafe fixture path {value}"));
    }
    Ok(())
}

fn reject_repo_output_path(repo_root: &Path, output_path: &Path) -> Result<(), String> {
    let absolute = if output_path.is_absolute() {
        output_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to resolve output path: {error}"))?
            .join(output_path)
    };
    if absolute.starts_with(repo_root) {
        return Err("raw evaluation evidence must remain outside the Git checkout".to_string());
    }
    Ok(())
}

fn required_git_commit() -> Result<String, String> {
    let commit = std::env::var("CINDX_EVAL_GIT_COMMIT")
        .map_err(|_| "CINDX_EVAL_GIT_COMMIT is required".to_string())?;
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("CINDX_EVAL_GIT_COMMIT must be a full 40-character SHA".to_string());
    }
    Ok(commit)
}

fn selection(name: &str) -> Option<BTreeSet<String>> {
    let values = std::env::var(name)
        .ok()?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    (!values.is_empty()).then_some(values)
}

fn selected_replicates(replicates: u32) -> Result<Vec<u32>, String> {
    let Some(value) = std::env::var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX").ok() else {
        return Ok((1..=replicates).collect());
    };
    let index = value
        .parse::<u32>()
        .map_err(|_| "CINDX_AGENT_REALWORLD_REPLICATE_INDEX must be an integer".to_string())?;
    if !(1..=replicates).contains(&index) {
        return Err(format!(
            "CINDX_AGENT_REALWORLD_REPLICATE_INDEX must be between 1 and {replicates}"
        ));
    }
    Ok(vec![index])
}

fn required_execution_cell(
    suite: &RealworldSuite,
    replicate: u32,
    case: &RealworldCase,
    treatment: Treatment,
) -> Result<ExecutionCell, String> {
    let case_index = suite
        .cases
        .iter()
        .position(|candidate| candidate.id == case.id)
        .ok_or_else(|| format!("case {} is not in the frozen suite", case.id))?;
    let block_index = (replicate.saturating_sub(1) as usize)
        .saturating_mul(suite.cases.len())
        .saturating_add(case_index);
    let rotation = block_index % suite.execution_order.base_treatments.len();
    let treatment_position = (0..suite.execution_order.base_treatments.len())
        .find(|position| {
            let index = (position + rotation) % suite.execution_order.base_treatments.len();
            suite.execution_order.base_treatments[index] == treatment.label()
        })
        .map(|position| position + 1)
        .ok_or_else(|| "treatment is missing from the frozen execution order".to_string())?;
    let execution_index = block_index
        .saturating_mul(suite.execution_order.base_treatments.len())
        .saturating_add(treatment_position);
    let observed_index = required_env_usize("CINDX_AGENT_REALWORLD_EXECUTION_INDEX")?;
    let observed_position = required_env_usize("CINDX_AGENT_REALWORLD_TREATMENT_POSITION")?;
    if observed_index != execution_index || observed_position != treatment_position {
        return Err(format!(
            "execution cell receipt drifted: expected index {execution_index} position {treatment_position}, observed index {observed_index} position {observed_position}"
        ));
    }
    let plan_sha256 = std::env::var("CINDX_AGENT_REALWORLD_PLAN_SHA256")
        .map_err(|_| "CINDX_AGENT_REALWORLD_PLAN_SHA256 is required".to_string())?;
    if !is_lower_sha256(&plan_sha256) {
        return Err("CINDX_AGENT_REALWORLD_PLAN_SHA256 must be a lowercase SHA-256".to_string());
    }
    Ok(ExecutionCell {
        execution_index,
        treatment_position,
        plan_sha256,
    })
}

fn load_evaluation_frozen_profile(
    repo_root: &Path,
    treatment: Treatment,
) -> Result<Option<FrozenPromptProfileSnapshot>, String> {
    let Some(path) = std::env::var_os("CINDX_AGENT_REALWORLD_PROFILE_PATH").map(PathBuf::from)
    else {
        return Ok(None);
    };
    let Some(effort) = treatment.profile_effort() else {
        return Err("frozen profile artifacts are valid only for Auto or Pro cells".to_string());
    };
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "failed to resolve frozen profile {}: {error}",
            path.display()
        )
    })?;
    if canonical.starts_with(repo_root) {
        return Err("frozen profile artifacts must remain outside the Git checkout".to_string());
    }
    let encoded = fs::read(&canonical).map_err(|error| {
        format!(
            "failed to read frozen profile {}: {error}",
            canonical.display()
        )
    })?;
    let snapshot = FrozenPromptProfileSnapshot::from_json_slice(&encoded)?;
    if snapshot.effort != effort {
        return Err(format!(
            "frozen profile effort {} does not match treatment {}",
            snapshot.effort, effort
        ));
    }
    let artifact_sha256 = snapshot.artifact_sha256()?;
    if let Ok(expected) = std::env::var("CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256") {
        if expected != artifact_sha256 {
            return Err(
                "frozen profile artifact digest does not match runner preflight".to_string(),
            );
        }
    }
    Ok(Some(snapshot))
}

fn required_env_usize(name: &str) -> Result<usize, String> {
    std::env::var(name)
        .map_err(|_| format!("{name} is required"))?
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{name} must be a positive integer"))
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn install_eval_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

fn materialize_case(root: &Path, case: &RealworldCase) -> Result<(), String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
    for fixture in &case.files {
        validate_relative_path(&fixture.path)?;
        let path = root.join(&fixture.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(&path, &fixture.content)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    Ok(())
}

fn interrupted_run(
    case: &RealworldCase,
    treatment: Treatment,
    replicate: u32,
    direct_model: String,
    execution: &ExecutionCell,
) -> RawRun {
    let product_mechanism_exercised = !treatment.is_oracle_reference();
    RawRun {
        execution_index: execution.execution_index,
        treatment_position: execution.treatment_position,
        replicate,
        case_id: case.id.clone(),
        category: case.category.clone(),
        treatment,
        product_mechanism_exercised,
        completed: false,
        terminal_status: "running".to_string(),
        configured_models: if product_mechanism_exercised {
            Vec::new()
        } else {
            vec![direct_model]
        },
        tools_used: Vec::new(),
        fixture_receipt: None,
        tool_receipts: Vec::new(),
        memory_records_after_seed: None,
        memory_seed_sha256: None,
        input_sha256: case_input_sha256(case),
        output_sha256: sha256_hex(&[]),
        output: String::new(),
        error: Some("evaluation process exited before verification".to_string()),
        evidence_error: Some("run did not reach provider evidence collection".to_string()),
        outcome_trace: None,
        outcome_trace_error: Some("run did not reach outcome trace collection".to_string()),
        collaboration_learning_events: Vec::new(),
        setup_failure: None,
        resolved_budget: ResolvedBudgetReceipt::for_treatment(treatment),
        strategy_receipt: None,
        direct_finalizer_execution: None,
        direct_finalizer_evidence_error: None,
        memory_evaluation_receipt: None,
        model_receipts: Vec::new(),
        metrics: RuntimeMetrics::default(),
        verification: VerificationResult {
            external_effect_passed: product_mechanism_exercised.then_some(false),
            failures: vec!["run did not reach verification".to_string()],
            ..VerificationResult::default()
        },
    }
}

fn failed_run(
    case: &RealworldCase,
    treatment: Treatment,
    replicate: u32,
    execution: &ExecutionCell,
    details: FailedRunDetails,
) -> RawRun {
    RawRun {
        execution_index: execution.execution_index,
        treatment_position: execution.treatment_position,
        replicate,
        case_id: case.id.clone(),
        category: case.category.clone(),
        treatment,
        product_mechanism_exercised: !treatment.is_oracle_reference(),
        completed: false,
        terminal_status: "infrastructure_failed".to_string(),
        configured_models: Vec::new(),
        tools_used: Vec::new(),
        fixture_receipt: None,
        tool_receipts: Vec::new(),
        memory_records_after_seed: None,
        memory_seed_sha256: None,
        input_sha256: details.input_sha256,
        output_sha256: sha256_hex(&[]),
        output: String::new(),
        error: Some(details.error),
        evidence_error: None,
        outcome_trace: None,
        outcome_trace_error: Some("run did not reach outcome trace collection".to_string()),
        collaboration_learning_events: Vec::new(),
        setup_failure: Some(details.setup_failure),
        resolved_budget: ResolvedBudgetReceipt::for_treatment(treatment),
        strategy_receipt: None,
        direct_finalizer_execution: None,
        direct_finalizer_evidence_error: None,
        memory_evaluation_receipt: None,
        model_receipts: Vec::new(),
        metrics: RuntimeMetrics {
            latency_ms: elapsed_ms(details.started),
            setup_latency_ms: details.setup_latency_ms,
            resident_kib_after: process_resident_kib(),
            ..RuntimeMetrics::default()
        },
        verification: VerificationResult {
            failures: vec!["run did not reach verification".to_string()],
            ..VerificationResult::default()
        },
    }
}

fn write_raw_report(
    output_path: &Path,
    context: &RawReportContext<'_>,
    runs: &[RawRun],
) -> Result<(), String> {
    let report = RawReport {
        schema: raw_schema(&context.suite.schema).expect("validated suite schema"),
        suite_id: &context.suite.id,
        suite_version: context.suite.version,
        suite_description: &context.suite.description,
        suite_sha256: sha256_hex(context.suite_bytes),
        execution_order_protocol: &context.suite.execution_order.protocol,
        execution_plan_sha256: &context.execution.plan_sha256,
        generated_at_ms: current_time_millis(),
        git_commit: context.git_commit.to_string(),
        app_version: env!("CARGO_PKG_VERSION"),
        provider_id: context.provider.provider_id.clone(),
        provider_endpoint: context.provider.base_url.clone(),
        configured_models: configured_models(context.provider),
        requested_replicates: context.replicates,
        selected_cases: context.selected_cases.to_vec(),
        selected_treatments: context.treatments.to_vec(),
        runs,
    };
    let encoded = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("failed to encode raw report: {error}"))?;
    write_private_file_atomically(output_path, &encoded, "Agent real-world raw report")
        .map_err(|error| error.to_string())
}

fn configured_models(config: &ProviderConfig) -> BTreeMap<String, String> {
    [
        ("default", config.model.as_str()),
        ("conductor", config.conductor_model.as_str()),
        ("planner", config.planner_model.as_str()),
        ("executor", config.executor_model.as_str()),
        ("reviewer", config.reviewer_model.as_str()),
        ("summarizer", config.summarizer_model.as_str()),
        ("embedding", config.embedding_model.as_str()),
    ]
    .into_iter()
    .map(|(role, model)| (role.to_string(), model.to_string()))
    .collect()
}

fn metadata_u64(metadata: &Metadata, key: &str) -> u64 {
    metadata
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn process_resident_kib() -> u64 {
    Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_default()
}

fn directory_size(root: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                return 0;
            };
            if file_type.is_symlink() {
                0
            } else if file_type.is_dir() {
                directory_size(&path)
            } else {
                entry
                    .metadata()
                    .map(|metadata| metadata.len())
                    .unwrap_or_default()
            }
        })
        .sum()
}
