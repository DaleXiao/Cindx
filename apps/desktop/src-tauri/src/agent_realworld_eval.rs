use crate::session_output_cache_store::SessionOutputCache;
use crate::suspended_run_runtime::SuspendedRunStore;
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::Manager;

const SUITE_SCHEMA: &str = "cindx.agent-realworld-suite.v1";
const RAW_SCHEMA: &str = "cindx.agent-realworld-raw.v1";
const MAX_DRIVER_ROUNDS: usize = 24;

#[derive(Debug, Deserialize)]
struct RealworldSuite {
    schema: String,
    id: String,
    version: u32,
    description: String,
    default_replicates: u32,
    per_run_timeout_seconds: u64,
    treatments: Vec<String>,
    cases: Vec<RealworldCase>,
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
    tool_calls: usize,
    permission_requests: usize,
    denied_permissions: usize,
    recovery_events: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    tools: BTreeSet<String>,
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
        .unwrap_or_else(|| repo_root.join("benchmarks/agent/realworld-v1.json"));
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
    let bootstrap_root = suite_root.join("bootstrap");
    fs::create_dir_all(&bootstrap_root)
        .map_err(|error| format!("failed to create bootstrap workspace: {error}"))?;
    let sidecars = SidecarConfig::default();
    apply_sidecar_env(&sidecars);
    let app = build_evaluation_app(provider.clone(), sidecars, &bootstrap_root)?;
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
                    &runs,
                )?;
                let run = execute_case(
                    &app, &state, &provider, case, *treatment, replicate, &run_root,
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

fn install_eval_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

fn build_evaluation_app(
    provider: ProviderConfig,
    sidecars: SidecarConfig,
    root: &Path,
) -> Result<tauri::App<tauri::Wry>, String> {
    let project_sessions = isolated_project_config(root, "bootstrap");
    let state = AppState {
        store: Mutex::new(SqliteStore::in_memory().map_err(|error| error.to_string())?),
        attachment_upload_batches: Mutex::new(AttachmentUploadBatches::default()),
        manual_tool_execution_gate: Mutex::new(()),
        provider_config: Mutex::new(provider),
        provider_config_update: Mutex::new(()),
        workspace_config: Mutex::new(WorkspaceConfig {
            root: root.to_path_buf(),
        }),
        sidecar_config: Mutex::new(sidecars),
        web_search_config: Mutex::new(WebSearchConfig::default()),
        project_session_config: Mutex::new(project_sessions),
        session_lifecycle_gate: Mutex::new(()),
        schedule_config: Mutex::new(crate::schedule::ScheduleConfig::default()),
        schedule_last_error: Mutex::new(None),
        mcp_catalog: Mutex::new(McpCatalogService::load(
            root.join(".eval-mcp.json"),
            root.join(".eval-mcp-cache.json"),
        )),
        suspended_agent_runs: SuspendedRunStore::default(),
        session_output_cache: SessionOutputCache::default(),
        agent_run_controls: agent_harness::RunRegistry::new("agent realworld control"),
        prompt_evaluation_controls: agent_harness::RunRegistry::new(
            "agent realworld prompt evaluation control",
        ),
        rag_operation_controls: Mutex::new(BTreeMap::new()),
        queue_dispatching_sessions: agent_harness::ExclusiveKeyRegistry::new(
            "agent realworld queue",
        ),
        session_title_refinement_sessions: agent_harness::ExclusiveKeyRegistry::new(
            "agent realworld title",
        ),
        workspace_knowledge_cache: Mutex::new(BTreeMap::new()),
        tool_registry_cache: Mutex::new(ToolRegistryCache::default()),
        conductor_health: Mutex::new(
            crate::conductor_health_runtime::ConductorHealthLedger::default(),
        ),
        tool_registry_generation: AtomicU64::new(0),
        allow_exit: AtomicBool::new(false),
        quit_prompt_active: AtomicBool::new(false),
    };
    tauri::Builder::default()
        .manage(state)
        .build(crate::app_bootstrap::application_context())
        .map_err(|error| format!("failed to build headless evaluation app: {error}"))
}

fn isolated_project_config(root: &Path, key: &str) -> ProjectSessionConfig {
    let mut config = ProjectSessionConfig::default_for_root(root);
    let project_id = format!("project-realworld-{key}");
    let session_id = format!("session-realworld-{key}");
    config.active_project_id = project_id.clone();
    config.active_session_id = session_id.clone();
    config.projects[0].id = project_id.clone();
    config.projects[0].name = format!("Realworld {key}");
    config.projects[0].root = root.display().to_string();
    config.sessions[0].id = session_id;
    config.sessions[0].project_id = project_id;
    config.sessions[0].name = format!("Realworld {key}");
    config
}

fn configure_run_project(
    state: &tauri::State<'_, AppState>,
    root: &Path,
    key: &str,
) -> Result<(String, String), String> {
    let config = isolated_project_config(root, key);
    let project_id = config.active_project_id.clone();
    let session_id = config.active_session_id.clone();
    *state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))? = WorkspaceConfig {
        root: root.to_path_buf(),
    };
    *state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session lock poisoned: {error}"))? = config;
    state
        .workspace_knowledge_cache
        .lock()
        .map_err(|error| format!("knowledge cache lock poisoned: {error}"))?
        .clear();
    state
        .tool_registry_cache
        .lock()
        .map_err(|error| format!("tool cache lock poisoned: {error}"))?
        .clear();
    state
        .tool_registry_generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok((project_id, session_id))
}

fn add_recall_session(
    state: &tauri::State<'_, AppState>,
    project_id: &str,
    key: &str,
) -> Result<String, String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session lock poisoned: {error}"))?;
    let mut session = config
        .sessions
        .first()
        .cloned()
        .ok_or_else(|| "evaluation project has no seed session".to_string())?;
    let session_id = format!("session-realworld-{key}-recall");
    session.id = session_id.clone();
    session.project_id = project_id.to_string();
    session.name = "Memory recall".to_string();
    session.title_state = SessionTitleState::Pending;
    session.seen_event_sequence = 0;
    session.created_at_ms = current_time_millis();
    session.updated_at_ms = session.created_at_ms;
    session.archived_at_ms = None;
    config.sessions.push(session);
    config.active_session_id = session_id.clone();
    Ok(session_id)
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
        return RawRun {
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
            metrics: RuntimeMetrics {
                latency_ms: completion.latency_ms,
                model_calls: 1,
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
        Err(error) => return failed_run(case, treatment, replicate, input_sha256, error, started),
    };
    let objective = resolved_objective(case, root);
    let mut setup_latency_ms = 0_u64;
    let mut memory_records_after_seed = None;
    if case.index_workspace {
        let setup_started = Instant::now();
        if let Err(error) = index_workspace_rag_blocking(
            app.handle(),
            RagOperationInput {
                operation_id: unique_id("realworld-index"),
            },
        ) {
            return failed_run(case, treatment, replicate, input_sha256, error, started);
        }
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(setup_started));
    }
    if let Some(seed_prompt) = case.seed_memory_prompt.as_deref() {
        let setup_started = Instant::now();
        let seed = run_product_task(
            app.handle(),
            state,
            &session_id,
            seed_prompt,
            treatment,
            PermissionPolicy::AllowOnce,
        );
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(setup_started));
        if seed.state.status != "completed" {
            return failed_run(
                case,
                treatment,
                replicate,
                input_sha256,
                seed.error
                    .unwrap_or_else(|| format!("memory seed ended as {}", seed.state.status)),
                started,
            );
        }
        memory_records_after_seed = state
            .store
            .lock()
            .ok()
            .and_then(|mut store| load_project_memory_ledger(&mut store, &project_id).ok())
            .map(|ledger| ledger.records.len());
        session_id = match add_recall_session(state, &project_id, &key) {
            Ok(session_id) => session_id,
            Err(error) => {
                return failed_run(case, treatment, replicate, input_sha256, error, started)
            }
        };
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
    let event_metrics = collect_event_metrics(state, &session_id).unwrap_or_default();
    let tools = event_metrics.tools.into_iter().collect::<Vec<_>>();
    let denied_permissions = product
        .denied_permissions
        .max(event_metrics.denied_permissions);
    let verification = verify_case(case, treatment, root, &output, &tools, denied_permissions);
    RawRun {
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
        metrics: RuntimeMetrics {
            latency_ms: elapsed_ms(started).saturating_sub(setup_latency_ms),
            setup_latency_ms,
            model_calls: event_metrics.model_calls,
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
) -> RawRun {
    let product_mechanism_exercised = treatment != Treatment::Direct;
    RawRun {
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
) -> RawRun {
    RawRun {
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
        metrics: RuntimeMetrics {
            latency_ms: elapsed_ms(started),
            resident_kib_after: process_resident_kib(),
            ..RuntimeMetrics::default()
        },
        verification: VerificationResult {
            failures: vec!["run did not reach verification".to_string()],
            ..VerificationResult::default()
        },
    }
}

fn run_product_task(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    session_id: &str,
    prompt: &str,
    treatment: Treatment,
    permission_policy: PermissionPolicy,
) -> ProductRun {
    let effort = treatment.label();
    let initial = (|| -> Result<AgentState, String> {
        let lease = begin_agent_run_control_for_effort(state, session_id, effort, None)?;
        let control = lease.control();
        let result = run_agent_task_blocking_inner(
            app,
            state.clone(),
            AgentTaskInput {
                prompt: prompt.to_string(),
                session_id: session_id.to_string(),
                current_time: chrono::Utc::now().to_rfc3339(),
                queue_id: None,
                effort: effort.to_string(),
                attachments: Vec::new(),
            },
            &control,
        );
        drop(lease);
        result
    })();
    let mut current = match initial {
        Ok(state) => state,
        Err(error) => {
            return ProductRun {
                state: empty_agent_state(session_id, &error),
                permission_requests: 0,
                denied_permissions: 0,
                error: Some(error),
            }
        }
    };
    let mut permission_requests = 0usize;
    let mut denied_permissions = 0usize;
    let mut error = None;
    for _ in 0..MAX_DRIVER_ROUNDS {
        if let Some(approval) = current.pending_approvals.first() {
            permission_requests += 1;
            let decision = match permission_policy {
                PermissionPolicy::AllowOnce => "allow_once",
                PermissionPolicy::DenyMutations => {
                    denied_permissions += 1;
                    "deny"
                }
            };
            match resolve_agent_permission_blocking(
                app,
                state.clone(),
                approval.request_id.clone(),
                decision.to_string(),
                session_id.to_string(),
            ) {
                Ok(next) => current = next,
                Err(resolve_error) => {
                    error = Some(resolve_error);
                    break;
                }
            }
            continue;
        }
        if current.status == "paused" && current.can_continue {
            match retry_agent_task_blocking(
                app,
                state.clone(),
                SessionActionInput {
                    session_id: session_id.to_string(),
                },
            ) {
                Ok(next) => current = next,
                Err(resume_error) => {
                    error = Some(resume_error);
                    break;
                }
            }
            continue;
        }
        break;
    }
    if (!current.pending_approvals.is_empty()
        || (current.status == "paused" && current.can_continue))
        && error.is_none()
    {
        error = Some(format!(
            "evaluation driver exceeded {MAX_DRIVER_ROUNDS} permission or continuation rounds"
        ));
    }
    ProductRun {
        state: current,
        permission_requests,
        denied_permissions,
        error,
    }
}

fn empty_agent_state(session_id: &str, error: &str) -> AgentState {
    AgentState {
        task_id: String::new(),
        project_id: None,
        project_name: None,
        session_id: Some(session_id.to_string()),
        session_name: None,
        status: "failed".to_string(),
        turn_count: 0,
        max_turns: 0,
        transcript_messages: 0,
        context_tokens_used: 0,
        context_window_tokens: 0,
        context_remaining_percent: 0.0,
        context_usage_estimated: true,
        run_started_at_ms: 0,
        run_budget_ms: 0,
        run_model_call_budget: 0,
        run_tool_call_budget: 0,
        can_cancel: false,
        can_retry: false,
        can_continue: false,
        event_count: 0,
        latest_sequence: 0,
        oldest_sequence: 0,
        has_older_history: false,
        timeline: Vec::new(),
        messages: Vec::new(),
        pending_approvals: Vec::new(),
        queued_messages: Vec::new(),
        latest_answer: None,
        last_error: Some(error.to_string()),
    }
}

fn collect_event_metrics(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<EventMetrics, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let events = agent_events_for_session(&store, &phase16_task_id(), Some(session_id))
        .map_err(|error| error.to_string())?;
    let mut metrics = EventMetrics::default();
    for event in events {
        match event.kind {
            EventKind::ModelRequestStarted => metrics.model_calls += 1,
            EventKind::ModelRequestFinished => {
                metrics.prompt_tokens = metrics
                    .prompt_tokens
                    .saturating_add(metadata_u64(&event.metadata, "prompt_tokens"));
                metrics.completion_tokens = metrics
                    .completion_tokens
                    .saturating_add(metadata_u64(&event.metadata, "completion_tokens"));
                metrics.total_tokens = metrics
                    .total_tokens
                    .saturating_add(metadata_u64(&event.metadata, "total_tokens"));
            }
            EventKind::ToolCallStarted => {
                metrics.tool_calls += 1;
                if let Some(tool) = event.metadata.get("tool") {
                    metrics.tools.insert(tool.clone());
                }
            }
            EventKind::PermissionRequested => metrics.permission_requests += 1,
            EventKind::PermissionResolved => {
                if event.metadata.get("decision").map(String::as_str) == Some("deny") {
                    metrics.denied_permissions += 1;
                }
            }
            _ => {}
        }
        if event.metadata.contains_key("recovery_state")
            || event.metadata.contains_key("recovery_resume_key")
        {
            metrics.recovery_events += 1;
        }
    }
    if metrics.total_tokens == 0 {
        metrics.total_tokens = metrics
            .prompt_tokens
            .saturating_add(metrics.completion_tokens);
    }
    Ok(metrics)
}

fn direct_prompt(case: &RealworldCase, root: &Path) -> String {
    let evidence = case
        .files
        .iter()
        .map(|fixture| format!("[{}]\n{}", fixture.path, fixture.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    let seed = case
        .seed_memory_prompt
        .as_deref()
        .map(|value| format!("\nPrior project statement:\n{value}\n"))
        .unwrap_or_default();
    format!(
        "{}{}\nFrozen fixture evidence from {}:\n{}\nReturn the information or exact proposed changes needed to satisfy the objective.",
        resolved_objective(case, root),
        seed,
        root.display(),
        evidence
    )
}

fn resolved_objective(case: &RealworldCase, root: &Path) -> String {
    let browser_path = root.join("site/index.html");
    let browser_url = format!("file://{}", browser_path.display());
    case.objective
        .replace("{{WORKSPACE}}", &root.display().to_string())
        .replace("{{BROWSER_URL}}", &browser_url)
}

fn case_input_sha256(case: &RealworldCase) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(case.id.as_bytes());
    bytes.extend_from_slice(case.objective.as_bytes());
    if let Some(seed) = case.seed_memory_prompt.as_deref() {
        bytes.extend_from_slice(seed.as_bytes());
    }
    for fixture in &case.files {
        bytes.extend_from_slice(fixture.path.as_bytes());
        bytes.extend_from_slice(fixture.content.as_bytes());
    }
    sha256_hex(&bytes)
}

fn verify_case(
    case: &RealworldCase,
    treatment: Treatment,
    root: &Path,
    output: &str,
    tools: &[String],
    denied_permissions: usize,
) -> VerificationResult {
    let mut result = VerificationResult::default();
    let output_lower = output.to_lowercase();
    let answer_values = if treatment == Treatment::Direct {
        &case.verification.direct_output_contains
    } else {
        &case.verification.output_contains
    };
    for value in answer_values {
        record_check(
            &mut result,
            output_lower.contains(&value.to_lowercase()),
            format!("output is missing {value:?}"),
        );
    }
    result.answer_passed = result.failures.is_empty();
    if treatment == Treatment::Direct {
        result.external_effect_passed = None;
        result.quality_passed = result.answer_passed;
        return result;
    }

    let effect_failure_start = result.failures.len();
    for check in &case.verification.json_files {
        let path = root.join(&check.path);
        let actual = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        record_check(
            &mut result,
            actual.as_ref() == Some(&check.equals),
            format!("{} does not match the frozen JSON contract", check.path),
        );
    }
    for check in &case.verification.exact_files {
        let actual = fs::read_to_string(root.join(&check.path)).ok();
        record_check(
            &mut result,
            actual.as_deref() == Some(check.content.as_str()),
            format!("{} changed or is missing", check.path),
        );
    }
    for check in &case.verification.file_contains {
        let actual = fs::read_to_string(root.join(&check.path)).unwrap_or_default();
        for value in &check.values {
            record_check(
                &mut result,
                actual.contains(value),
                format!("{} is missing {value:?}", check.path),
            );
        }
    }
    for check in &case.verification.commands {
        let command_result = Command::new(&check.program)
            .args(&check.args)
            .current_dir(root)
            .output();
        let passed = command_result.as_ref().is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(&check.stdout_contains)
        });
        record_check(
            &mut result,
            passed,
            format!("verifier command {} {:?} failed", check.program, check.args),
        );
    }
    let tool_set = tools.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if !case.verification.required_tools_any.is_empty() {
        record_check(
            &mut result,
            case.verification
                .required_tools_any
                .iter()
                .any(|tool| tool_set.contains(tool.as_str())),
            format!(
                "none of the required evidence tools ran: {:?}",
                case.verification.required_tools_any
            ),
        );
    }
    for tool in &case.verification.required_tools_all {
        record_check(
            &mut result,
            tool_set.contains(tool.as_str()),
            format!("required tool {tool} did not run"),
        );
    }
    record_check(
        &mut result,
        denied_permissions >= case.verification.minimum_denied_permissions,
        format!(
            "expected at least {} denied permissions but observed {denied_permissions}",
            case.verification.minimum_denied_permissions
        ),
    );
    let effect_passed = result.failures.len() == effect_failure_start;
    result.external_effect_passed = Some(effect_passed);
    result.safety_violations =
        if matches!(case.permission_policy, PermissionPolicy::DenyMutations) && !effect_passed {
            1
        } else {
            0
        };
    result.quality_passed = result.answer_passed && effect_passed;
    result
}

fn record_check(result: &mut VerificationResult, passed: bool, failure: String) {
    result.total_checks += 1;
    if passed {
        result.passed_checks += 1;
    } else {
        result.failures.push(failure);
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
    runs: &[RawRun],
) -> Result<(), String> {
    let report = RawReport {
        schema: RAW_SCHEMA,
        suite_id: &suite.id,
        suite_version: suite.version,
        suite_description: &suite.description,
        suite_sha256: sha256_hex(suite_bytes),
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

#[cfg(test)]
mod tests {
    use super::{directory_size, selected_replicates};
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn workspace_size_does_not_follow_external_symlinks() {
        use std::os::unix::fs::symlink;

        let external = tempfile::tempdir().expect("external tempdir");
        fs::write(external.path().join("large.bin"), vec![0_u8; 64 * 1024])
            .expect("external fixture");
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        fs::write(workspace.path().join("local.txt"), b"local").expect("local fixture");
        symlink(external.path(), workspace.path().join("shared-runtime")).expect("runtime symlink");

        assert_eq!(directory_size(workspace.path()), 5);
    }

    #[test]
    fn replicate_selection_is_bounded() {
        std::env::set_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX", "2");
        assert_eq!(selected_replicates(3).expect("selection"), vec![2]);
        std::env::set_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX", "4");
        assert!(selected_replicates(3).is_err());
        std::env::remove_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX");
    }
}
