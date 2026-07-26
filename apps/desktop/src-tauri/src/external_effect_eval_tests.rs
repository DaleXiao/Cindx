use super::*;
use orchestrator::{AdaptiveWorkflow, AdaptiveWorkflowStep};

const EXTERNAL_EFFECT_SCHEMA: &str = "cindx.external_effect_eval.raw.v1";
const GPQA_SOURCE_URL: &str = "https://github.com/idavidrein/gpqa";
const GPQA_SOURCE_REVISION: &str = "56686c06f5e19865c153de0fdb11be3890014df7";
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
    requested_policy: String,
    effective_policy: String,
    models: Vec<String>,
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
    succeeded: bool,
    latency_ms: u64,
    usage: Metadata,
    output: String,
    error: Option<String>,
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
) -> PromptPlanCandidate {
    let profile = ConductorPromptGenome::seed_for_effort("auto");
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
                role: "reviewer".to_string(),
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
        profile.id.clone(),
        &workflow,
        WorkflowBudget {
            max_steps: workflow.steps.len(),
            max_models: 3,
            max_model_turns_per_step: profile.effective_max_model_turns_per_step(),
            max_tool_calls_per_step: 0,
            max_output_tokens_per_step: EVALUATION_MAX_OUTPUT_TOKENS as usize,
        },
    );
    for step in &mut plan.steps {
        step.tool_policy = WorkflowToolPolicy::None;
    }
    PromptPlanCandidate {
        genome: profile,
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
    let finalizer_models = terminal_executor_models(config);
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
        Err(failure) => {
            fallback_workflow_delivery(workflow_output, workflow_error, &mut usage, failure.message)
        }
    };
    TreatmentOutput {
        policy,
        models: planning_models,
        succeeded,
        latency_ms: planning_latency_ms
            .saturating_add(execution.execution.latency_ms)
            .saturating_add(finalizer_latency_ms),
        usage,
        output,
        error,
    }
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

fn terminal_executor_models(config: &ProviderConfig) -> Vec<String> {
    ordered_unique_models([
        config.model_for_role(&ModelRole::Executor),
        config.model_for_role(&ModelRole::Reviewer),
        config.model.clone(),
        config.model_for_conductor(),
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
) -> TreatmentOutput {
    let context = RoutingContext::from_prompt(prompt, model_candidates_for_config(config));
    let decision = RuleBasedRouter.route(&context);
    let policy = decision.policy.label().to_string();
    match decision.policy {
        OrchestrationPolicy::BestOfN { candidates } => {
            conductor_gpqa_treatment(config, workspace_root, prompt, "auto", candidates, policy)
        }
        _ => workflow_treatment(
            config,
            workspace_root,
            prompt,
            policy,
            deterministic_auto_plan(config, prompt, &decision),
            &evaluation_run_control("auto"),
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
        &ConductorPromptGenome::seed_for_effort(effort),
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
    workflow_treatment(config, workspace_root, prompt, policy, candidate, &control)
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
    result: TreatmentOutput,
) {
    let parsed = parse_gpqa_answer(&result.output);
    let exact_score = parsed.map(|answer| f64::from(answer == case.expected));
    runs.push(ExternalEffectRun {
        benchmark: "gpqa_diamond".to_string(),
        case_id: case.case_id.clone(),
        category: case.domain.clone(),
        treatment: treatment.to_string(),
        requested_policy: treatment.to_string(),
        effective_policy: result.policy,
        models: result.models,
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
    });
}

fn append_mrcr_run(
    runs: &mut Vec<ExternalEffectRun>,
    page_row: &MrcrPageRow,
    treatment: &str,
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
        requested_policy: treatment.to_string(),
        effective_policy: policy.to_string(),
        models,
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
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).expect("raw evaluation output directory should exist");
    }
    let temporary_path = output_path.with_extension("json.tmp");
    fs::write(
        &temporary_path,
        serde_json::to_vec_pretty(&report).expect("raw evaluation report should serialize"),
    )
    .expect("raw evaluation checkpoint should write");
    fs::rename(&temporary_path, output_path).expect("raw evaluation checkpoint should publish");
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
fn terminal_executor_model_pool_preserves_order_and_removes_duplicates() {
    assert_eq!(
        ordered_unique_models([
            "executor".to_string(),
            "reviewer".to_string(),
            "executor".to_string(),
            String::new(),
        ]),
        vec!["executor".to_string(), "reviewer".to_string()]
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
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let gpqa_path = PathBuf::from(
        std::env::var("CINDX_GPQA_CSV").expect("CINDX_GPQA_CSV must point to gpqa_diamond.csv"),
    );
    let mrcr_paths = std::env::var("CINDX_MRCR_JSONS")
        .expect("CINDX_MRCR_JSONS must contain comma-separated Hugging Face page JSON files")
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| PathBuf::from(value.trim()))
        .collect::<Vec<_>>();
    let output_path = PathBuf::from(
        std::env::var("CINDX_EXTERNAL_EVAL_OUTPUT")
            .expect("CINDX_EXTERNAL_EVAL_OUTPUT must point to a private raw result path"),
    );
    let per_domain = std::env::var("CINDX_GPQA_PER_DOMAIN")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4)
        .max(1);
    let mrcr_limit = std::env::var("CINDX_MRCR_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(mrcr_paths.len())
        .min(mrcr_paths.len());
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root should resolve");
    let mut gpqa_cases = load_gpqa_cases(&gpqa_path, per_domain).expect("GPQA cases should load");
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
    let mut sources = vec![EvalSource {
        benchmark: "gpqa_diamond".to_string(),
        source_url: GPQA_SOURCE_URL.to_string(),
        revision: GPQA_SOURCE_REVISION.to_string(),
        file_sha256: file_sha256(&gpqa_path).expect("GPQA file should hash"),
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
        append_gpqa_run(
            &mut runs,
            case,
            "direct_default",
            direct_gpqa_treatment(&config, &case.prompt),
        );
        append_gpqa_run(
            &mut runs,
            case,
            "cindx_auto",
            auto_gpqa_treatment(&config, &repository_root, &case.prompt),
        );
        append_gpqa_run(
            &mut runs,
            case,
            "cindx_pro",
            conductor_gpqa_treatment(
                &config,
                &repository_root,
                &case.prompt,
                "pro",
                3,
                "best_of_n".to_string(),
            ),
        );
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
