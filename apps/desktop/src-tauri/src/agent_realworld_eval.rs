use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;
use tauri::Manager;

mod receipts;
mod runtime;
mod setup;
#[cfg(test)]
mod tests;
mod verification;

use receipts::{
    model_receipts_from_metadata, ModelReceipt, ResolvedBudgetReceipt, StrategyReceipt,
};
use runtime::{collect_event_metrics, run_product_task};
use setup::{
    activate_evaluation_data_root, add_recall_session, build_evaluation_app, configure_run_project,
    seed_memory_fixture_for_case, SetupFailure, SetupFailureCode, SetupFailureStage,
};
use verification::{case_input_sha256, direct_prompt, resolved_objective, verify_case};

const SUITE_SCHEMA: &str = "cindx.agent-realworld-suite.v2";
const RAW_SCHEMA: &str = "cindx.agent-realworld-raw.v2";

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
    seed_memory_prompt: Option<String>,
    #[serde(default)]
    index_workspace: bool,
    files: Vec<FixtureFile>,
    permission_policy: PermissionPolicy,
    verification: VerificationContract,
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
    json_files: Vec<JsonFileCheck>,
    #[serde(default)]
    exact_files: Vec<ExactFileCheck>,
    #[serde(default)]
    file_contains: Vec<FileContainsCheck>,
    #[serde(default)]
    commands: Vec<CommandCheck>,
    #[serde(default)]
    required_tools_any: Vec<String>,
    #[serde(default)]
    required_tools_all: Vec<String>,
    #[serde(default)]
    minimum_denied_permissions: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum Treatment {
    Direct,
    Fast,
    Auto,
    Pro,
}

impl Treatment {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "direct" => Ok(Self::Direct),
            "fast" => Ok(Self::Fast),
            "auto" => Ok(Self::Auto),
            "pro" => Ok(Self::Pro),
            other => Err(format!("unsupported treatment {other}")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Fast => "fast",
            Self::Auto => "auto",
            Self::Pro => "pro",
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct RuntimeMetrics {
    latency_ms: u64,
    setup_latency_ms: u64,
    model_calls: usize,
    model_responses: usize,
    tool_calls: usize,
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
    memory_records_after_seed: Option<usize>,
    input_sha256: String,
    output_sha256: String,
    output: String,
    error: Option<String>,
    evidence_error: Option<String>,
    setup_failure: Option<SetupFailure>,
    resolved_budget: ResolvedBudgetReceipt,
    strategy_receipt: Option<StrategyReceipt>,
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
    resolved_budget: Option<ResolvedBudgetReceipt>,
    strategy_receipt: Option<StrategyReceipt>,
    model_receipts: Vec<ModelReceipt>,
    evidence_errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct ExecutionCell {
    execution_index: usize,
    treatment_position: usize,
    plan_sha256: String,
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
        .unwrap_or_else(|| repo_root.join("benchmarks/agent/realworld-v3.json"));
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
                write_raw_report(
                    &output_path,
                    &suite,
                    &suite_bytes,
                    &provider,
                    &git_commit,
                    replicates,
                    &selected_case_names,
                    &treatments,
                    &execution,
                    &runs,
                )?;
                let run = execute_case(
                    &app,
                    &state,
                    &provider,
                    case,
                    *treatment,
                    replicate,
                    &run_root,
                    &evaluation_database,
                    &execution,
                    frozen_profile.as_ref(),
                );
                *runs.last_mut().expect("pending evaluation run") = run;
                write_raw_report(
                    &output_path,
                    &suite,
                    &suite_bytes,
                    &provider,
                    &git_commit,
                    replicates,
                    &selected_case_names,
                    &treatments,
                    &execution,
                    &runs,
                )?;
            }
        }
    }
    eprintln!(
        "[agent-realworld] private raw evidence: {}",
        output_path.display()
    );
    Ok(())
}

fn validate_suite(suite: &RealworldSuite) -> Result<(), String> {
    if suite.schema != SUITE_SCHEMA {
        return Err(format!("unsupported suite schema {}", suite.schema));
    }
    if !(60..=3600).contains(&suite.per_run_timeout_seconds) {
        return Err("suite per-run timeout must be between 60 and 3600 seconds".to_string());
    }
    if suite.id.trim().is_empty() || suite.version == 0 || suite.default_replicates == 0 {
        return Err("suite identity and replicate count must be non-empty".to_string());
    }
    let treatment_set = suite
        .treatments
        .iter()
        .map(|label| Treatment::parse(label).map(|treatment| treatment.label()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if treatment_set != BTreeSet::from(["direct", "fast", "auto", "pro"]) {
        return Err("suite must contain direct, fast, auto, and pro exactly once".to_string());
    }
    if suite.execution_order.protocol != "cyclic_latin_square_v1"
        || suite.execution_order.base_treatments != ["direct", "fast", "auto", "pro"]
    {
        return Err("suite must use the frozen cyclic_latin_square_v1 treatment order".to_string());
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
        for fixture in &case.files {
            validate_relative_path(&fixture.path)?;
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
            .chain(
                case.verification
                    .file_contains
                    .iter()
                    .map(|check| check.path.as_str()),
            )
        {
            validate_relative_path(path)?;
        }
        for command in &case.verification.commands {
            if command.program != "node" || command.args.is_empty() {
                return Err(format!(
                    "{} verifier commands must use direct node argv",
                    case.id
                ));
            }
        }
    }
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
    if !matches!(treatment, Treatment::Auto | Treatment::Pro) {
        return Err("frozen profile artifacts are valid only for Auto or Pro cells".to_string());
    }
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
    if snapshot.effort != treatment.label() {
        return Err(format!(
            "frozen profile effort {} does not match treatment {}",
            snapshot.effort,
            treatment.label()
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

fn execute_case(
    app: &tauri::App<tauri::Wry>,
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    case: &RealworldCase,
    treatment: Treatment,
    replicate: u32,
    root: &Path,
    evaluation_database: &Path,
    execution: &ExecutionCell,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
) -> RawRun {
    let input_sha256 = case_input_sha256(case);
    let started = Instant::now();
    if treatment == Treatment::Direct {
        let prompt = direct_prompt(case, root);
        let control = Arc::new(AgentRunControl::with_budget(RunBudget::for_effort("fast")));
        let completion = complete_collaboration_model_with_control(
            provider.clone(),
            ModelRole::Executor,
            provider.model.clone(),
            "You are the frozen direct baseline. You have no tools and cannot change external state. Answer only from the supplied fixture evidence; never claim that a file or command was executed."
                .to_string(),
            prompt,
            Some(control),
            |_| {},
        );
        let output = completion.content.unwrap_or_default();
        let verification = verify_case(case, treatment, root, &output, &[], 0);
        let model_responses = usize::from(completion.error.is_none());
        let (model_receipts, evidence_error) = if model_responses == 0 {
            (Vec::new(), None)
        } else {
            match model_receipts_from_metadata(&completion.usage) {
                Ok(receipts)
                    if receipts.len() == model_responses
                        && receipts
                            .iter()
                            .all(|receipt| receipt.receipt_status == "observed") =>
                {
                    (receipts, None)
                }
                Ok(receipts) => (
                    receipts,
                    Some("direct provider identity evidence is incomplete".to_string()),
                ),
                Err(error) => (Vec::new(), Some(error)),
            }
        };
        return RawRun {
            execution_index: execution.execution_index,
            treatment_position: execution.treatment_position,
            replicate,
            case_id: case.id.clone(),
            category: case.category.clone(),
            treatment,
            product_mechanism_exercised: false,
            completed: completion.error.is_none() && !output.trim().is_empty(),
            terminal_status: if completion.error.is_none() {
                "completed".to_string()
            } else {
                "failed".to_string()
            },
            configured_models: vec![provider.model.clone()],
            tools_used: Vec::new(),
            memory_records_after_seed: None,
            input_sha256,
            output_sha256: sha256_hex(output.as_bytes()),
            output,
            error: completion.error,
            evidence_error,
            setup_failure: None,
            resolved_budget: ResolvedBudgetReceipt::for_treatment(treatment),
            strategy_receipt: None,
            model_receipts,
            metrics: RuntimeMetrics {
                latency_ms: completion.latency_ms,
                model_calls: 1,
                model_responses,
                prompt_tokens: metadata_u64(&completion.usage, "prompt_tokens"),
                completion_tokens: metadata_u64(&completion.usage, "completion_tokens"),
                total_tokens: metadata_u64(&completion.usage, "total_tokens"),
                resident_kib_after: process_resident_kib(),
                workspace_bytes_after: directory_size(root),
                ..RuntimeMetrics::default()
            },
            verification,
        };
    }

    let key = format!("r{replicate}-{}-{}", case.id, treatment.label());
    let (project_id, mut session_id) = match configure_run_project(state, root, &key) {
        Ok(value) => value,
        Err(error) => {
            return failed_run(
                case,
                treatment,
                replicate,
                input_sha256,
                error,
                started,
                SetupFailure::new(
                    SetupFailureStage::ProjectConfiguration,
                    SetupFailureCode::Configuration,
                    false,
                ),
                0,
                execution,
            )
        }
    };
    let objective = resolved_objective(case, root);
    let mut setup_latency_ms = 0_u64;
    let mut memory_records_after_seed = None;
    if case.index_workspace {
        let setup_started = Instant::now();
        let index_result = index_workspace_rag_blocking(
            app.handle(),
            RagOperationInput {
                operation_id: unique_id("realworld-index"),
            },
        );
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(setup_started));
        if let Err(error) = index_result {
            let setup_failure = if error == RAG_INDEX_CANCELLED {
                SetupFailure::new(
                    SetupFailureStage::WorkspaceIndex,
                    SetupFailureCode::Transient,
                    true,
                )
            } else {
                SetupFailure::new(
                    SetupFailureStage::WorkspaceIndex,
                    SetupFailureCode::Index,
                    false,
                )
            };
            return failed_run(
                case,
                treatment,
                replicate,
                input_sha256,
                error,
                started,
                setup_failure,
                setup_latency_ms,
                execution,
            );
        }
    }
    if let Some(seed_prompt) = case.seed_memory_prompt.as_deref() {
        let setup_started = Instant::now();
        let seed_result = seed_memory_fixture_for_case(
            state,
            evaluation_database,
            &project_id,
            &session_id,
            seed_prompt,
        );
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(setup_started));
        memory_records_after_seed = match seed_result {
            Ok(record_count) => Some(record_count),
            Err((setup_failure, error)) => {
                return failed_run(
                    case,
                    treatment,
                    replicate,
                    input_sha256,
                    error,
                    started,
                    setup_failure,
                    setup_latency_ms,
                    execution,
                )
            }
        };
        let session_started = Instant::now();
        session_id = match add_recall_session(state, &project_id, &key) {
            Ok(session_id) => session_id,
            Err(error) => {
                setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(session_started));
                return failed_run(
                    case,
                    treatment,
                    replicate,
                    input_sha256,
                    error,
                    started,
                    SetupFailure::new(
                        SetupFailureStage::RecallSession,
                        SetupFailureCode::Configuration,
                        false,
                    ),
                    setup_latency_ms,
                    execution,
                );
            }
        };
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(session_started));
    }

    let product = run_product_task(
        app.handle(),
        state,
        &session_id,
        &objective,
        treatment,
        case.permission_policy,
    );
    let output = product.state.latest_answer.clone().unwrap_or_default();
    let mut event_metrics =
        match collect_event_metrics(state, &session_id, treatment, frozen_profile) {
            Ok(metrics) => metrics,
            Err(error) => EventMetrics {
                evidence_errors: vec![error],
                ..EventMetrics::default()
            },
        };
    let tools = std::mem::take(&mut event_metrics.tools)
        .into_iter()
        .collect::<Vec<_>>();
    let denied_permissions = product
        .denied_permissions
        .max(event_metrics.denied_permissions);
    let verification = verify_case(case, treatment, root, &output, &tools, denied_permissions);
    let evidence_error = (!event_metrics.evidence_errors.is_empty())
        .then(|| event_metrics.evidence_errors.join(" | "));
    RawRun {
        execution_index: execution.execution_index,
        treatment_position: execution.treatment_position,
        replicate,
        case_id: case.id.clone(),
        category: case.category.clone(),
        treatment,
        product_mechanism_exercised: true,
        completed: product.state.status == "completed",
        terminal_status: product.state.status.clone(),
        configured_models: configured_models(provider).into_values().collect(),
        tools_used: tools,
        memory_records_after_seed,
        input_sha256,
        output_sha256: sha256_hex(output.as_bytes()),
        output,
        error: product.error.or(product.state.last_error.clone()),
        evidence_error,
        setup_failure: None,
        resolved_budget: event_metrics
            .resolved_budget
            .unwrap_or_else(|| ResolvedBudgetReceipt::for_treatment(treatment)),
        strategy_receipt: event_metrics.strategy_receipt,
        model_receipts: event_metrics.model_receipts,
        metrics: RuntimeMetrics {
            latency_ms: elapsed_ms(started).saturating_sub(setup_latency_ms),
            setup_latency_ms,
            model_calls: event_metrics.model_calls,
            model_responses: event_metrics.model_responses,
            tool_calls: event_metrics.tool_calls,
            permission_requests: product
                .permission_requests
                .max(event_metrics.permission_requests),
            denied_permissions,
            recovery_events: event_metrics.recovery_events,
            prompt_tokens: event_metrics.prompt_tokens,
            completion_tokens: event_metrics.completion_tokens,
            total_tokens: event_metrics.total_tokens,
            context_tokens_used: product.state.context_tokens_used,
            resident_kib_after: process_resident_kib(),
            workspace_bytes_after: directory_size(root),
        },
        verification,
    }
}

fn interrupted_run(
    case: &RealworldCase,
    treatment: Treatment,
    replicate: u32,
    direct_model: String,
    execution: &ExecutionCell,
) -> RawRun {
    let product_mechanism_exercised = treatment != Treatment::Direct;
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
        memory_records_after_seed: None,
        input_sha256: case_input_sha256(case),
        output_sha256: sha256_hex(&[]),
        output: String::new(),
        error: Some("evaluation process exited before verification".to_string()),
        evidence_error: Some("run did not reach provider evidence collection".to_string()),
        setup_failure: None,
        resolved_budget: ResolvedBudgetReceipt::for_treatment(treatment),
        strategy_receipt: None,
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
    input_sha256: String,
    error: String,
    started: Instant,
    setup_failure: SetupFailure,
    setup_latency_ms: u64,
    execution: &ExecutionCell,
) -> RawRun {
    RawRun {
        execution_index: execution.execution_index,
        treatment_position: execution.treatment_position,
        replicate,
        case_id: case.id.clone(),
        category: case.category.clone(),
        treatment,
        product_mechanism_exercised: treatment != Treatment::Direct,
        completed: false,
        terminal_status: "infrastructure_failed".to_string(),
        configured_models: Vec::new(),
        tools_used: Vec::new(),
        memory_records_after_seed: None,
        input_sha256,
        output_sha256: sha256_hex(&[]),
        output: String::new(),
        error: Some(error),
        evidence_error: None,
        setup_failure: Some(setup_failure),
        resolved_budget: ResolvedBudgetReceipt::for_treatment(treatment),
        strategy_receipt: None,
        model_receipts: Vec::new(),
        metrics: RuntimeMetrics {
            latency_ms: elapsed_ms(started),
            setup_latency_ms,
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
    suite: &RealworldSuite,
    suite_bytes: &[u8],
    provider: &ProviderConfig,
    git_commit: &str,
    replicates: u32,
    selected_cases: &[String],
    treatments: &[Treatment],
    execution: &ExecutionCell,
    runs: &[RawRun],
) -> Result<(), String> {
    let report = RawReport {
        schema: RAW_SCHEMA,
        suite_id: &suite.id,
        suite_version: suite.version,
        suite_description: &suite.description,
        suite_sha256: sha256_hex(suite_bytes),
        execution_order_protocol: &suite.execution_order.protocol,
        execution_plan_sha256: &execution.plan_sha256,
        generated_at_ms: current_time_millis(),
        git_commit: git_commit.to_string(),
        app_version: env!("CARGO_PKG_VERSION"),
        provider_id: provider.provider_id.clone(),
        provider_endpoint: provider.base_url.clone(),
        configured_models: configured_models(provider),
        requested_replicates: replicates,
        selected_cases: selected_cases.to_vec(),
        selected_treatments: treatments.to_vec(),
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
