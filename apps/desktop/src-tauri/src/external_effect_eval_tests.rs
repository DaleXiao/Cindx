use super::*;
use orchestrator::{AdaptiveWorkflow, AdaptiveWorkflowStep, WorkflowOutputKind};

const EXTERNAL_EFFECT_SCHEMA: &str = "cindx.external_effect_eval.raw.v3";
const GPQA_SOURCE_URL: &str = "https://github.com/idavidrein/gpqa";
const GPQA_SOURCE_REVISION: &str = "56686c06f5e19865c153de0fdb11be3890014df7";
const GPQA_SOURCE_FILE_SHA256: &str =
    "41d1213cd7a4998605a26c2798500652572007161b3a92817ba46b35befcd305";
const GPQA_BASELINE_CASES_PER_DOMAIN: usize = 4;
const GPQA_BASELINE_CASE_COUNT: usize = 12;
const GPQA_BASELINE_MANIFEST_SHA256: &str =
    "203d665f53e6ecdd1e07fb325792bd4a48c6ac954a4a144b3114f26a4efea02d";
const GPQA_BASELINE_DIRECT_PROFILE: &str = "gpqa-direct-baseline-v1";
const GPQA_BASELINE_DIRECT_PROFILE_SHA256: &str =
    "e5ec8624aeaeb02021e6e6db4cc00ab97af1793af3a6e051094a1583fe2d6443";
const GPQA_BASELINE_AUTO_PROFILE_SHA256: &str =
    "be58315c193ef1544b1bea4bc0cfd8c666f27f82447699cb5c5f975a67bdd68a";
const GPQA_BASELINE_PRO_PROFILE_SHA256: &str =
    "f6f7df16306550cfc54dbbad3ed834df9d4253e7e7c7bfb8bb303fd4c7db7697";
const MRCR_SOURCE_URL: &str = "https://huggingface.co/datasets/openai/mrcr";
const MRCR_DATASET_REVISION: &str = "2025-12-05-bugfix";
const EVALUATION_MODEL_CALL_TIMEOUT_SECONDS: u64 = 180;
const EVALUATION_TREATMENT_DEADLINE_SECONDS: u64 = 300;
const EVALUATION_TERMINAL_RESERVE_SECONDS: u64 = EVALUATION_MODEL_CALL_TIMEOUT_SECONDS;
const EVALUATION_MAX_OUTPUT_TOKENS: u64 = COLLABORATION_MAX_OUTPUT_TOKENS;
const EVALUATION_FINALIZER_RECOVERY_SECONDS: u64 = 60;

fn evaluation_run_control(effort: &str) -> Arc<AgentRunControl> {
    let mut budget = RunBudget::for_effort(effort);
    budget.max_duration = Duration::from_secs(EVALUATION_TREATMENT_DEADLINE_SECONDS);
    budget.model_call_timeout = Duration::from_secs(EVALUATION_MODEL_CALL_TIMEOUT_SECONDS);
    budget.terminal_time_reserve = Duration::from_secs(EVALUATION_TERMINAL_RESERVE_SECONDS);
    Arc::new(AgentRunControl::with_budget(budget))
}

#[derive(Debug, Deserialize)]
struct GpqaRow {
    #[serde(rename = "Question")]
    question: String,
    #[serde(rename = "Correct Answer")]
    correct_answer: String,
    #[serde(rename = "Incorrect Answer 1")]
    incorrect_answer_1: String,
    #[serde(rename = "Incorrect Answer 2")]
    incorrect_answer_2: String,
    #[serde(rename = "Incorrect Answer 3")]
    incorrect_answer_3: String,
    #[serde(rename = "Record ID")]
    record_id: String,
    #[serde(rename = "High-level domain")]
    domain: String,
}

#[derive(Debug, Clone)]
struct GpqaCase {
    case_id: String,
    domain: String,
    prompt: String,
    expected: char,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GpqaTreatment {
    Direct,
    Auto,
    Pro,
}

impl GpqaTreatment {
    fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct_default",
            Self::Auto => "cindx_auto",
            Self::Pro => "cindx_pro",
        }
    }
}

#[derive(Debug, Deserialize)]
struct MrcrPage {
    rows: Vec<MrcrPageRow>,
}

#[derive(Debug, Deserialize)]
struct MrcrPageRow {
    row_idx: usize,
    row: MrcrRow,
}

#[derive(Debug, Deserialize)]
struct MrcrRow {
    prompt: String,
    answer: String,
    random_string_to_prepend: String,
    n_needles: usize,
    total_messages: usize,
    n_chars: usize,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct EvalSource {
    benchmark: String,
    source_url: String,
    revision: String,
    file_sha256: String,
    sample_count: usize,
    protocol: String,
}

#[derive(Debug, Serialize)]
struct ExternalEffectRun {
    benchmark: String,
    case_id: String,
    category: String,
    treatment: String,
    treatment_position: usize,
    requested_policy: String,
    effective_policy: String,
    models: Vec<String>,
    prompt_profile: String,
    prompt_profile_origin: String,
    prompt_profile_sha256: String,
    prompt_profile_artifact_sha256: Option<String>,
    gepa_frozen: bool,
    succeeded: bool,
    latency_ms: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    first_token_latency_ms: Option<u64>,
    input_sha256: String,
    expected_sha256: String,
    output_sha256: String,
    parsed_answer: Option<String>,
    exact_score: Option<f64>,
    prefix_valid: Option<bool>,
    n_chars: Option<usize>,
    n_needles: Option<usize>,
    total_messages: Option<usize>,
    expected: String,
    output: String,
    error: Option<String>,
    diagnostics: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct ExternalEffectReport<'a> {
    schema: &'static str,
    generated_at_ms: u64,
    git_commit: String,
    app_version: &'static str,
    provider_endpoint: &'a str,
    configured_models: BTreeMap<String, String>,
    evaluation_limits: BTreeMap<String, u64>,
    sources: &'a [EvalSource],
    runs: &'a [ExternalEffectRun],
}

#[derive(Debug)]
struct TreatmentOutput {
    policy: String,
    models: Vec<String>,
    prompt_profile: String,
    prompt_profile_origin: String,
    prompt_profile_sha256: String,
    prompt_profile_artifact_sha256: Option<String>,
    gepa_frozen: bool,
    succeeded: bool,
    latency_ms: u64,
    usage: Metadata,
    output: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct EvaluationPromptProfile {
    genome: ConductorPromptGenome,
    origin: &'static str,
    genome_sha256: String,
    artifact_sha256: Option<String>,
    gepa_frozen: bool,
}

impl EvaluationPromptProfile {
    fn seed(effort: &str) -> Self {
        let genome = ConductorPromptGenome::seed_for_effort(effort);
        let genome_sha256 = orchestrator::prompt_genome_sha256(&genome)
            .expect("built-in prompt genome should validate and serialize");
        Self {
            genome,
            origin: "built_in_seed",
            genome_sha256,
            artifact_sha256: None,
            gepa_frozen: false,
        }
    }

    fn from_frozen_snapshot(
        effort: &str,
        snapshot: orchestrator::FrozenPromptProfileSnapshot,
    ) -> Result<Self, String> {
        if snapshot.effort != effort {
            return Err(format!(
                "frozen prompt profile effort {} does not match requested {effort}",
                snapshot.effort
            ));
        }
        let artifact_sha256 = snapshot.artifact_sha256()?;
        Ok(Self {
            genome: snapshot.genome,
            origin: "frozen_gepa_snapshot",
            genome_sha256: snapshot.candidate_sha256,
            artifact_sha256: Some(artifact_sha256),
            gepa_frozen: true,
        })
    }
}

fn evaluation_prompt_profile(effort: &str) -> Result<EvaluationPromptProfile, String> {
    let env_name = match effort {
        "auto" => Some("CINDX_EVAL_FROZEN_GEPA_AUTO_PATH"),
        "pro" => Some("CINDX_EVAL_FROZEN_GEPA_PRO_PATH"),
        "fast" => None,
        other => return Err(format!("unsupported evaluation effort: {other}")),
    };
    let Some(env_name) = env_name else {
        return Ok(EvaluationPromptProfile::seed(effort));
    };
    let Some(path) = std::env::var_os(env_name) else {
        return Ok(EvaluationPromptProfile::seed(effort));
    };
    let path = PathBuf::from(path);
    let encoded = fs::read(&path)
        .map_err(|error| format!("failed to read {} from {env_name}: {error}", path.display()))?;
    let snapshot = orchestrator::FrozenPromptProfileSnapshot::from_json_slice(&encoded)
        .map_err(|error| format!("invalid frozen prompt profile {}: {error}", path.display()))?;
    EvaluationPromptProfile::from_frozen_snapshot(effort, snapshot)
}

fn validate_gpqa_baseline_profiles(
    auto: &EvaluationPromptProfile,
    pro: &EvaluationPromptProfile,
) -> Result<(), String> {
    if fixed_protocol_sha256(GPQA_BASELINE_DIRECT_PROFILE) != GPQA_BASELINE_DIRECT_PROFILE_SHA256
        || auto.origin != "built_in_seed"
        || auto.genome.id != "seed-auto-v1"
        || auto.genome_sha256 != GPQA_BASELINE_AUTO_PROFILE_SHA256
        || auto.artifact_sha256.is_some()
        || auto.gepa_frozen
        || pro.origin != "built_in_seed"
        || pro.genome.id != "seed-pro-v1"
        || pro.genome_sha256 != GPQA_BASELINE_PRO_PROFILE_SHA256
        || pro.artifact_sha256.is_some()
        || pro.gepa_frozen
    {
        return Err("provider baseline prompt profiles have drifted".to_string());
    }
    Ok(())
}

fn fixed_protocol_sha256(profile: &str) -> String {
    sha256_hex(profile.as_bytes())
}

fn deterministic_rank(seed: &str, value: &str) -> String {
    sha256_hex(format!("{seed}\u{1f}{value}").as_bytes())
}

fn gpqa_case_from_row(row: GpqaRow, seed: &str) -> GpqaCase {
    let mut options = [
        (row.correct_answer.trim().to_string(), true),
        (row.incorrect_answer_1.trim().to_string(), false),
        (row.incorrect_answer_2.trim().to_string(), false),
        (row.incorrect_answer_3.trim().to_string(), false),
    ];
    options.sort_by_key(|(answer, _)| {
        deterministic_rank(seed, &format!("{}\u{1f}{answer}", row.record_id))
    });
    let expected_index = options
        .iter()
        .position(|(_, correct)| *correct)
        .expect("GPQA row should contain one correct answer");
    let expected = (b'A' + expected_index as u8) as char;
    let choices = options
        .iter()
        .enumerate()
        .map(|(index, (answer, _))| format!("({}) {answer}", (b'A' + index as u8) as char))
        .collect::<Vec<_>>()
        .join("\n");
    GpqaCase {
        case_id: row.record_id,
        domain: row.domain,
        prompt: format!(
            "What is the correct answer to this question?\n{}\n\n{}\n\nFormat your response as follows: \"The correct answer is (insert answer here)\"",
            row.question.trim(),
            choices,
        ),
        expected,
    }
}

fn load_gpqa_cases(path: &Path, per_domain: usize) -> Result<Vec<GpqaCase>, String> {
    let mut reader = csv::Reader::from_path(path)
        .map_err(|error| format!("failed to open GPQA CSV: {error}"))?;
    let mut by_domain = BTreeMap::<String, Vec<GpqaCase>>::new();
    for row in reader.deserialize::<GpqaRow>() {
        let row = row.map_err(|error| format!("failed to parse GPQA row: {error}"))?;
        let case = gpqa_case_from_row(row, "cindx-fugu-effect-pilot-v1");
        by_domain.entry(case.domain.clone()).or_default().push(case);
    }
    let mut selected = Vec::new();
    for domain in ["Biology", "Chemistry", "Physics"] {
        let cases = by_domain
            .get_mut(domain)
            .ok_or_else(|| format!("GPQA domain {domain} is absent"))?;
        cases.sort_by_key(|case| deterministic_rank("gpqa-sample-v1", &case.case_id));
        if cases.len() < per_domain {
            return Err(format!(
                "GPQA domain {domain} has only {} rows, requested {per_domain}",
                cases.len()
            ));
        }
        selected.extend(cases.iter().take(per_domain).cloned());
    }
    Ok(selected)
}

fn gpqa_treatment_order(case_index: usize) -> [GpqaTreatment; 3] {
    match case_index % 3 {
        0 => [
            GpqaTreatment::Direct,
            GpqaTreatment::Auto,
            GpqaTreatment::Pro,
        ],
        1 => [
            GpqaTreatment::Auto,
            GpqaTreatment::Pro,
            GpqaTreatment::Direct,
        ],
        _ => [
            GpqaTreatment::Pro,
            GpqaTreatment::Direct,
            GpqaTreatment::Auto,
        ],
    }
}

fn validate_gpqa_baseline_cases(cases: &[GpqaCase]) -> Result<(), String> {
    if cases.len() != GPQA_BASELINE_CASE_COUNT {
        return Err(format!(
            "provider baseline requires exactly {GPQA_BASELINE_CASE_COUNT} GPQA cases, got {}",
            cases.len()
        ));
    }
    let manifest = cases
        .iter()
        .map(|case| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                case.case_id,
                case.domain,
                sha256_hex(case.prompt.as_bytes()),
                sha256_hex(case.expected.to_string().as_bytes())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if sha256_hex(manifest.as_bytes()) != GPQA_BASELINE_MANIFEST_SHA256 {
        return Err(
            "provider baseline GPQA cases do not match the pinned ordered manifest".to_string(),
        );
    }
    Ok(())
}

fn parse_gpqa_answer(output: &str) -> Option<char> {
    fn letter_from_fragment(fragment: &str) -> Option<char> {
        let trimmed = fragment
            .trim()
            .trim_matches(['.', ',', ':', ';', '"', '\'']);
        if trimmed.len() == 1 {
            return trimmed
                .chars()
                .next()
                .map(|value| value.to_ascii_uppercase())
                .filter(|value| ('A'..='D').contains(value));
        }
        if trimmed.len() == 3 && trimmed.starts_with('(') && trimmed.ends_with(')') {
            return trimmed
                .chars()
                .nth(1)
                .map(|value| value.to_ascii_uppercase())
                .filter(|value| ('A'..='D').contains(value));
        }
        None
    }

    for line in output.lines().rev() {
        let upper = line.trim().to_ascii_uppercase();
        if let Some(letter) = letter_from_fragment(&upper) {
            return Some(letter);
        }
        for marker in [
            "THE CORRECT ANSWER IS",
            "CORRECT ANSWER IS",
            "FINAL ANSWER:",
            "ANSWER IS",
            "ANSWER:",
        ] {
            if let Some((_, suffix)) = upper.rsplit_once(marker) {
                if let Some(letter) = suffix.split_whitespace().find_map(letter_from_fragment) {
                    return Some(letter);
                }
            }
        }
    }
    None
}

fn configured_models(config: &ProviderConfig) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("default".to_string(), config.model.clone()),
        ("conductor".to_string(), config.model_for_conductor()),
        (
            "planner".to_string(),
            config.model_for_role(&ModelRole::Planner),
        ),
        (
            "executor".to_string(),
            config.model_for_role(&ModelRole::Executor),
        ),
        (
            "reviewer".to_string(),
            config.model_for_role(&ModelRole::Reviewer),
        ),
        (
            "summarizer".to_string(),
            config.model_for_role(&ModelRole::Summarizer),
        ),
    ])
}

fn usage_value(usage: &Metadata, key: &str) -> u64 {
    usage
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

fn treatment_from_completion(
    policy: impl Into<String>,
    models: Vec<String>,
    prompt_profile: &str,
    completion: CollaborationCompletion,
) -> TreatmentOutput {
    let succeeded = completion.error.is_none()
        && completion
            .content
            .as_ref()
            .is_some_and(|content| !content.trim().is_empty());
    TreatmentOutput {
        policy: policy.into(),
        models,
        prompt_profile: prompt_profile.to_string(),
        prompt_profile_origin: "fixed_protocol".to_string(),
        prompt_profile_sha256: fixed_protocol_sha256(prompt_profile),
        prompt_profile_artifact_sha256: None,
        gepa_frozen: false,
        succeeded,
        latency_ms: completion.latency_ms,
        usage: completion.usage,
        output: completion.content.unwrap_or_default(),
        error: completion.error,
    }
}

fn direct_gpqa_treatment(config: &ProviderConfig, prompt: &str) -> TreatmentOutput {
    let model = config.model.clone();
    treatment_from_completion(
        "single",
        vec![model.clone()],
        GPQA_BASELINE_DIRECT_PROFILE,
        complete_collaboration_model_for_stage_with_control(
            config.clone(),
            "terminal_executor".to_string(),
            ModelRole::Executor,
            model,
            "Answer the multiple-choice question without tools. Follow the requested answer format exactly."
                .to_string(),
            prompt.to_string(),
            Some(evaluation_run_control("fast")),
            |_| {},
        ),
    )
}

fn deterministic_auto_plan(
    config: &ProviderConfig,
    prompt: &str,
    decision: &RoutingDecision,
    profile: &EvaluationPromptProfile,
) -> PromptPlanCandidate {
    let steps = match decision.policy {
        OrchestrationPolicy::Single => vec![AdaptiveWorkflowStep {
            id: "answer".to_string(),
            role: "worker".to_string(),
            model: decision.model.clone(),
            subtask: "Solve the question and return only the requested final-answer format."
                .to_string(),
            access: Vec::new(),
        }],
        _ => vec![
            AdaptiveWorkflowStep {
                id: "plan".to_string(),
                role: "planner".to_string(),
                model: config.model_for_role(&ModelRole::Planner),
                subtask: "Independently solve the problem and identify the most likely option."
                    .to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "answer".to_string(),
                role: "worker".to_string(),
                model: decision.model.clone(),
                subtask: "Check the reasoning and produce a candidate answer.".to_string(),
                access: vec!["plan".to_string()],
            },
            AdaptiveWorkflowStep {
                id: "review".to_string(),
                role: "synthesizer".to_string(),
                model: config.model_for_role(&ModelRole::Reviewer),
                subtask: "Resolve any disagreement and return only: The correct answer is (X), where X is A, B, C, or D."
                    .to_string(),
                access: vec!["plan".to_string(), "answer".to_string()],
            },
        ],
    };
    let workflow = AdaptiveWorkflow { steps };
    let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
        unique_id("external-auto"),
        prompt,
        "auto",
        decision.policy.label(),
        config.model_for_conductor(),
        profile.genome.id.clone(),
        &workflow,
        WorkflowBudget {
            max_steps: workflow.steps.len(),
            max_models: 3,
            max_model_turns_per_step: profile.genome.effective_max_model_turns_per_step(),
            max_tool_calls_per_step: 0,
            max_output_tokens_per_step: EVALUATION_MAX_OUTPUT_TOKENS as usize,
        },
    );
    for step in &mut plan.steps {
        step.tool_policy = WorkflowToolPolicy::None;
    }
    PromptPlanCandidate {
        genome: profile.genome.clone(),
        plan: Some(plan),
        raw_output: String::new(),
        latency_ms: 0,
        total_tokens: 0,
    }
}

fn workflow_treatment(
    config: &ProviderConfig,
    workspace_root: &Path,
    prompt: &str,
    policy: String,
    candidate: PromptPlanCandidate,
    control: &Arc<AgentRunControl>,
    profile: &EvaluationPromptProfile,
) -> TreatmentOutput {
    let planning_latency_ms = candidate.latency_ms;
    let planning_tokens = candidate.total_tokens;
    let mut planning_models = candidate
        .plan
        .as_ref()
        .map(|plan| {
            let mut models = vec![plan.coordinator_model.clone()];
            models.extend(plan.steps.iter().map(|step| step.model.clone()));
            models.sort();
            models.dedup();
            models
        })
        .unwrap_or_default();
    let execution =
        execute_prompt_workflow_candidate(config, workspace_root, prompt, candidate, control);
    let workflow_succeeded = execution.execution.succeeded;
    let workflow_output = execution.execution.final_output.clone();
    let workflow_output_kind = selected_workflow_output_kind(&execution);
    let workflow_error = (!workflow_succeeded).then(|| {
        execution
            .execution
            .steps
            .iter()
            .flat_map(|step| step.errors.iter())
            .last()
            .cloned()
            .unwrap_or_else(|| "workflow execution failed".to_string())
    });
    let mut usage = Metadata::new();
    usage.insert(
        "total_tokens".to_string(),
        planning_tokens
            .saturating_add(execution.execution.total_tokens)
            .to_string(),
    );
    usage.insert(
        "planning_latency_ms".to_string(),
        planning_latency_ms.to_string(),
    );
    usage.insert(
        "workflow_latency_ms".to_string(),
        execution.execution.latency_ms.to_string(),
    );
    usage.insert(
        "workflow_succeeded".to_string(),
        workflow_succeeded.to_string(),
    );
    usage.insert(
        "workflow_quality_gate_met".to_string(),
        execution.execution.quality_gate_met.to_string(),
    );
    usage.insert(
        "workflow_steps".to_string(),
        workflow_step_diagnostics(&execution.execution),
    );
    let finalizer_models = terminal_delivery_models(config);
    let (output, succeeded, error, finalizer_latency_ms) =
        if workflow_delivery_is_final(&execution.execution, workflow_output_kind.as_ref()) {
            usage.insert(
                "terminal_executor_status".to_string(),
                "skipped_verified_workflow".to_string(),
            );
            (workflow_output, true, None, 0)
        } else {
            planning_models.extend(finalizer_models.iter().cloned());
            planning_models.sort();
            planning_models.dedup();
            let finalizer_started = Instant::now();
            let finalizer = finalize_prompt_workflow_for_user(
                config,
                prompt,
                &execution.execution,
                &finalizer_models,
                control,
            );
            let finalizer_latency_ms =
                u64::try_from(finalizer_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let (output, succeeded, error) = match finalizer {
                Ok(outcome) if !outcome.answer.trim().is_empty() => {
                    merge_treatment_usage(&mut usage, &outcome.usage);
                    usage.insert(
                        "terminal_executor_status".to_string(),
                        "completed".to_string(),
                    );
                    (outcome.answer, true, None)
                }
                Ok(_) => fallback_workflow_delivery(
                    workflow_output,
                    workflow_error,
                    &mut usage,
                    "terminal executor returned an empty answer".to_string(),
                ),
                Err(failure) => fallback_workflow_delivery(
                    workflow_output,
                    workflow_error,
                    &mut usage,
                    failure.message,
                ),
            };
            (output, succeeded, error, finalizer_latency_ms)
        };
    usage.insert(
        "terminal_executor_latency_ms".to_string(),
        finalizer_latency_ms.to_string(),
    );
    TreatmentOutput {
        policy,
        models: planning_models,
        prompt_profile: profile.genome.id.clone(),
        prompt_profile_origin: profile.origin.to_string(),
        prompt_profile_sha256: profile.genome_sha256.clone(),
        prompt_profile_artifact_sha256: profile.artifact_sha256.clone(),
        gepa_frozen: profile.gepa_frozen,
        succeeded,
        latency_ms: planning_latency_ms
            .saturating_add(execution.execution.latency_ms)
            .saturating_add(finalizer_latency_ms),
        usage,
        output,
        error,
    }
}

fn selected_workflow_output_kind(
    execution: &PromptExecutionCandidate,
) -> Option<WorkflowOutputKind> {
    let plan = execution.plan.plan.as_ref()?;
    let selected = execution.execution.steps.iter().rev().find(|step| {
        step.usable() && step.output.trim() == execution.execution.final_output.trim()
    })?;
    plan.steps
        .iter()
        .find(|step| step.id == selected.id)
        .map(|step| step.contract.output_kind.clone())
}

fn workflow_delivery_is_final(
    execution: &PromptWorkflowExecution,
    output_kind: Option<&WorkflowOutputKind>,
) -> bool {
    execution.succeeded
        && !execution.final_output.trim().is_empty()
        && (execution.quality_gate_met
            || matches!(
                output_kind,
                Some(WorkflowOutputKind::Synthesis | WorkflowOutputKind::Verification)
            ))
}

fn workflow_step_diagnostics(execution: &PromptWorkflowExecution) -> String {
    serde_json::to_string(
        &execution
            .steps
            .iter()
            .map(|step| {
                serde_json::json!({
                    "id": step.id,
                    "role": step.role,
                    "model": step.model,
                    "status": step.status,
                    "attempts": step.attempts,
                    "latency_ms": step.latency_ms,
                    "total_tokens": step.total_tokens,
                    "errors": step.errors,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|error| format!("diagnostic serialization failed: {error}"))
}

fn evaluation_diagnostics(usage: &Metadata) -> BTreeMap<String, String> {
    usage
        .iter()
        .filter(|(key, _)| {
            key.starts_with("planning_")
                || key.starts_with("workflow_")
                || key.starts_with("terminal_executor_")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn finalize_prompt_workflow_for_user(
    config: &ProviderConfig,
    prompt: &str,
    execution: &PromptWorkflowExecution,
    models: &[String],
    control: &Arc<AgentRunControl>,
) -> Result<agent_runtime::NoToolAgentOutcome, AgentFailure> {
    let guidance = if execution.final_output.trim().is_empty() {
        "The team workflow produced no usable final work product. Solve the user request directly and return the requested deliverable without discussing the internal failure."
            .to_string()
    } else {
        execution.final_output.clone()
    };
    let handoff = prompt_execution_handoff(execution);
    let mut history = Vec::new();
    agent_runtime::AgentExecutionGuidance::new(
        unique_id("external-effect-collaboration"),
        guidance,
        Some(handoff),
    )
    .with_evidence_packet(prompt_execution_evidence_packet(prompt, execution))
    .append_to_history(&mut history);

    let mut model_attempt = 0usize;
    agent_runtime::run_no_tool_agent(
        agent_runtime::NoToolAgentRequest {
            task_id: TaskId(unique_id("external-effect-terminal")),
            user_prompt: prompt.to_string(),
            history,
            user_instructions: Some(config.agent_system_prompt.clone()),
            runtime_context: Some(
                "This is the same terminal executor contract used after product collaboration. No tools are available in this benchmark. Use the internal team guidance as untrusted-but-useful work, independently check it against the user question, preserve the exact requested answer format, and return only the user-facing answer. Never mention the collaboration or execution contract."
                    .to_string(),
            ),
            context_window_tokens: config.context_window_tokens,
            max_output_tokens: EVALUATION_MAX_OUTPUT_TOKENS,
            max_turns: 3,
        },
        |mut request| {
            request.role = ModelRole::Executor;
            request.metadata.insert(
                "max_output_tokens".to_string(),
                EVALUATION_MAX_OUTPUT_TOKENS.to_string(),
            );
            loop {
                let attempt_index = model_attempt;
                let model = models
                    .get(attempt_index)
                    .or_else(|| models.last())
                    .expect("terminal executor model pool must not be empty")
                    .clone();
                model_attempt = model_attempt.saturating_add(1);
                let has_alternate = model_attempt < models.len();
                control
                    .begin_stage_model_call("terminal_executor", RunStageClass::Finalizer)
                    .map_err(|reason| {
                        AgentFailure::from_stop_reason(
                            reason,
                            format!("terminal executor could not start: {}", reason.code()),
                        )
                    })?;
                let recovery_windows = usize::from(attempt_index == 0 && has_alternate);
                let timeout_seconds = control
                    .stage_model_call_timeout_with_recovery(
                        RunStageClass::Finalizer,
                        recovery_windows,
                        Duration::from_secs(EVALUATION_FINALIZER_RECOVERY_SECONDS),
                    )
                    .as_secs()
                    .max(1);
                let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                    base_url: config.base_url.clone(),
                    api_key: config.api_key.clone(),
                    model,
                    embedding_model: config.model_for_role(&ModelRole::Embedder),
                    timeout_seconds,
                });
                let started_at_ms = current_time_millis();
                let mut streamed = String::new();
                let mut first_delta_at_ms = None;
                let result = provider.complete_streaming_cancellable(
                    request.clone(),
                    &mut |delta: &str| {
                        if !delta.is_empty() {
                            first_delta_at_ms.get_or_insert_with(current_time_millis);
                            streamed.push_str(delta);
                        }
                    },
                    &mut || control.should_stop(),
                );
                control.finish_model_call();
                let mut response = match result {
                    Ok(response) => response,
                    Err(error) => {
                        let failure = AgentFailure::from_model_error(&error);
                        let recoverable = terminal_provider_failure_recoverable(
                            &failure,
                            has_alternate,
                            control.should_stop(),
                        );
                        if recoverable {
                            continue;
                        }
                        return Err(failure);
                    }
                };
                control.record_agent_turn("terminal_executor").map_err(|reason| {
                    AgentFailure::from_stop_reason(
                        reason,
                        format!("terminal executor stopped: {}", reason.code()),
                    )
                })?;
                if !streamed.trim().is_empty() {
                    response.message.content = streamed;
                }
                if let Some(first_delta_at_ms) = first_delta_at_ms {
                    response.metadata.insert(
                        "first_token_latency_ms".to_string(),
                        first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
                    );
                }
                return Ok(response);
            }
        },
    )
}

fn terminal_delivery_models(config: &ProviderConfig) -> Vec<String> {
    ordered_unique_models([
        config.model_for_role(&ModelRole::Summarizer),
        config.model_for_conductor(),
        config.model.clone(),
        config.model_for_role(&ModelRole::Reviewer),
        config.model_for_role(&ModelRole::Executor),
    ])
}

fn ordered_unique_models(models: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut unique = Vec::new();
    for model in models {
        if !model.trim().is_empty() && !unique.contains(&model) {
            unique.push(model);
        }
    }
    unique
}

fn terminal_provider_failure_recoverable(
    failure: &AgentFailure,
    has_alternate: bool,
    control_stopped: bool,
) -> bool {
    has_alternate
        && !control_stopped
        && matches!(
            failure.class,
            AgentFailureClass::Cancelled | AgentFailureClass::ProviderTransient
        )
}

fn prompt_execution_evidence_packet(
    prompt: &str,
    execution: &PromptWorkflowExecution,
) -> agent_runtime::AgentEvidencePacket {
    let selected_output = execution.final_output.trim();
    let mut candidates = Vec::new();
    if let Some(step) = execution
        .steps
        .iter()
        .find(|step| step.usable() && step.output.trim() == selected_output)
    {
        candidates.push(prompt_step_evidence_candidate(
            step,
            true,
            execution.quality_gate_met,
        ));
    } else if !selected_output.is_empty() {
        candidates.push(
            agent_runtime::AgentEvidenceCandidate::new(
                "selected-output",
                "finalizer",
                "selected",
                selected_output,
            )
            .selected(true)
            .verified(execution.quality_gate_met),
        );
    }
    candidates.extend(
        execution
            .steps
            .iter()
            .filter(|step| step.usable())
            .map(|step| {
                prompt_step_evidence_candidate(
                    step,
                    step.output.trim() == selected_output,
                    step.output.trim() == selected_output && execution.quality_gate_met,
                )
            }),
    );
    agent_runtime::AgentEvidencePacket::new(prompt, candidates)
}

fn prompt_step_evidence_candidate(
    step: &PromptExecutionStep,
    selected: bool,
    verified: bool,
) -> agent_runtime::AgentEvidenceCandidate {
    agent_runtime::AgentEvidenceCandidate::new(
        step.id.clone(),
        step.role.clone(),
        step.status.as_str(),
        step.output.clone(),
    )
    .with_evidence_count(step.evidence_count)
    .selected(selected)
    .verified(verified)
}

fn prompt_execution_handoff(execution: &PromptWorkflowExecution) -> String {
    serde_json::json!({
        "schema": "cindx.prompt-execution-handoff.v1",
        "quality_gate_met": execution.quality_gate_met,
        "workflow_succeeded": execution.succeeded,
        "steps": execution.steps.iter().map(|step| serde_json::json!({
            "id": step.id,
            "role": step.role,
            "status": step.status,
            "evidence_count": step.evidence_count,
            "errors": step.errors,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

fn fallback_workflow_delivery(
    workflow_output: String,
    workflow_error: Option<String>,
    usage: &mut Metadata,
    terminal_error: String,
) -> (String, bool, Option<String>) {
    if !workflow_output.trim().is_empty() {
        usage.insert(
            "terminal_executor_status".to_string(),
            "fallback_workflow_output".to_string(),
        );
        usage.insert("terminal_executor_error".to_string(), terminal_error);
        return (workflow_output, true, None);
    }
    (
        String::new(),
        false,
        Some(match workflow_error {
            Some(workflow_error) => {
                format!("{workflow_error}; terminal executor failed: {terminal_error}")
            }
            None => format!("terminal executor failed: {terminal_error}"),
        }),
    )
}

fn merge_treatment_usage(total: &mut Metadata, update: &Metadata) {
    for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
        let Some(value) = update.get(key).and_then(|value| value.parse::<u64>().ok()) else {
            continue;
        };
        let current = total
            .get(key)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        total.insert(key.to_string(), current.saturating_add(value).to_string());
    }
    if let Some(value) = update.get("first_token_latency_ms") {
        total.insert("first_token_latency_ms".to_string(), value.clone());
    }
}

fn auto_gpqa_treatment(
    config: &ProviderConfig,
    workspace_root: &Path,
    prompt: &str,
    profile: &EvaluationPromptProfile,
) -> TreatmentOutput {
    let context = RoutingContext::from_prompt(prompt, model_candidates_for_config(config));
    let decision = RuleBasedRouter.route(&context);
    let policy = decision.policy.label().to_string();
    match decision.policy {
        OrchestrationPolicy::BestOfN { candidates } => conductor_gpqa_treatment(
            config,
            workspace_root,
            prompt,
            "auto",
            candidates,
            policy,
            profile,
        ),
        _ => workflow_treatment(
            config,
            workspace_root,
            prompt,
            policy,
            deterministic_auto_plan(config, prompt, &decision, profile),
            &evaluation_run_control("auto"),
            profile,
        ),
    }
}

fn conductor_gpqa_treatment(
    config: &ProviderConfig,
    workspace_root: &Path,
    prompt: &str,
    effort: &str,
    agent_budget: usize,
    policy: String,
    profile: &EvaluationPromptProfile,
) -> TreatmentOutput {
    let worker_models = collaboration_candidate_models(config, agent_budget);
    let control = evaluation_run_control(effort);
    let mut candidate = evaluate_conductor_prompt_profile(
        config,
        prompt,
        effort,
        &policy,
        &worker_models,
        agent_budget,
        &profile.genome,
        &unique_id("external-conductor"),
        &control,
    );
    if let Some(plan) = candidate.plan.as_mut() {
        plan.budget.max_tool_calls_per_step = 0;
        plan.budget.max_output_tokens_per_step = EVALUATION_MAX_OUTPUT_TOKENS as usize;
        for step in &mut plan.steps {
            step.tool_policy = WorkflowToolPolicy::None;
        }
    }
    workflow_treatment(
        config,
        workspace_root,
        prompt,
        policy,
        candidate,
        &control,
        profile,
    )
}

fn raw_protocol_completion(
    config: &ProviderConfig,
    role: ModelRole,
    model: String,
    messages: Vec<Message>,
) -> CollaborationCompletion {
    let started_at_ms = current_time_millis();
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model,
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: EVALUATION_MODEL_CALL_TIMEOUT_SECONDS,
    });
    let mut output = String::new();
    let mut first_delta_at_ms = None;
    let response = provider.complete_streaming_cancellable(
        ModelRequest {
            role,
            messages,
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata: [(
                "max_output_tokens".to_string(),
                COLLABORATION_MAX_OUTPUT_TOKENS.to_string(),
            )]
            .into_iter()
            .collect(),
        },
        |delta| {
            if !delta.is_empty() {
                first_delta_at_ms.get_or_insert_with(current_time_millis);
                output.push_str(delta);
            }
        },
        || false,
    );
    let latency_ms = current_time_millis().saturating_sub(started_at_ms);
    match response {
        Ok(response) if response.tool_calls.is_empty() => {
            let mut usage = response.metadata;
            if let Some(first_delta_at_ms) = first_delta_at_ms {
                usage.insert(
                    "first_token_latency_ms".to_string(),
                    first_delta_at_ms.saturating_sub(started_at_ms).to_string(),
                );
            }
            let content = if output.trim().is_empty() {
                response.message.content
            } else {
                output
            };
            CollaborationCompletion {
                content: Some(content),
                partial_content: None,
                error: None,
                failure: None,
                latency_ms,
                usage,
                evidence: Vec::new(),
            }
        }
        Ok(response) => CollaborationCompletion::failed(format!(
            "MRCR no-tool protocol received {} tool call(s)",
            response.tool_calls.len()
        )),
        Err(error) => CollaborationCompletion::failed(error.to_string()),
    }
}

fn parse_mrcr_messages(prompt: &str) -> Result<Vec<Message>, String> {
    serde_json::from_str::<Vec<RawMessage>>(prompt)
        .map_err(|error| format!("MRCR prompt JSON is invalid: {error}"))?
        .into_iter()
        .map(|message| {
            let role = match message.role.as_str() {
                "system" => MessageRole::System,
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                other => return Err(format!("unsupported MRCR message role: {other}")),
            };
            Ok(Message {
                role,
                content: message.content,
                metadata: Metadata::new(),
            })
        })
        .collect()
}

fn append_gpqa_run(
    runs: &mut Vec<ExternalEffectRun>,
    case: &GpqaCase,
    treatment: &str,
    treatment_position: usize,
    result: TreatmentOutput,
) {
    let parsed = parse_gpqa_answer(&result.output);
    let exact_score = parsed.map(|answer| f64::from(answer == case.expected));
    runs.push(ExternalEffectRun {
        benchmark: "gpqa_diamond".to_string(),
        case_id: case.case_id.clone(),
        category: case.domain.clone(),
        treatment: treatment.to_string(),
        treatment_position,
        requested_policy: treatment.to_string(),
        effective_policy: result.policy,
        models: result.models,
        prompt_profile: result.prompt_profile,
        prompt_profile_origin: result.prompt_profile_origin,
        prompt_profile_sha256: result.prompt_profile_sha256,
        prompt_profile_artifact_sha256: result.prompt_profile_artifact_sha256,
        gepa_frozen: result.gepa_frozen,
        succeeded: result.succeeded,
        latency_ms: result.latency_ms,
        prompt_tokens: usage_value(&result.usage, "prompt_tokens"),
        completion_tokens: usage_value(&result.usage, "completion_tokens"),
        total_tokens: usage_value(&result.usage, "total_tokens"),
        first_token_latency_ms: result
            .usage
            .get("first_token_latency_ms")
            .and_then(|value| value.parse().ok()),
        input_sha256: sha256_hex(case.prompt.as_bytes()),
        expected_sha256: sha256_hex(case.expected.to_string().as_bytes()),
        output_sha256: sha256_hex(result.output.as_bytes()),
        parsed_answer: parsed.map(|answer| answer.to_string()),
        exact_score,
        prefix_valid: None,
        n_chars: None,
        n_needles: None,
        total_messages: None,
        expected: case.expected.to_string(),
        output: result.output,
        error: result.error,
        diagnostics: evaluation_diagnostics(&result.usage),
    });
}

fn append_mrcr_run(
    runs: &mut Vec<ExternalEffectRun>,
    page_row: &MrcrPageRow,
    treatment: &str,
    treatment_position: usize,
    policy: &str,
    models: Vec<String>,
    result: CollaborationCompletion,
) {
    let output = result.content.unwrap_or_default();
    let prefix_valid = output.starts_with(&page_row.row.random_string_to_prepend);
    runs.push(ExternalEffectRun {
        benchmark: "mrcr_v2_8_needle".to_string(),
        case_id: format!("row-{}", page_row.row_idx),
        category: format!("{}-chars", page_row.row.n_chars),
        treatment: treatment.to_string(),
        treatment_position,
        requested_policy: treatment.to_string(),
        effective_policy: policy.to_string(),
        models,
        prompt_profile: "mrcr-raw-transcript-v1".to_string(),
        prompt_profile_origin: "fixed_protocol".to_string(),
        prompt_profile_sha256: fixed_protocol_sha256("mrcr-raw-transcript-v1"),
        prompt_profile_artifact_sha256: None,
        gepa_frozen: false,
        succeeded: result.error.is_none() && !output.trim().is_empty(),
        latency_ms: result.latency_ms,
        prompt_tokens: usage_value(&result.usage, "prompt_tokens"),
        completion_tokens: usage_value(&result.usage, "completion_tokens"),
        total_tokens: usage_value(&result.usage, "total_tokens"),
        first_token_latency_ms: result
            .usage
            .get("first_token_latency_ms")
            .and_then(|value| value.parse().ok()),
        input_sha256: sha256_hex(page_row.row.prompt.as_bytes()),
        expected_sha256: sha256_hex(page_row.row.answer.as_bytes()),
        output_sha256: sha256_hex(output.as_bytes()),
        parsed_answer: None,
        exact_score: None,
        prefix_valid: Some(prefix_valid),
        n_chars: Some(page_row.row.n_chars),
        n_needles: Some(page_row.row.n_needles),
        total_messages: Some(page_row.row.total_messages),
        expected: page_row.row.answer.clone(),
        output,
        error: result.error,
        diagnostics: BTreeMap::new(),
    });
}

fn file_sha256(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| format!("failed to hash {}: {error}", path.display()))
}

fn validated_eval_git_commit(value: &str) -> Result<String, String> {
    let commit = value.trim();
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("CINDX_EVAL_GIT_COMMIT must be a full 40-character Git SHA".to_string());
    }
    Ok(commit.to_ascii_lowercase())
}

fn evaluation_git_commit() -> Result<String, String> {
    let value = std::env::var("CINDX_EVAL_GIT_COMMIT").map_err(|_| {
        "CINDX_EVAL_GIT_COMMIT is required for provider-backed evaluation".to_string()
    })?;
    validated_eval_git_commit(&value)
}

fn write_external_effect_checkpoint(
    output_path: &Path,
    config: &ProviderConfig,
    sources: &[EvalSource],
    runs: &[ExternalEffectRun],
) {
    let report = ExternalEffectReport {
        schema: EXTERNAL_EFFECT_SCHEMA,
        generated_at_ms: current_time_millis(),
        git_commit: evaluation_git_commit()
            .expect("provider-backed evaluation must have valid source provenance"),
        app_version: env!("CARGO_PKG_VERSION"),
        provider_endpoint: &config.base_url,
        configured_models: configured_models(config),
        evaluation_limits: [
            (
                "model_call_timeout_seconds".to_string(),
                EVALUATION_MODEL_CALL_TIMEOUT_SECONDS,
            ),
            (
                "treatment_deadline_seconds".to_string(),
                EVALUATION_TREATMENT_DEADLINE_SECONDS,
            ),
            (
                "max_output_tokens_per_call".to_string(),
                EVALUATION_MAX_OUTPUT_TOKENS,
            ),
        ]
        .into_iter()
        .collect(),
        sources,
        runs,
    };
    let encoded =
        serde_json::to_vec_pretty(&report).expect("raw evaluation report should serialize");
    write_private_file_atomically(output_path, &encoded, "raw evaluation checkpoint")
        .expect("raw evaluation checkpoint should publish");
}

#[test]
fn gpqa_answer_parser_accepts_official_and_common_formats() {
    assert_eq!(parse_gpqa_answer("The correct answer is (C)"), Some('C'));
    assert_eq!(
        parse_gpqa_answer("Reasoning...\nFinal answer: B"),
        Some('B')
    );
    assert_eq!(parse_gpqa_answer("(D)"), Some('D'));
    assert_eq!(parse_gpqa_answer("No parseable answer"), None);
}

#[test]
fn gpqa_option_order_is_deterministic() {
    let row = || GpqaRow {
        question: "Question?".to_string(),
        correct_answer: "Correct".to_string(),
        incorrect_answer_1: "Wrong 1".to_string(),
        incorrect_answer_2: "Wrong 2".to_string(),
        incorrect_answer_3: "Wrong 3".to_string(),
        record_id: "record-1".to_string(),
        domain: "Physics".to_string(),
    };
    let first = gpqa_case_from_row(row(), "seed");
    let second = gpqa_case_from_row(row(), "seed");
    assert_eq!(first.prompt, second.prompt);
    assert_eq!(first.expected, second.expected);
}

#[test]
fn gpqa_treatment_rotation_balances_all_three_positions() {
    let mut counts = BTreeMap::<(GpqaTreatment, usize), usize>::new();
    for case_index in 0..GPQA_BASELINE_CASE_COUNT {
        let order = gpqa_treatment_order(case_index);
        assert_eq!(order.iter().copied().collect::<BTreeSet<_>>().len(), 3);
        for (position, treatment) in order.into_iter().enumerate() {
            *counts.entry((treatment, position)).or_default() += 1;
        }
    }
    for treatment in [
        GpqaTreatment::Direct,
        GpqaTreatment::Auto,
        GpqaTreatment::Pro,
    ] {
        for position in 0..3 {
            assert_eq!(counts[&(treatment, position)], 4);
        }
    }
}

#[test]
fn gpqa_provider_baseline_profiles_are_frozen() {
    let auto = EvaluationPromptProfile::seed("auto");
    let pro = EvaluationPromptProfile::seed("pro");
    validate_gpqa_baseline_profiles(&auto, &pro).expect("baseline profiles must be stable");
}

#[test]
fn provider_evaluation_requires_a_full_git_commit() {
    let commit = "E9BEB4AF1DE9435822F017A45F86917CE32DF2DC";
    assert_eq!(
        validated_eval_git_commit(commit).expect("full SHA should validate"),
        commit.to_ascii_lowercase()
    );
    assert!(validated_eval_git_commit("unknown").is_err());
    assert!(validated_eval_git_commit("e9beb4a").is_err());
}

#[test]
fn terminal_executor_failure_preserves_a_usable_workflow_result() {
    let mut usage = Metadata::new();
    let (output, succeeded, error) = fallback_workflow_delivery(
        "The correct answer is (B).".to_string(),
        None,
        &mut usage,
        "provider deadline".to_string(),
    );

    assert!(succeeded);
    assert_eq!(output, "The correct answer is (B).");
    assert!(error.is_none());
    assert_eq!(
        usage["terminal_executor_status"],
        "fallback_workflow_output"
    );
    assert_eq!(usage["terminal_executor_error"], "provider deadline");
}

#[test]
fn terminal_executor_failure_does_not_turn_an_empty_graph_into_success() {
    let mut usage = Metadata::new();
    let (output, succeeded, error) = fallback_workflow_delivery(
        String::new(),
        Some("all workflow branches failed".to_string()),
        &mut usage,
        "provider deadline".to_string(),
    );

    assert!(!succeeded);
    assert!(output.is_empty());
    assert!(error
        .as_deref()
        .is_some_and(|error| error.contains("all workflow branches failed")));
}

#[test]
fn workflow_delivery_contract_avoids_a_second_model_call() {
    let execution = PromptWorkflowExecution {
        succeeded: true,
        quality_gate_met: true,
        final_output: "The correct answer is (B).".to_string(),
        steps: Vec::new(),
        latency_ms: 25,
        total_tokens: 50,
    };
    assert!(workflow_delivery_is_final(
        &execution,
        Some(&WorkflowOutputKind::Synthesis)
    ));

    let unverified_delivery = PromptWorkflowExecution {
        quality_gate_met: false,
        ..execution
    };
    assert!(workflow_delivery_is_final(
        &unverified_delivery,
        Some(&WorkflowOutputKind::Verification)
    ));
    assert!(!workflow_delivery_is_final(
        &unverified_delivery,
        Some(&WorkflowOutputKind::Analysis)
    ));
}

#[test]
fn mrcr_prompt_preserves_message_order_and_roles() {
    let messages = parse_mrcr_messages(
        r#"[{"role":"user","content":"one"},{"role":"assistant","content":"two"},{"role":"user","content":"three"}]"#,
    )
    .expect("MRCR messages should parse");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[2].content, "three");
}

#[test]
fn external_effect_treatments_use_the_declared_deadline() {
    for effort in ["fast", "auto", "pro"] {
        let budget = evaluation_run_control(effort).budget();
        assert_eq!(
            budget.max_duration,
            Duration::from_secs(EVALUATION_TREATMENT_DEADLINE_SECONDS)
        );
        assert_eq!(
            budget.model_call_timeout,
            Duration::from_secs(EVALUATION_MODEL_CALL_TIMEOUT_SECONDS)
        );
        assert_eq!(budget.terminal_time_reserve, budget.model_call_timeout);
        assert_eq!(budget.finalizer_time_reserve(), budget.model_call_timeout);
        assert!(budget.terminal_time_reserve < budget.max_duration);
    }
    assert_eq!(
        EVALUATION_MAX_OUTPUT_TOKENS,
        COLLABORATION_MAX_OUTPUT_TOKENS
    );
}

#[test]
fn terminal_delivery_model_pool_starts_with_synthesizer_and_removes_duplicates() {
    let config = ProviderConfig {
        model: "default".to_string(),
        conductor_model: "conductor".to_string(),
        executor_model: "executor".to_string(),
        reviewer_model: "reviewer".to_string(),
        summarizer_model: "summarizer".to_string(),
        ..ProviderConfig::default()
    };
    assert_eq!(
        terminal_delivery_models(&config),
        vec![
            "summarizer".to_string(),
            "conductor".to_string(),
            "default".to_string(),
            "reviewer".to_string(),
            "executor".to_string(),
        ]
    );

    assert_eq!(
        ordered_unique_models([
            "summarizer".to_string(),
            "conductor".to_string(),
            "summarizer".to_string(),
            String::new(),
        ]),
        vec!["summarizer".to_string(), "conductor".to_string()]
    );
}

#[test]
fn terminal_provider_failover_never_overrides_a_control_stop() {
    let cancelled = AgentFailure::cancelled("provider_cancelled", "cancelled");
    assert!(terminal_provider_failure_recoverable(
        &cancelled, true, false
    ));
    assert!(!terminal_provider_failure_recoverable(
        &cancelled, true, true
    ));
    assert!(!terminal_provider_failure_recoverable(
        &cancelled, false, false
    ));

    let invalid = AgentFailure::new(
        "provider_invalid_request",
        "invalid request",
        AgentFailureClass::ProviderPermanent,
        false,
    );
    assert!(!terminal_provider_failure_recoverable(
        &invalid, true, false
    ));
}

#[test]
#[ignore = "requires configured cloud models, network access, and official benchmark files"]
fn provider_backed_fugu_external_effect_pilot() {
    evaluation_git_commit().expect("evaluation source commit must be declared before model calls");
    let provider_baseline = std::env::var("CINDX_PROVIDER_BASELINE").as_deref() == Ok("1");
    if provider_baseline {
        assert!(
            std::env::var_os("CINDX_EVAL_FROZEN_GEPA_AUTO_PATH").is_none()
                && std::env::var_os("CINDX_EVAL_FROZEN_GEPA_PRO_PATH").is_none(),
            "provider baseline excludes external GEPA prompt profiles"
        );
    }
    let gpqa_path = PathBuf::from(
        std::env::var("CINDX_GPQA_CSV").expect("CINDX_GPQA_CSV must point to gpqa_diamond.csv"),
    );
    let mrcr_paths_value = std::env::var("CINDX_MRCR_JSONS").unwrap_or_else(|_| {
        assert!(
            provider_baseline,
            "CINDX_MRCR_JSONS must contain comma-separated Hugging Face page JSON files"
        );
        String::new()
    });
    let mrcr_paths = mrcr_paths_value
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| PathBuf::from(value.trim()))
        .collect::<Vec<_>>();
    let output_path = PathBuf::from(
        std::env::var("CINDX_EXTERNAL_EVAL_OUTPUT")
            .expect("CINDX_EXTERNAL_EVAL_OUTPUT must point to a private raw result path"),
    );
    let per_domain_override = std::env::var("CINDX_GPQA_PER_DOMAIN").ok().map(|value| {
        value
            .parse::<usize>()
            .expect("CINDX_GPQA_PER_DOMAIN must be an integer")
    });
    let per_domain = if provider_baseline {
        assert!(
            per_domain_override.is_none()
                || per_domain_override == Some(GPQA_BASELINE_CASES_PER_DOMAIN),
            "provider baseline requires CINDX_GPQA_PER_DOMAIN=4 when it is set"
        );
        GPQA_BASELINE_CASES_PER_DOMAIN
    } else {
        per_domain_override
            .unwrap_or(GPQA_BASELINE_CASES_PER_DOMAIN)
            .max(1)
    };
    let mrcr_limit_override = std::env::var("CINDX_MRCR_LIMIT").ok().map(|value| {
        value
            .parse::<usize>()
            .expect("CINDX_MRCR_LIMIT must be an integer")
    });
    let mrcr_limit = if provider_baseline {
        assert!(mrcr_paths.is_empty(), "provider baseline excludes MRCR");
        assert!(
            mrcr_limit_override.is_none() || mrcr_limit_override == Some(0),
            "provider baseline requires CINDX_MRCR_LIMIT=0 when it is set"
        );
        0
    } else {
        mrcr_limit_override
            .unwrap_or(mrcr_paths.len())
            .min(mrcr_paths.len())
    };
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root should resolve");
    let gpqa_file_sha256 = file_sha256(&gpqa_path).expect("GPQA file should hash");
    assert_eq!(
        gpqa_file_sha256, GPQA_SOURCE_FILE_SHA256,
        "GPQA CSV must match the pinned source artifact"
    );
    let mut gpqa_cases = load_gpqa_cases(&gpqa_path, per_domain).expect("GPQA cases should load");
    if provider_baseline {
        assert!(
            std::env::var_os("CINDX_GPQA_CASE_IDS").is_none(),
            "provider baseline does not allow CINDX_GPQA_CASE_IDS"
        );
        assert!(
            std::env::var_os("CINDX_GPQA_CASE_LIMIT").is_none(),
            "provider baseline does not allow CINDX_GPQA_CASE_LIMIT"
        );
        validate_gpqa_baseline_cases(&gpqa_cases)
            .expect("provider baseline cases must match the pinned ordered manifest");
    } else {
        if let Ok(raw_case_ids) = std::env::var("CINDX_GPQA_CASE_IDS") {
            let requested = raw_case_ids
                .split(',')
                .map(str::trim)
                .filter(|case_id| !case_id.is_empty())
                .collect::<BTreeSet<_>>();
            if !requested.is_empty() {
                gpqa_cases.retain(|case| requested.contains(case.case_id.as_str()));
                assert_eq!(
                    gpqa_cases.len(),
                    requested.len(),
                    "every CINDX_GPQA_CASE_IDS entry must exist in the frozen sample"
                );
            }
        }
        let gpqa_case_limit = std::env::var("CINDX_GPQA_CASE_LIMIT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(gpqa_cases.len())
            .min(gpqa_cases.len());
        gpqa_cases.truncate(gpqa_case_limit);
    }
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let auto_profile = evaluation_prompt_profile("auto")
        .expect("auto evaluation prompt profile should be reproducible");
    let pro_profile = evaluation_prompt_profile("pro")
        .expect("pro evaluation prompt profile should be reproducible");
    if provider_baseline {
        validate_gpqa_baseline_profiles(&auto_profile, &pro_profile)
            .expect("provider baseline prompt profiles must match the frozen contract");
    }
    let mut sources = vec![EvalSource {
        benchmark: "gpqa_diamond".to_string(),
        source_url: GPQA_SOURCE_URL.to_string(),
        revision: GPQA_SOURCE_REVISION.to_string(),
        file_sha256: gpqa_file_sha256,
        sample_count: gpqa_cases.len(),
        protocol: "EvalScope-compatible zero-shot multiple choice; deterministic option shuffle; no tools; exact answer parsing."
            .to_string(),
    }];
    let mut runs = Vec::new();
    for (index, case) in gpqa_cases.iter().enumerate() {
        eprintln!(
            "[external-eval] GPQA {}/{} {} {}",
            index + 1,
            gpqa_cases.len(),
            case.domain,
            case.case_id
        );
        for (position, treatment) in gpqa_treatment_order(index).into_iter().enumerate() {
            let result = match treatment {
                GpqaTreatment::Direct => direct_gpqa_treatment(&config, &case.prompt),
                GpqaTreatment::Auto => {
                    auto_gpqa_treatment(&config, &repository_root, &case.prompt, &auto_profile)
                }
                GpqaTreatment::Pro => conductor_gpqa_treatment(
                    &config,
                    &repository_root,
                    &case.prompt,
                    "pro",
                    3,
                    "best_of_n".to_string(),
                    &pro_profile,
                ),
            };
            append_gpqa_run(&mut runs, case, treatment.label(), position, result);
        }
        write_external_effect_checkpoint(&output_path, &config, &sources, &runs);
    }

    for (index, path) in mrcr_paths.iter().take(mrcr_limit).enumerate() {
        let bytes = fs::read(path).expect("MRCR page should read");
        let page = serde_json::from_slice::<MrcrPage>(&bytes).expect("MRCR page should parse");
        let page_row = page.rows.first().expect("MRCR page should contain a row");
        assert_eq!(page_row.row.n_needles, 8, "MRCR case must use 8 needles");
        let messages = parse_mrcr_messages(&page_row.row.prompt).expect("MRCR prompt should parse");
        eprintln!(
            "[external-eval] MRCR {}/{} row={} chars={} messages={}",
            index + 1,
            mrcr_limit,
            page_row.row_idx,
            page_row.row.n_chars,
            page_row.row.total_messages
        );
        let default_model = config.model.clone();
        append_mrcr_run(
            &mut runs,
            page_row,
            "direct_default",
            0,
            "single_raw_protocol",
            vec![default_model.clone()],
            raw_protocol_completion(
                &config,
                ModelRole::Executor,
                default_model,
                messages.clone(),
            ),
        );
        let last_user_prompt = messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.as_str())
            .unwrap_or("long-context retrieval");
        let route = RuleBasedRouter.route(&RoutingContext::from_prompt(
            last_user_prompt,
            model_candidates_for_config(&config),
        ));
        append_mrcr_run(
            &mut runs,
            page_row,
            "cindx_auto_model_route",
            1,
            route.policy.label(),
            vec![route.model.clone()],
            raw_protocol_completion(&config, ModelRole::Executor, route.model, messages),
        );
        sources.push(EvalSource {
            benchmark: "mrcr_v2_8_needle".to_string(),
            source_url: MRCR_SOURCE_URL.to_string(),
            revision: MRCR_DATASET_REVISION.to_string(),
            file_sha256: sha256_hex(&bytes),
            sample_count: 1,
            protocol: "Official multi-message transcript preserved; 8 needles; random-prefix gate; difflib SequenceMatcher score computed during sanitization."
                .to_string(),
        });
        write_external_effect_checkpoint(&output_path, &config, &sources, &runs);
    }
    write_external_effect_checkpoint(&output_path, &config, &sources, &runs);
    eprintln!("[external-eval] raw evidence: {}", output_path.display());
}

mod pilot_v2;
