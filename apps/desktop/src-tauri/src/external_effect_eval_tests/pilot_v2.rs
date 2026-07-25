use super::*;

const PILOT_SCHEMA: &str = "cindx.pilot_v2.raw.v1";
const PILOT_ID: &str = "pilot-v2-8x4";

#[derive(Debug, Clone)]
struct WorkspaceCase {
    case_id: String,
    category: String,
    objective: String,
    files: Vec<(String, String)>,
    expected: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PilotSource {
    benchmark: String,
    source_url: String,
    revision: String,
    file_sha256: String,
    sample_count: usize,
    protocol: String,
}

#[derive(Debug, Serialize)]
struct PilotStep {
    id: String,
    role: String,
    model: String,
    attempts: usize,
    succeeded: bool,
    latency_ms: u64,
    total_tokens: u64,
    prompt_sha256: String,
    output_sha256: String,
    prompt: String,
    output: String,
    errors: Vec<String>,
    tool_calls: Vec<AgentEvaluationToolTrace>,
}

#[derive(Debug, Serialize)]
struct PilotRun {
    replicate: u32,
    benchmark: String,
    case_id: String,
    category: String,
    treatment: String,
    protocol_class: String,
    effective_policy: String,
    models: Vec<String>,
    prompt_profile: String,
    gepa_frozen: bool,
    succeeded: bool,
    latency_ms: u64,
    planning_latency_ms: u64,
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
    safety_violations: u64,
    error: Option<String>,
    expected: String,
    output: String,
    steps: Vec<PilotStep>,
}

#[derive(Debug, Serialize)]
struct PilotReport<'a> {
    schema: &'static str,
    pilot_id: &'static str,
    generated_at_ms: u64,
    git_commit: String,
    app_version: &'static str,
    replicate: u32,
    provider_endpoint: &'a str,
    configured_models: BTreeMap<String, String>,
    treatments: Vec<&'static str>,
    gepa_frozen: bool,
    execution_boundary: &'static str,
    evaluation_limits: BTreeMap<String, u64>,
    stop_rules: Vec<&'static str>,
    sources: &'a [PilotSource],
    runs: &'a [PilotRun],
}

#[derive(Debug)]
struct DetailedTreatment {
    policy: String,
    models: Vec<String>,
    prompt_profile: String,
    succeeded: bool,
    latency_ms: u64,
    planning_latency_ms: u64,
    usage: Metadata,
    output: String,
    error: Option<String>,
    steps: Vec<PilotStep>,
}

fn workspace_cases() -> Vec<WorkspaceCase> {
    vec![
        WorkspaceCase {
            case_id: "workspace-single-file-secret".to_string(),
            category: "single_file_retrieval".to_string(),
            objective: "Use read-only workspace evidence to inspect evidence/ledger.txt. Return the exact deployment verification code and the filename that contains it. Do not guess.".to_string(),
            files: vec![(
                "evidence/ledger.txt".to_string(),
                "Deployment verification code: CINDX-PILOT-7Q4M\nOwner: release-engineering\n"
                    .to_string(),
            )],
            expected: vec![
                "CINDX-PILOT-7Q4M".to_string(),
                "evidence/ledger.txt".to_string(),
            ],
        },
        WorkspaceCase {
            case_id: "workspace-multi-file-synthesis".to_string(),
            category: "multi_file_synthesis".to_string(),
            objective: "Inspect the read-only workspace and identify the release codename, the only service allowed to ship, and the exact approval ticket. Return all three with their source filenames.".to_string(),
            files: vec![
                (
                    "release/manifest.toml".to_string(),
                    "codename = \"Silver Current\"\nservice = \"relay-gateway\"\n"
                        .to_string(),
                ),
                (
                    "release/approvals.md".to_string(),
                    "# Release approval\nOnly relay-gateway is approved. Ticket: SAFE-2718.\n"
                        .to_string(),
                ),
                (
                    "notes/draft.txt".to_string(),
                    "Unapproved draft: consider shipping analytics-worker later.\n".to_string(),
                ),
            ],
            expected: vec![
                "Silver Current".to_string(),
                "relay-gateway".to_string(),
                "SAFE-2718".to_string(),
                "release/manifest.toml".to_string(),
                "release/approvals.md".to_string(),
            ],
        },
        WorkspaceCase {
            case_id: "workspace-contradiction-resolution".to_string(),
            category: "contradiction_verification".to_string(),
            objective: "Resolve the conflicting port claims in the read-only workspace. State the active production port, identify the authoritative source, and briefly explain why the other claim is stale.".to_string(),
            files: vec![
                (
                    "README.md".to_string(),
                    "Legacy quickstart: connect to port 8080. This section predates the runtime migration.\n"
                        .to_string(),
                ),
                (
                    "config/runtime.toml".to_string(),
                    "# This file is authoritative for production\nproduction_port = 4317\n"
                        .to_string(),
                ),
                (
                    "docs/operations.md".to_string(),
                    "Production settings are sourced from config/runtime.toml; README quickstarts may be stale.\n"
                        .to_string(),
                ),
            ],
            expected: vec![
                "4317".to_string(),
                "config/runtime.toml".to_string(),
                "README".to_string(),
                "stale".to_string(),
            ],
        },
    ]
}

fn pilot_replicate() -> u32 {
    std::env::var("CINDX_PILOT_V2_REPLICATE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1)
}

fn pilot_pair_filter() -> Option<BTreeSet<(String, String)>> {
    let raw = std::env::var("CINDX_PILOT_V2_PAIR_FILTER").ok()?;
    let pairs = raw
        .split(',')
        .filter_map(|entry| {
            let (case_id, treatment) = entry.trim().split_once('|')?;
            Some((case_id.trim().to_string(), treatment.trim().to_string()))
        })
        .collect::<BTreeSet<_>>();
    (!pairs.is_empty()).then_some(pairs)
}

fn selected_pair(
    filter: Option<&BTreeSet<(String, String)>>,
    case_id: &str,
    treatment: &str,
) -> bool {
    filter.is_none_or(|pairs| pairs.contains(&(case_id.to_string(), treatment.to_string())))
}

fn direct_completion(config: &ProviderConfig, prompt: &str, model: String) -> DetailedTreatment {
    let completion = complete_collaboration_model_with_control(
        config.clone(),
        ModelRole::Executor,
        model.clone(),
        "You are the frozen single-model Pilot v2 baseline. Solve the task directly from the supplied information. Do not claim tools or actions that are not present. Return a concise final answer that follows the requested format."
            .to_string(),
        prompt.to_string(),
        Some(evaluation_run_control("fast")),
        |_| {},
    );
    let succeeded = completion.error.is_none()
        && completion
            .content
            .as_ref()
            .is_some_and(|content| !content.trim().is_empty());
    DetailedTreatment {
        policy: "single_raw_baseline".to_string(),
        models: vec![model],
        prompt_profile: "frozen-direct-baseline-v1".to_string(),
        succeeded,
        latency_ms: completion.latency_ms,
        planning_latency_ms: 0,
        usage: completion.usage,
        output: completion.content.unwrap_or_default(),
        error: completion.error,
        steps: Vec::new(),
    }
}

fn deterministic_plan(
    config: &ProviderConfig,
    objective: &str,
    effort: &str,
    policy: &OrchestrationPolicy,
    selected_model: &str,
    tool_policy: WorkflowToolPolicy,
) -> PromptPlanCandidate {
    let profile = ConductorPromptGenome::seed_for_effort(effort);
    let steps = match policy {
        OrchestrationPolicy::Single | OrchestrationPolicy::AutoRouter => vec![
            AdaptiveWorkflowStep {
                id: "answer".to_string(),
                role: "worker".to_string(),
                model: selected_model.to_string(),
                subtask: "Solve the objective completely. Use read-only evidence when available, verify the requested facts, and return the requested final work product."
                    .to_string(),
                access: Vec::new(),
            },
        ],
        OrchestrationPolicy::PlanExecuteReview => vec![
            AdaptiveWorkflowStep {
                id: "plan".to_string(),
                role: "planner".to_string(),
                model: config.model_for_role(&ModelRole::Planner),
                subtask: "Identify the exact claims that must be established and a concise verification plan."
                    .to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "execute".to_string(),
                role: "worker".to_string(),
                model: selected_model.to_string(),
                subtask: "Execute the plan using only authorized evidence and produce a complete candidate answer."
                    .to_string(),
                access: vec!["plan".to_string()],
            },
            AdaptiveWorkflowStep {
                id: "review".to_string(),
                role: "reviewer".to_string(),
                model: config.model_for_role(&ModelRole::Reviewer),
                subtask: "Check every requested claim against the objective and authorized evidence, correct errors, and return only the final answer."
                    .to_string(),
                access: vec!["plan".to_string(), "execute".to_string()],
            },
        ],
        OrchestrationPolicy::BestOfN { .. } => unreachable!("BestOfN uses the conductor"),
    };
    let workflow = AdaptiveWorkflow { steps };
    let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
        unique_id("pilot-v2-plan"),
        objective,
        effort,
        policy.label(),
        config.model_for_conductor(),
        profile.id.clone(),
        &workflow,
        WorkflowBudget {
            max_steps: workflow.steps.len(),
            max_models: 3,
            max_model_turns_per_step: if tool_policy == WorkflowToolPolicy::None {
                1
            } else {
                2
            },
            max_tool_calls_per_step: if tool_policy == WorkflowToolPolicy::None {
                0
            } else {
                4
            },
            max_output_tokens_per_step: COLLABORATION_MAX_OUTPUT_TOKENS as usize,
        },
    );
    for step in &mut plan.steps {
        step.tool_policy = tool_policy.clone();
    }
    PromptPlanCandidate {
        genome: profile,
        plan: Some(plan),
        raw_output: String::new(),
        latency_ms: 0,
        total_tokens: 0,
    }
}

fn conductor_plan(
    config: &ProviderConfig,
    objective: &str,
    effort: &str,
    policy: &OrchestrationPolicy,
    tool_policy: WorkflowToolPolicy,
    control: &Arc<AgentRunControl>,
) -> PromptPlanCandidate {
    let agent_budget = match policy {
        OrchestrationPolicy::BestOfN { candidates } => (*candidates).clamp(1, 3),
        _ => 3,
    };
    let worker_models = collaboration_candidate_models(config, agent_budget);
    let profile = ConductorPromptGenome::seed_for_effort(effort);
    let mut candidate = evaluate_conductor_prompt_profile(
        config,
        objective,
        effort,
        policy.label(),
        &worker_models,
        agent_budget,
        &profile,
        &unique_id("pilot-v2-conductor"),
        control,
    );
    if let Some(plan) = candidate.plan.as_mut() {
        plan.budget.max_model_turns_per_step = if tool_policy == WorkflowToolPolicy::None {
            1
        } else {
            2
        };
        plan.budget.max_tool_calls_per_step = if tool_policy == WorkflowToolPolicy::None {
            0
        } else {
            4
        };
        for step in &mut plan.steps {
            step.tool_policy = tool_policy.clone();
        }
    }
    candidate
}

fn workflow_treatment_detailed(
    config: &ProviderConfig,
    workspace_root: &Path,
    objective: &str,
    treatment: &str,
    tool_policy: WorkflowToolPolicy,
) -> DetailedTreatment {
    let (effort, policy, selected_model) = match treatment {
        "cindx_fast" => ("fast", OrchestrationPolicy::Single, config.model.clone()),
        "cindx_auto" => {
            let decision = RuleBasedRouter.route(&RoutingContext::from_prompt(
                objective,
                model_candidates_for_config(config),
            ));
            ("auto", decision.policy, decision.model)
        }
        "cindx_pro" => (
            "pro",
            OrchestrationPolicy::BestOfN { candidates: 3 },
            config.model_for_role(&ModelRole::Executor),
        ),
        other => panic!("unsupported workflow treatment: {other}"),
    };
    let control = evaluation_run_control(effort);
    let candidate = if matches!(policy, OrchestrationPolicy::BestOfN { .. }) {
        conductor_plan(config, objective, effort, &policy, tool_policy, &control)
    } else {
        deterministic_plan(
            config,
            objective,
            effort,
            &policy,
            &selected_model,
            tool_policy,
        )
    };
    let planning_latency_ms = candidate.latency_ms;
    let planning_tokens = candidate.total_tokens;
    let prompt_profile = candidate.genome.id.clone();
    let mut models = candidate
        .plan
        .as_ref()
        .map(|plan| {
            let mut values = vec![plan.coordinator_model.clone()];
            values.extend(plan.steps.iter().map(|step| step.model.clone()));
            values
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();
    let executed =
        execute_prompt_workflow_candidate(config, workspace_root, objective, candidate, &control);
    let mut usage = Metadata::new();
    usage.insert(
        "total_tokens".to_string(),
        planning_tokens
            .saturating_add(executed.execution.total_tokens)
            .to_string(),
    );
    let error = (!executed.execution.succeeded).then(|| {
        executed
            .execution
            .steps
            .iter()
            .flat_map(|step| step.errors.iter())
            .last()
            .cloned()
            .unwrap_or_else(|| "workflow execution failed".to_string())
    });
    let steps = executed
        .execution
        .steps
        .iter()
        .map(|step| PilotStep {
            id: step.id.clone(),
            role: step.role.clone(),
            model: step.model.clone(),
            attempts: step.attempts,
            succeeded: step.succeeded(),
            latency_ms: step.latency_ms,
            total_tokens: step.total_tokens,
            prompt_sha256: sha256_hex(step.prompt.as_bytes()),
            output_sha256: sha256_hex(step.output.as_bytes()),
            prompt: step.prompt.clone(),
            output: step.output.clone(),
            errors: step.errors.clone(),
            tool_calls: step.tool_calls.clone(),
        })
        .collect::<Vec<_>>();
    DetailedTreatment {
        policy: policy.label().to_string(),
        models,
        prompt_profile,
        succeeded: executed.execution.succeeded,
        latency_ms: planning_latency_ms.saturating_add(executed.execution.latency_ms),
        planning_latency_ms,
        usage,
        output: executed.execution.final_output,
        error,
        steps,
    }
}

fn tool_safety_violations(steps: &[PilotStep]) -> u64 {
    steps
        .iter()
        .flat_map(|step| step.tool_calls.iter())
        .filter(|call| {
            !matches!(
                call.tool.as_str(),
                "file.read" | "file.read_many" | "file.list" | "file.search"
            )
        })
        .count() as u64
}

fn gpqa_run(case: &GpqaCase, treatment: &str, result: DetailedTreatment) -> PilotRun {
    let parsed = parse_gpqa_answer(&result.output);
    let safety_violations = tool_safety_violations(&result.steps);
    PilotRun {
        replicate: pilot_replicate(),
        benchmark: "gpqa_diamond".to_string(),
        case_id: case.case_id.clone(),
        category: case.domain.clone(),
        treatment: treatment.to_string(),
        protocol_class: if treatment == "single_model_baseline" {
            "official_protocol"
        } else {
            "product_mechanism"
        }
        .to_string(),
        effective_policy: result.policy,
        models: result.models,
        prompt_profile: result.prompt_profile,
        gepa_frozen: true,
        succeeded: result.succeeded,
        latency_ms: result.latency_ms,
        planning_latency_ms: result.planning_latency_ms,
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
        exact_score: parsed.map(|answer| f64::from(answer == case.expected)),
        prefix_valid: None,
        n_chars: None,
        n_needles: None,
        total_messages: None,
        safety_violations,
        error: result.error,
        expected: case.expected.to_string(),
        output: result.output,
        steps: result.steps,
    }
}

fn workspace_run(case: &WorkspaceCase, treatment: &str, result: DetailedTreatment) -> PilotRun {
    let normalized_output = result.output.to_lowercase();
    let matched = case
        .expected
        .iter()
        .filter(|expected| normalized_output.contains(&expected.to_lowercase()))
        .count();
    let safety_violations = tool_safety_violations(&result.steps);
    PilotRun {
        replicate: pilot_replicate(),
        benchmark: "cindx_read_only_workspace".to_string(),
        case_id: case.case_id.clone(),
        category: case.category.clone(),
        treatment: treatment.to_string(),
        protocol_class: "product_mechanism".to_string(),
        effective_policy: result.policy,
        models: result.models,
        prompt_profile: result.prompt_profile,
        gepa_frozen: true,
        succeeded: result.succeeded,
        latency_ms: result.latency_ms,
        planning_latency_ms: result.planning_latency_ms,
        prompt_tokens: usage_value(&result.usage, "prompt_tokens"),
        completion_tokens: usage_value(&result.usage, "completion_tokens"),
        total_tokens: usage_value(&result.usage, "total_tokens"),
        first_token_latency_ms: result
            .usage
            .get("first_token_latency_ms")
            .and_then(|value| value.parse().ok()),
        input_sha256: sha256_hex(case.objective.as_bytes()),
        expected_sha256: sha256_hex(case.expected.join("\u{1f}").as_bytes()),
        output_sha256: sha256_hex(result.output.as_bytes()),
        parsed_answer: None,
        exact_score: Some(matched as f64 / case.expected.len() as f64),
        prefix_valid: None,
        n_chars: None,
        n_needles: None,
        total_messages: None,
        safety_violations,
        error: result.error,
        expected: case.expected.join("\n"),
        output: result.output,
        steps: result.steps,
    }
}

fn mrcr_run(
    page_row: &MrcrPageRow,
    treatment: &str,
    policy: &str,
    models: Vec<String>,
    result: CollaborationCompletion,
) -> PilotRun {
    let output = result.content.unwrap_or_default();
    let prefix_valid = output.starts_with(&page_row.row.random_string_to_prepend);
    PilotRun {
        replicate: pilot_replicate(),
        benchmark: "mrcr_v2_8_needle".to_string(),
        case_id: format!("row-{}", page_row.row_idx),
        category: format!("{}-chars", page_row.row.n_chars),
        treatment: treatment.to_string(),
        protocol_class: "official_protocol".to_string(),
        effective_policy: policy.to_string(),
        models,
        prompt_profile: "raw-transcript-v1".to_string(),
        gepa_frozen: true,
        succeeded: result.error.is_none() && !output.trim().is_empty(),
        latency_ms: result.latency_ms,
        planning_latency_ms: 0,
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
        safety_violations: 0,
        error: result.error,
        expected: page_row.row.answer.clone(),
        output,
        steps: Vec::new(),
    }
}

fn baseline_workspace_prompt(case: &WorkspaceCase) -> String {
    let evidence = case
        .files
        .iter()
        .map(|(path, content)| format!("[{path}]\n{content}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n\nFrozen read-only evidence corpus:\n{}",
        case.objective, evidence
    )
}

fn materialize_workspace_case(root: &Path, case: &WorkspaceCase) {
    for (relative, content) in &case.files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("Pilot workspace parent should exist");
        }
        fs::write(path, content).expect("Pilot workspace fixture should write");
    }
}

fn write_pilot_checkpoint(
    output_path: &Path,
    config: &ProviderConfig,
    sources: &[PilotSource],
    runs: &[PilotRun],
) {
    let report = PilotReport {
        schema: PILOT_SCHEMA,
        pilot_id: PILOT_ID,
        generated_at_ms: current_time_millis(),
        git_commit: std::env::var("CINDX_EVAL_GIT_COMMIT")
            .unwrap_or_else(|_| "unknown".to_string()),
        app_version: env!("CARGO_PKG_VERSION"),
        replicate: pilot_replicate(),
        provider_endpoint: &config.base_url,
        configured_models: configured_models(config),
        treatments: vec![
            "single_model_baseline",
            "cindx_fast",
            "cindx_auto",
            "cindx_pro",
        ],
        gepa_frozen: true,
        execution_boundary: "Temporary read-only workspaces only; no generated code execution, writes, shell, browser, computer-use, or benchmark network tools.",
        evaluation_limits: [
            (
                "model_call_timeout_seconds".to_string(),
                EVALUATION_MODEL_CALL_TIMEOUT_SECONDS,
            ),
            (
                "treatment_deadline_seconds".to_string(),
                EVALUATION_TREATMENT_DEADLINE_SECONDS,
            ),
            ("first_pass_run_cap".to_string(), 32),
            ("total_run_cap_with_reruns".to_string(), 56),
        ]
        .into_iter()
        .collect(),
        stop_rules: vec![
            "Stop immediately on any safety-boundary violation.",
            "Stop before the full benchmark if one treatment has repeated infrastructure failures.",
            "Use a second seed only for failed or materially ambiguous first-pass cells.",
        ],
        sources,
        runs,
    };
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).expect("Pilot output directory should exist");
    }
    let temporary_path = output_path.with_extension("json.tmp");
    fs::write(
        &temporary_path,
        serde_json::to_vec_pretty(&report).expect("Pilot report should serialize"),
    )
    .expect("Pilot checkpoint should write");
    fs::rename(&temporary_path, output_path).expect("Pilot checkpoint should publish");
}

#[test]
fn pilot_v2_workspace_cases_have_stable_unique_ids() {
    let cases = workspace_cases();
    let ids = cases
        .iter()
        .map(|case| case.case_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), cases.len());
    assert!(cases.iter().all(|case| !case.expected.is_empty()));
}

#[test]
fn pilot_v2_safety_boundary_accepts_batch_reads_but_rejects_writes() {
    let step = |tool: &str| PilotStep {
        id: "worker".to_string(),
        role: "worker".to_string(),
        model: "model".to_string(),
        attempts: 1,
        succeeded: true,
        latency_ms: 1,
        total_tokens: 1,
        prompt_sha256: String::new(),
        output_sha256: String::new(),
        prompt: String::new(),
        output: String::new(),
        errors: Vec::new(),
        tool_calls: vec![AgentEvaluationToolTrace {
            tool: tool.to_string(),
            request: "{}".to_string(),
            response: String::new(),
            error: None,
        }],
    };

    assert_eq!(tool_safety_violations(&[step("file.read_many")]), 0);
    assert_eq!(tool_safety_violations(&[step("file.write")]), 1);
}

#[test]
#[ignore = "requires configured cloud models, network access, and official benchmark files"]
fn provider_backed_pilot_v2() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let pair_filter = pilot_pair_filter();
    let workspace_cases = workspace_cases();
    let workspace_case_ids = workspace_cases
        .iter()
        .map(|case| case.case_id.as_str())
        .collect::<BTreeSet<_>>();
    let selects_workspace = pair_filter.as_ref().is_none_or(|pairs| {
        pairs
            .iter()
            .any(|(case_id, _)| workspace_case_ids.contains(case_id.as_str()))
    });
    let selects_mrcr = pair_filter
        .as_ref()
        .is_none_or(|pairs| pairs.iter().any(|(case_id, _)| case_id.starts_with("row-")));
    let selects_gpqa = pair_filter.as_ref().is_none_or(|pairs| {
        pairs.iter().any(|(case_id, _)| {
            !case_id.starts_with("row-") && !workspace_case_ids.contains(case_id.as_str())
        })
    });
    let output_path = PathBuf::from(
        std::env::var("CINDX_PILOT_V2_OUTPUT")
            .expect("CINDX_PILOT_V2_OUTPUT must point to a private raw result path"),
    );
    let git_commit =
        std::env::var("CINDX_EVAL_GIT_COMMIT").unwrap_or_else(|_| "unknown".to_string());
    eprintln!(
        "[pilot-v2] frozen baseline app={} commit={} GEPA=off",
        env!("CARGO_PKG_VERSION"),
        git_commit
    );

    let mut sources = Vec::new();
    let gpqa_cases = if selects_gpqa {
        let gpqa_path = PathBuf::from(
            std::env::var("CINDX_PILOT_V2_GPQA_CSV")
                .expect("CINDX_PILOT_V2_GPQA_CSV must point to gpqa_diamond.csv"),
        );
        let cases = load_gpqa_cases(&gpqa_path, 1).expect("GPQA cases should load");
        assert_eq!(cases.len(), 3);
        sources.push(PilotSource {
            benchmark: "gpqa_diamond".to_string(),
            source_url: GPQA_SOURCE_URL.to_string(),
            revision: GPQA_SOURCE_REVISION.to_string(),
            file_sha256: file_sha256(&gpqa_path).expect("GPQA file should hash"),
            sample_count: cases.len(),
            protocol: "Official zero-shot multiple choice with deterministic option shuffle. The direct baseline is protocol evidence; Cindx treatments are product-mechanism evidence."
                .to_string(),
        });
        cases
    } else {
        Vec::new()
    };
    if selects_workspace {
        sources.push(PilotSource {
            benchmark: "cindx_read_only_workspace".to_string(),
            source_url: "local-generated-fixture".to_string(),
            revision: "pilot-v2-fixture-v1".to_string(),
            file_sha256: sha256_hex(
                workspace_cases
                    .iter()
                    .flat_map(|case| case.files.iter())
                    .flat_map(|(path, content)| [path.as_bytes(), content.as_bytes()])
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>()
                    .as_slice(),
            ),
            sample_count: workspace_cases.len(),
            protocol: "Deterministic temporary fixtures. Baseline receives the same evidence inline; Cindx modes receive only bounded read-only workspace tools."
                .to_string(),
        });
    }
    let mrcr_paths = if selects_mrcr {
        let paths = std::env::var("CINDX_PILOT_V2_MRCR_JSONS")
            .expect("CINDX_PILOT_V2_MRCR_JSONS must contain two page JSON files")
            .split(',')
            .filter(|value| !value.trim().is_empty())
            .map(|value| PathBuf::from(value.trim()))
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 2, "Pilot v2 requires exactly two MRCR rows");
        paths
    } else {
        Vec::new()
    };
    let mut mrcr_rows = Vec::new();
    for path in &mrcr_paths {
        let bytes = fs::read(path).expect("MRCR page should read");
        let page = serde_json::from_slice::<MrcrPage>(&bytes).expect("MRCR page should parse");
        let page_row = page
            .rows
            .into_iter()
            .next()
            .expect("MRCR page should have one row");
        assert_eq!(page_row.row.n_needles, 8);
        sources.push(PilotSource {
            benchmark: "mrcr_v2_8_needle".to_string(),
            source_url: MRCR_SOURCE_URL.to_string(),
            revision: MRCR_DATASET_REVISION.to_string(),
            file_sha256: sha256_hex(&bytes),
            sample_count: 1,
            protocol: "Official multi-message transcript preserved. All four treatments are single-call protocol-preserving worker routes; this segment does not test multi-model synthesis."
                .to_string(),
        });
        mrcr_rows.push(page_row);
    }

    let treatments = [
        "single_model_baseline",
        "cindx_fast",
        "cindx_auto",
        "cindx_pro",
    ];
    let expected_runs = pair_filter.as_ref().map_or(32, BTreeSet::len);
    let mut runs = Vec::new();

    for case in &gpqa_cases {
        for treatment in treatments {
            if !selected_pair(pair_filter.as_ref(), &case.case_id, treatment) {
                continue;
            }
            eprintln!(
                "[pilot-v2] run {}/{} GPQA {} {}",
                runs.len() + 1,
                expected_runs,
                case.domain,
                treatment
            );
            let result = if treatment == "single_model_baseline" {
                direct_completion(&config, &case.prompt, config.model.clone())
            } else {
                workflow_treatment_detailed(
                    &config,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &case.prompt,
                    treatment,
                    WorkflowToolPolicy::None,
                )
            };
            let run = gpqa_run(case, treatment, result);
            assert_eq!(run.safety_violations, 0, "Pilot safety boundary failed");
            runs.push(run);
            write_pilot_checkpoint(&output_path, &config, &sources, &runs);
        }
    }

    let fixture_root = std::env::temp_dir().join(unique_id("cindx-pilot-v2-workspace"));
    fs::create_dir_all(&fixture_root).expect("Pilot fixture root should exist");
    for case in &workspace_cases {
        let case_root = fixture_root.join(&case.case_id);
        fs::create_dir_all(&case_root).expect("Pilot case root should exist");
        materialize_workspace_case(&case_root, case);
        for treatment in treatments {
            if !selected_pair(pair_filter.as_ref(), &case.case_id, treatment) {
                continue;
            }
            eprintln!(
                "[pilot-v2] run {}/{} WORKSPACE {} {}",
                runs.len() + 1,
                expected_runs,
                case.case_id,
                treatment
            );
            let result = if treatment == "single_model_baseline" {
                direct_completion(
                    &config,
                    &baseline_workspace_prompt(case),
                    config.model.clone(),
                )
            } else {
                workflow_treatment_detailed(
                    &config,
                    &case_root,
                    &case.objective,
                    treatment,
                    WorkflowToolPolicy::ReadOnlyEvidence,
                )
            };
            let run = workspace_run(case, treatment, result);
            assert_eq!(run.safety_violations, 0, "Pilot safety boundary failed");
            runs.push(run);
            write_pilot_checkpoint(&output_path, &config, &sources, &runs);
        }
    }
    fs::remove_dir_all(&fixture_root).expect("Pilot fixture root should clean up");

    for page_row in &mrcr_rows {
        let messages = parse_mrcr_messages(&page_row.row.prompt).expect("MRCR prompt should parse");
        let last_user_prompt = messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.as_str())
            .unwrap_or("long-context retrieval");
        let auto_route = RuleBasedRouter.route(&RoutingContext::from_prompt(
            last_user_prompt,
            model_candidates_for_config(&config),
        ));
        for treatment in treatments {
            let case_id = format!("row-{}", page_row.row_idx);
            if !selected_pair(pair_filter.as_ref(), &case_id, treatment) {
                continue;
            }
            eprintln!(
                "[pilot-v2] run {}/{} MRCR row={} chars={} {}",
                runs.len() + 1,
                expected_runs,
                page_row.row_idx,
                page_row.row.n_chars,
                treatment
            );
            let (policy, model) = match treatment {
                "single_model_baseline" => ("single_raw_baseline", config.model.clone()),
                "cindx_fast" => ("single_raw_fast", config.model.clone()),
                "cindx_auto" => (auto_route.policy.label(), auto_route.model.clone()),
                "cindx_pro" => (
                    "protocol_preserving_pro_worker_proxy",
                    config.model_for_role(&ModelRole::Executor),
                ),
                _ => unreachable!(),
            };
            let completion = raw_protocol_completion(
                &config,
                ModelRole::Executor,
                model.clone(),
                messages.clone(),
            );
            runs.push(mrcr_run(
                page_row,
                treatment,
                policy,
                vec![model],
                completion,
            ));
            write_pilot_checkpoint(&output_path, &config, &sources, &runs);
        }
    }

    assert_eq!(
        runs.len(),
        expected_runs,
        "Pilot v2 must execute every selected pair"
    );
    write_pilot_checkpoint(&output_path, &config, &sources, &runs);
    eprintln!("[pilot-v2] raw evidence: {}", output_path.display());
}
