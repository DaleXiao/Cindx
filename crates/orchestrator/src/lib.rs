use agent_core::{Metadata, ModelRole};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestrationPolicy {
    Single,
    PlanExecuteReview,
    BestOfN { candidates: usize },
    AutoRouter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationStep {
    pub role: ModelRole,
    pub instruction: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationPlan {
    pub policy: OrchestrationPolicy,
    pub steps: Vec<OrchestrationStep>,
    pub metadata: Metadata,
}

impl OrchestrationPolicy {
    pub fn label(&self) -> &'static str {
        match self {
            OrchestrationPolicy::Single => "single",
            OrchestrationPolicy::PlanExecuteReview => "plan_execute_review",
            OrchestrationPolicy::BestOfN { .. } => "best_of_n",
            OrchestrationPolicy::AutoRouter => "auto_router",
        }
    }
}

pub fn parse_policy(label: &str) -> Option<OrchestrationPolicy> {
    match label {
        "single" => Some(OrchestrationPolicy::Single),
        "plan_execute_review" => Some(OrchestrationPolicy::PlanExecuteReview),
        "best_of_n" => Some(OrchestrationPolicy::BestOfN { candidates: 3 }),
        "auto_router" => Some(OrchestrationPolicy::AutoRouter),
        _ => None,
    }
}

pub fn role_label(role: &ModelRole) -> &'static str {
    match role {
        ModelRole::Planner => "planner",
        ModelRole::Executor => "executor",
        ModelRole::Reviewer => "reviewer",
        ModelRole::Summarizer => "summarizer",
        ModelRole::Embedder => "embedder",
    }
}

pub fn step_prompt(
    plan: &OrchestrationPlan,
    step_index: usize,
    user_prompt: &str,
    previous_outputs: &[String],
) -> Option<String> {
    let step = plan.steps.get(step_index)?;
    let mut prompt = String::new();
    prompt.push_str("You are running inside Cindx orchestration.\n");
    prompt.push_str(&format!("Policy: {}\n", plan.policy.label()));
    prompt.push_str(&format!("Role: {}\n", role_label(&step.role)));
    prompt.push_str(&format!("Step instruction: {}\n\n", step.instruction));
    prompt.push_str("User request:\n");
    prompt.push_str(user_prompt);
    prompt.push('\n');

    if !previous_outputs.is_empty() {
        prompt.push_str("\nPrevious step outputs:\n");
        for (index, output) in previous_outputs.iter().enumerate() {
            prompt.push_str(&format!("Step {}:\n{}\n", index + 1, output));
        }
    }

    Some(prompt)
}

pub fn default_plan(policy: OrchestrationPolicy) -> OrchestrationPlan {
    let steps = match policy {
        OrchestrationPolicy::Single => vec![OrchestrationStep {
            role: ModelRole::Executor,
            instruction: "Answer or act directly with tool support when needed.".to_string(),
            metadata: Metadata::new(),
        }],
        OrchestrationPolicy::PlanExecuteReview => vec![
            OrchestrationStep {
                role: ModelRole::Planner,
                instruction: "Create a concise, checkable plan.".to_string(),
                metadata: Metadata::new(),
            },
            OrchestrationStep {
                role: ModelRole::Executor,
                instruction: "Execute the approved plan through local tools.".to_string(),
                metadata: Metadata::new(),
            },
            OrchestrationStep {
                role: ModelRole::Reviewer,
                instruction: "Review the result against the request and evidence.".to_string(),
                metadata: Metadata::new(),
            },
        ],
        OrchestrationPolicy::BestOfN { candidates } => vec![
            OrchestrationStep {
                role: ModelRole::Planner,
                instruction: format!("Generate {candidates} independent candidate approaches."),
                metadata: Metadata::new(),
            },
            OrchestrationStep {
                role: ModelRole::Reviewer,
                instruction: "Select or synthesize the best candidate using external evidence when possible.".to_string(),
                metadata: Metadata::new(),
            },
        ],
        OrchestrationPolicy::AutoRouter => vec![OrchestrationStep {
            role: ModelRole::Executor,
            instruction: "Answer or act directly after the router selects a concrete policy.".to_string(),
            metadata: Metadata::new(),
        }],
    };

    OrchestrationPlan {
        policy,
        steps,
        metadata: Metadata::new(),
    }
}

pub const MAX_ADAPTIVE_WORKFLOW_STEPS: usize = 5;
pub const MAX_ADAPTIVE_WORKFLOW_AGENTS: usize = 3;
pub const WORKFLOW_IR_SCHEMA: &str = "cindx.workflow.v1";

pub fn adaptive_workflow_step_budget(agent_budget: usize) -> usize {
    match agent_budget.clamp(1, MAX_ADAPTIVE_WORKFLOW_AGENTS) {
        1 => 1,
        2 => 3,
        _ => MAX_ADAPTIVE_WORKFLOW_STEPS,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveWorkflowStep {
    pub id: String,
    pub role: String,
    pub model: String,
    pub subtask: String,
    pub access: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveWorkflow {
    pub steps: Vec<AdaptiveWorkflowStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowToolPolicy {
    None,
    ReadOnlyEvidence,
}

impl WorkflowToolPolicy {
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ReadOnlyEvidence => "read_only_evidence",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowBudget {
    pub max_steps: usize,
    pub max_models: usize,
    pub max_model_turns_per_step: usize,
    pub max_tool_calls_per_step: usize,
    pub max_output_tokens_per_step: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPlanStep {
    pub id: String,
    pub role: String,
    pub model: String,
    pub subtask: String,
    pub access: Vec<String>,
    pub tool_policy: WorkflowToolPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPlanIr {
    pub schema: String,
    pub workflow_id: String,
    pub objective: String,
    pub effort: String,
    pub policy: String,
    pub coordinator_model: String,
    pub steps: Vec<WorkflowPlanStep>,
    pub budget: WorkflowBudget,
}

impl WorkflowPlanIr {
    #[allow(clippy::too_many_arguments)]
    pub fn from_adaptive(
        workflow_id: impl Into<String>,
        objective: impl Into<String>,
        effort: impl Into<String>,
        policy: impl Into<String>,
        coordinator_model: impl Into<String>,
        workflow: &AdaptiveWorkflow,
        budget: WorkflowBudget,
    ) -> Self {
        Self {
            schema: WORKFLOW_IR_SCHEMA.to_string(),
            workflow_id: workflow_id.into(),
            objective: objective.into(),
            effort: effort.into(),
            policy: policy.into(),
            coordinator_model: coordinator_model.into(),
            steps: workflow.steps.iter().map(|step| WorkflowPlanStep {
                id: step.id.clone(),
                role: step.role.clone(),
                model: step.model.clone(),
                subtask: step.subtask.clone(),
                access: step.access.clone(),
                tool_policy: WorkflowToolPolicy::ReadOnlyEvidence,
            }).collect(),
            budget,
        }
    }

    pub fn adaptive_workflow(&self) -> AdaptiveWorkflow {
        AdaptiveWorkflow {
            steps: self.steps.iter().map(|step| AdaptiveWorkflowStep {
                id: step.id.clone(),
                role: step.role.clone(),
                model: step.model.clone(),
                subtask: step.subtask.clone(),
                access: step.access.clone(),
            }).collect(),
        }
    }

    pub fn validate(&self, allowed_models: &[String]) -> Result<(), String> {
        if self.schema != WORKFLOW_IR_SCHEMA {
            return Err(format!("unsupported workflow schema: {}", self.schema));
        }
        if self.workflow_id.trim().is_empty() {
            return Err("workflow id is empty".to_string());
        }
        if self.objective.trim().is_empty() {
            return Err("workflow objective is empty".to_string());
        }
        if self.effort.trim().is_empty()
            || self.policy.trim().is_empty()
            || self.coordinator_model.trim().is_empty()
        {
            return Err("workflow routing metadata is incomplete".to_string());
        }
        if self.budget.max_steps == 0
            || self.budget.max_steps > MAX_ADAPTIVE_WORKFLOW_STEPS
            || self.budget.max_models == 0
            || self.budget.max_models > MAX_ADAPTIVE_WORKFLOW_AGENTS
            || self.budget.max_model_turns_per_step == 0
            || self.budget.max_output_tokens_per_step == 0
        {
            return Err("workflow budget is invalid".to_string());
        }
        if self.steps.len() > self.budget.max_steps {
            return Err("workflow exceeds its declared step budget".to_string());
        }
        let selected_models = self.steps.iter().map(|step| step.model.as_str()).collect::<BTreeSet<_>>();
        if selected_models.len() > self.budget.max_models {
            return Err("workflow exceeds its declared model budget".to_string());
        }
        if self.steps.iter().any(|step| {
            step.tool_policy == WorkflowToolPolicy::ReadOnlyEvidence
                && self.budget.max_tool_calls_per_step == 0
        }) {
            return Err("workflow enables evidence tools with a zero tool budget".to_string());
        }
        validate_adaptive_workflow(&self.adaptive_workflow(), allowed_models)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("workflow serialization failed: {error}"))
    }

    pub fn from_json(value: &str, allowed_models: &[String]) -> Result<Self, String> {
        let workflow = serde_json::from_str::<Self>(value)
            .map_err(|error| format!("workflow JSON is invalid: {error}"))?;
        workflow.validate(allowed_models)?;
        Ok(workflow)
    }
}

pub fn validate_adaptive_workflow(
    workflow: &AdaptiveWorkflow,
    allowed_models: &[String],
) -> Result<(), String> {
    if workflow.steps.is_empty() {
        return Err("adaptive workflow must contain at least one step".to_string());
    }
    if workflow.steps.len() > MAX_ADAPTIVE_WORKFLOW_STEPS {
        return Err(format!(
            "adaptive workflow exceeds the {MAX_ADAPTIVE_WORKFLOW_STEPS}-step budget"
        ));
    }

    let allowed_models = allowed_models.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut seen_ids = BTreeSet::new();
    let mut selected_models = BTreeSet::new();
    for step in &workflow.steps {
        if step.id.trim().is_empty() {
            return Err("adaptive workflow step id is empty".to_string());
        }
        if seen_ids.contains(step.id.as_str()) {
            return Err(format!("adaptive workflow step id is duplicated: {}", step.id));
        }
        if !allowed_models.contains(step.model.as_str()) {
            return Err(format!("adaptive workflow selected an unknown model: {}", step.model));
        }
        selected_models.insert(step.model.as_str());
        if step.subtask.trim().is_empty() {
            return Err(format!("adaptive workflow step {} has an empty subtask", step.id));
        }
        if !matches!(
            step.role.as_str(),
            "thinker" | "worker" | "verifier" | "synthesizer"
        ) {
            return Err(format!(
                "adaptive workflow step {} selected an unknown role: {}",
                step.id, step.role
            ));
        }

        let mut unique_access = BTreeSet::new();
        for dependency in &step.access {
            if !unique_access.insert(dependency.as_str()) {
                return Err(format!(
                    "adaptive workflow step {} repeats dependency {dependency}",
                    step.id
                ));
            }
            if !seen_ids.contains(dependency.as_str()) {
                return Err(format!(
                    "adaptive workflow step {} must only access earlier steps: {dependency}",
                    step.id
                ));
            }
        }
        seen_ids.insert(step.id.as_str());
    }
    if selected_models.len() > MAX_ADAPTIVE_WORKFLOW_AGENTS {
        return Err(format!(
            "adaptive workflow exceeds the {MAX_ADAPTIVE_WORKFLOW_AGENTS}-agent budget"
        ));
    }

    if workflow.steps.len() > 1
        && workflow
            .steps
            .last()
            .is_some_and(|step| step.access.is_empty())
    {
        return Err("the final adaptive workflow step must synthesize prior work".to_string());
    }
    if workflow
        .steps
        .last()
        .is_some_and(|step| step.role != "synthesizer")
    {
        return Err("the final adaptive workflow step must use the synthesizer role".to_string());
    }

    let indexes = workflow
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut included = BTreeSet::new();
    let mut pending = vec![workflow.steps.len() - 1];
    while let Some(index) = pending.pop() {
        if !included.insert(index) {
            continue;
        }
        for dependency in &workflow.steps[index].access {
            if let Some(dependency_index) = indexes.get(dependency.as_str()) {
                pending.push(*dependency_index);
            }
        }
    }
    if included.len() != workflow.steps.len() {
        let omitted = workflow
            .steps
            .iter()
            .enumerate()
            .filter(|(index, _)| !included.contains(index))
            .map(|(_, step)| step.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "the final adaptive workflow must incorporate every branch: {omitted}"
        ));
    }

    adaptive_workflow_layers(workflow)?;
    Ok(())
}

pub fn adaptive_workflow_layers(workflow: &AdaptiveWorkflow) -> Result<Vec<Vec<usize>>, String> {
    let mut indexes = BTreeMap::new();
    let mut step_layers = vec![0usize; workflow.steps.len()];
    for (step_index, step) in workflow.steps.iter().enumerate() {
        if indexes.contains_key(step.id.as_str()) {
            return Err(format!("adaptive workflow step id is duplicated: {}", step.id));
        }
        let mut layer = 0;
        for dependency in &step.access {
            let dependency_index = indexes.get(dependency.as_str()).copied().ok_or_else(|| {
                format!(
                    "adaptive workflow step {} must only access earlier steps: {dependency}",
                    step.id
                )
            })?;
            layer = layer.max(step_layers[dependency_index] + 1);
        }
        step_layers[step_index] = layer;
        indexes.insert(step.id.as_str(), step_index);
    }

    let mut layers = Vec::<Vec<usize>>::new();
    for (step_index, layer) in step_layers.into_iter().enumerate() {
        if layers.len() <= layer {
            layers.resize_with(layer + 1, Vec::new);
        }
        layers[layer].push(step_index);
    }
    Ok(layers)
}

pub fn adaptive_worker_prompt(
    workflow: &AdaptiveWorkflow,
    step_index: usize,
    user_prompt: &str,
    shared_memory: &str,
    outputs: &BTreeMap<String, String>,
) -> Option<String> {
    let step = workflow.steps.get(step_index)?;
    let role_instruction = match step.role.as_str() {
        "thinker" => "Explore an independent approach, decompose the problem, and expose assumptions.",
        "verifier" => "Audit supplied work against evidence, identify disagreements, and state exact corrections.",
        "synthesizer" => "Resolve disagreements and produce one checkable execution brief grounded in the supplied work.",
        _ => "Produce concrete work for the assigned subtask and report evidence and uncertainty.",
    };
    let mut prompt = format!(
        "You are isolated {} {} in a Cindx adaptive multi-model workflow. {} Complete only the assigned subtask. Do not assume you can see other agents unless their output is explicitly included below. Use exposed read-only evidence tools when the subtask depends on workspace facts. Return concrete findings for a later agent, not a user-facing answer.\n\nUser request:\n{}\n\nAssigned subtask:\n{}\n\nShared memory from earlier user turns:\n{}",
        step.role,
        step.id,
        role_instruction,
        user_prompt,
        step.subtask,
        if shared_memory.trim().is_empty() {
            "(none)"
        } else {
            shared_memory
        }
    );
    prompt.push_str("\n\nAuthorized prior step outputs:\n");
    if step.access.is_empty() {
        prompt.push_str("(none - work independently)\n");
    } else {
        for dependency in &step.access {
            let output = outputs.get(dependency)?;
            prompt.push_str(&format!("[{dependency}]\n{output}\n\n"));
        }
    }
    Some(prompt)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskClass {
    General,
    Coding,
    Research,
    Retrieval,
    Browser,
    Computer,
}

impl TaskClass {
    pub fn label(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Coding => "coding",
            Self::Research => "research",
            Self::Retrieval => "retrieval",
            Self::Browser => "browser",
            Self::Computer => "computer",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowExecutionTelemetry {
    pub task_class: TaskClass,
    pub plan: WorkflowPlanIr,
    pub succeeded: bool,
    pub quality_score: Option<f32>,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub fallback_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTopologyStep {
    pub role: String,
    pub model: String,
    pub access: Vec<usize>,
    pub tool_policy: WorkflowToolPolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowTopologyPrior {
    pub task_class: TaskClass,
    pub effort: String,
    pub max_models: usize,
    pub steps: Vec<WorkflowTopologyStep>,
    pub examples: usize,
    pub success_rate: f32,
    pub average_quality: Option<f32>,
    pub average_latency_ms: u64,
    pub average_total_tokens: u64,
    score: i64,
}

impl WorkflowTopologyPrior {
    pub fn prompt_hint(&self) -> String {
        let quality = self.average_quality
            .map(|score| format!("{score:.2}"))
            .unwrap_or_else(|| "unrated".to_string());
        let steps = self.steps.iter().enumerate().map(|(index, step)| {
            let access = if step.access.is_empty() {
                "none".to_string()
            } else {
                step.access.iter().map(|dependency| (dependency + 1).to_string())
                    .collect::<Vec<_>>().join(",")
            };
            format!(
                "{}. role={} model={} access={} tools={}",
                index + 1, step.role, step.model, access, step.tool_policy.label()
            )
        }).collect::<Vec<_>>().join("\n");
        format!(
            "Historical topology prior from {} comparable executions (success={:.0}%, quality={}, avg_latency_ms={}, avg_tokens={}). Treat this only as a prior: keep it when it fits the current query, otherwise design a better graph.\n{}",
            self.examples,
            self.success_rate * 100.0,
            quality,
            self.average_latency_ms,
            self.average_total_tokens,
            steps
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowSearchTeacher {
    priors: Vec<WorkflowTopologyPrior>,
}

impl WorkflowSearchTeacher {
    pub fn train(telemetry: &[WorkflowExecutionTelemetry]) -> Self {
        let mut grouped = BTreeMap::<
            (TaskClass, String, usize),
            BTreeMap<String, WorkflowPriorAccumulator>,
        >::new();
        for entry in telemetry.iter().filter(|entry| !entry.fallback_used) {
            let key = (
                entry.task_class.clone(),
                entry.plan.effort.clone(),
                entry.plan.budget.max_models,
            );
            grouped.entry(key).or_default()
                .entry(workflow_topology_signature(&entry.plan))
                .or_insert_with(|| WorkflowPriorAccumulator::new(&entry.plan))
                .record(entry);
        }

        let priors = grouped.into_iter().filter_map(|((task_class, effort, max_models), candidates)| {
            candidates.into_values()
                .map(|candidate| candidate.finish(task_class.clone(), effort.clone(), max_models))
                .max_by_key(|prior| prior.score)
        }).collect();
        Self { priors }
    }

    pub fn best_prior(
        &self,
        task_class: &TaskClass,
        effort: &str,
        allowed_models: &[String],
        max_models: usize,
    ) -> Option<&WorkflowTopologyPrior> {
        let allowed = allowed_models.iter().map(String::as_str).collect::<BTreeSet<_>>();
        self.priors.iter().filter(|prior| {
            &prior.task_class == task_class
                && prior.effort == effort
                && prior.max_models <= max_models
                && prior.examples >= 2
                && prior.success_rate >= 0.6
                && prior.steps.iter().all(|step| allowed.contains(step.model.as_str()))
        }).max_by_key(|prior| prior.score)
    }
}

#[derive(Debug, Clone)]
struct WorkflowPriorAccumulator {
    plan: WorkflowPlanIr,
    examples: usize,
    successes: usize,
    quality_total: f32,
    quality_examples: usize,
    latency_ms: u64,
    total_tokens: u64,
    tool_calls: u64,
}

impl WorkflowPriorAccumulator {
    fn new(plan: &WorkflowPlanIr) -> Self {
        Self {
            plan: plan.clone(), examples: 0, successes: 0, quality_total: 0.0,
            quality_examples: 0, latency_ms: 0, total_tokens: 0, tool_calls: 0,
        }
    }

    fn record(&mut self, telemetry: &WorkflowExecutionTelemetry) {
        self.examples += 1;
        self.successes += usize::from(telemetry.succeeded);
        if let Some(score) = telemetry.quality_score {
            self.quality_total += score.clamp(0.0, 1.0);
            self.quality_examples += 1;
        }
        self.latency_ms = self.latency_ms.saturating_add(telemetry.latency_ms);
        self.total_tokens = self.total_tokens.saturating_add(telemetry.total_tokens);
        self.tool_calls = self.tool_calls.saturating_add(telemetry.tool_calls);
    }

    fn finish(self, task_class: TaskClass, effort: String, max_models: usize) -> WorkflowTopologyPrior {
        let divisor = self.examples.max(1) as u64;
        let success_rate = self.successes as f32 / self.examples.max(1) as f32;
        let average_quality = (self.quality_examples > 0)
            .then(|| self.quality_total / self.quality_examples as f32);
        let average_latency_ms = self.latency_ms / divisor;
        let average_total_tokens = self.total_tokens / divisor;
        let quality = average_quality.unwrap_or(success_rate);
        let score = (success_rate * 10_000.0) as i64
            + (quality * 5_000.0) as i64
            - average_latency_ms as i64 / 100
            - average_total_tokens as i64 / 20
            - (self.tool_calls as f32 / self.examples.max(1) as f32 * 25.0) as i64;
        WorkflowTopologyPrior {
            task_class, effort, max_models,
            steps: normalized_topology_steps(&self.plan),
            examples: self.examples, success_rate, average_quality,
            average_latency_ms, average_total_tokens, score,
        }
    }
}

fn normalized_topology_steps(plan: &WorkflowPlanIr) -> Vec<WorkflowTopologyStep> {
    let indexes = plan.steps.iter().enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    plan.steps.iter().map(|step| WorkflowTopologyStep {
        role: step.role.clone(),
        model: step.model.clone(),
        access: step.access.iter()
            .filter_map(|dependency| indexes.get(dependency.as_str()).copied())
            .collect(),
        tool_policy: step.tool_policy.clone(),
    }).collect()
}

fn workflow_topology_signature(plan: &WorkflowPlanIr) -> String {
    normalized_topology_steps(plan).into_iter().map(|step| {
        format!(
            "{}:{}:[{}]:{}",
            step.role,
            step.model,
            step.access.iter().map(usize::to_string).collect::<Vec<_>>().join(","),
            step.tool_policy.label()
        )
    }).collect::<Vec<_>>().join("|")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCandidate {
    pub name: String,
    pub role: ModelRole,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub cost_tier: u8,
    pub latency_tier: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingContext {
    pub task_class: TaskClass,
    pub prompt_length: usize,
    pub needs_tools: bool,
    pub needs_retrieval: bool,
    pub needs_multi_model: bool,
    pub needs_vision: bool,
    pub high_stakes: bool,
    pub complexity_score: u8,
    pub estimated_steps: u8,
    pub parallelizable: bool,
    pub verification_required: bool,
    pub latency_sensitive: bool,
    pub user_policy_override: Option<OrchestrationPolicy>,
    pub model_candidates: Vec<ModelCandidate>,
}

impl RoutingContext {
    pub fn from_prompt(prompt: &str, model_candidates: Vec<ModelCandidate>) -> Self {
        let capability_question = is_capability_question(prompt);
        let task_class = classify_task(prompt);
        let coding_action = contains_any(
            prompt,
            &[
                "fix", "modify", "edit", "refactor", "implement", "debug", "compile", "test",
                "run", "修复", "修改", "重构", "实现", "调试", "编译", "测试", "运行", "执行",
                "定位", "排查",
            ],
        );
        let workspace_reference = contains_any(
            prompt,
            &[
                "workspace", "repo", "repository", "project", "codebase", "this file",
                "these files", "工作区", "仓库", "项目", "代码库", "这个文件", "这些文件",
                "现有代码",
            ],
        );
        let explicit_retrieval = contains_any(
            prompt,
            &[
                "rag", "search", "retrieve", "source", "sources", "citation", "docs", "搜索",
                "检索", "来源", "引用", "文档", "资料",
            ],
        );
        let explicit_multi_model = contains_any(
            prompt,
            &[
                "fugu",
                "multi-model",
                "multi model",
                "multiple models",
                "ensemble",
                "orchestration",
                "agent team",
                "协同",
                "协作",
                "多模型",
                "多个模型",
                "多智能体",
                "模型团队",
            ],
        );
        let high_stakes = contains_any(
            prompt,
            &[
                "security", "legal", "medical", "financial", "production", "migration",
                "critical", "high-stakes", "安全", "法律", "医疗", "财务", "生产", "迁移",
                "高风险", "关键",
            ],
        );
        let deep_analysis = contains_any(
            prompt,
            &[
                "compare", "tradeoff", "trade-off", "architecture", "strategy", "root cause",
                "investigate", "comprehensive", "alternatives", "方案", "比较", "对比", "权衡",
                "架构", "策略", "根因", "深入", "全面", "多条路径",
            ],
        );
        let parallelizable = !capability_question
            && contains_any(
                prompt,
                &[
                    "compare", "alternatives", "independent", "multiple options", "second opinion",
                    "cross-check", "parallel", "sources", "citations", "比较", "对比", "多个方案",
                    "independent analysis", "investigate", "root cause", "独立分析", "交叉验证",
                    "并行", "多条路径", "来源", "引用", "根因", "排查",
                ],
            );
        let multi_phase = !capability_question
            && contains_any(
                prompt,
                &[
                    " and then ", " then ", " after that ", "并且", "然后", "之后", "再运行",
                    "再检查", "同时", "and run tests", "fix and test", "implement and test",
                    "修改并", "修复并", "实现并", "排查并",
                ],
            );
        let latency_sensitive = !capability_question
            && contains_any(
                prompt,
                &[
                    "quick", "quickly", "fast", "brief", "one sentence", "简单回答", "快速",
                    "尽快", "一句话", "简短",
                ],
            );
        let needs_tools = !capability_question
            && (matches!(task_class, TaskClass::Browser | TaskClass::Computer)
                || coding_action
                || contains_any(
                    prompt,
                    &[
                        "tool", "file", "shell", "run", "edit", "工具", "文件", "运行", "执行",
                        "修改",
                    ],
                ));
        let needs_retrieval = !capability_question
            && (matches!(task_class, TaskClass::Retrieval)
                || explicit_retrieval
                || (workspace_reference
                    && matches!(task_class, TaskClass::Coding | TaskClass::Research))
                || (matches!(task_class, TaskClass::Coding) && coding_action));
        let needs_vision = !capability_question
            && (matches!(task_class, TaskClass::Computer)
                || contains_any(
                    prompt,
                    &[
                        "screenshot", "screen", "visible", "ui", "截图", "屏幕", "界面", "可见",
                    ],
                ));
        let verification_required = !capability_question
            && (high_stakes
                || coding_action
                || explicit_retrieval
                || contains_any(
                    prompt,
                    &[
                        "verify", "review", "validate", "check", "proof", "验证", "审查", "复核",
                        "检查", "证明",
                    ],
                ));
        let prompt_length = prompt.chars().count();
        let estimated_steps = if capability_question {
            1
        } else {
            (1u8
                + u8::from(needs_tools)
                + u8::from(needs_retrieval)
                + u8::from(verification_required)
                + u8::from(deep_analysis)
                + u8::from(multi_phase))
            .min(MAX_ADAPTIVE_WORKFLOW_STEPS as u8)
        };
        let mut complexity_score = 0u8;
        if explicit_multi_model {
            complexity_score += 3;
        }
        if matches!(task_class, TaskClass::Research | TaskClass::Retrieval) {
            complexity_score += 1;
        }
        if deep_analysis {
            complexity_score += 1;
        }
        if high_stakes {
            complexity_score += 1;
        }
        if needs_tools && needs_retrieval {
            complexity_score += 1;
        }
        if parallelizable && estimated_steps >= 4 {
            complexity_score += 1;
        }
        if multi_phase {
            complexity_score += 1;
        }
        if prompt_length >= 600 {
            complexity_score += 1;
        }
        Self {
            task_class: task_class.clone(),
            prompt_length,
            needs_tools,
            needs_retrieval,
            needs_multi_model: !capability_question && explicit_multi_model,
            needs_vision,
            high_stakes: !capability_question && high_stakes,
            complexity_score: if capability_question { 0 } else { complexity_score },
            estimated_steps,
            parallelizable,
            verification_required,
            latency_sensitive,
            user_policy_override: None,
            model_candidates,
        }
    }

    pub fn learning_signature(&self) -> String {
        format!(
            "{}:tools={}:retrieval={}:vision={}:high_stakes={}:complexity={}:steps={}:parallel={}:verify={}:latency={}",
            self.task_class.label(),
            u8::from(self.needs_tools),
            u8::from(self.needs_retrieval),
            u8::from(self.needs_vision),
            u8::from(self.high_stakes),
            (self.complexity_score / 2).min(3),
            self.estimated_steps.min(5),
            u8::from(self.parallelizable),
            u8::from(self.verification_required),
            u8::from(self.latency_sensitive),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingDecision {
    pub policy: OrchestrationPolicy,
    pub model: String,
    pub verifier_role: Option<ModelRole>,
    pub retrieval_mode: String,
    pub explanation: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingOutcome {
    Succeeded,
    Failed,
    UserRejected,
}

impl RoutingOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::UserRejected => "user_rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingTelemetry {
    pub task_class: TaskClass,
    pub context_signature: String,
    pub selected_policy: OrchestrationPolicy,
    pub selected_model: String,
    pub latency_ms: u64,
    pub outcome: RoutingOutcome,
    pub cost_proxy: u64,
    pub tool_count: u64,
    pub retrieval_count: u64,
    pub user_override: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LearnedRoute {
    pub task_class: TaskClass,
    pub policy: OrchestrationPolicy,
    pub model: String,
    pub examples: usize,
    pub success_rate: f32,
    pub average_latency_ms: u64,
    pub average_cost_proxy: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingEvaluationReport {
    pub examples: usize,
    pub baseline_policy: String,
    pub router_policy_matches_baseline: usize,
    pub router_policy_differs_from_baseline: usize,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingEvalCase {
    pub id: String,
    pub context: RoutingContext,
    pub expected_policy: OrchestrationPolicy,
    pub expected_retrieval_mode: String,
    pub expected_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingBenchmarkReport {
    pub cases: usize,
    pub passed: usize,
    pub over_orchestrated: usize,
    pub under_orchestrated: usize,
    pub failures: Vec<String>,
}

impl RoutingBenchmarkReport {
    pub fn pass_rate(&self) -> f32 {
        if self.cases == 0 {
            1.0
        } else {
            self.passed as f32 / self.cases as f32
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperationalEvaluationReport {
    pub runs: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub user_rejected: usize,
    pub success_rate: f32,
    pub average_latency_ms: u64,
    pub average_cost_proxy: u64,
    pub average_tool_calls: f32,
    pub average_retrievals: f32,
    pub policy_counts: BTreeMap<String, usize>,
    pub model_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityRubricScore {
    pub correctness: u8,
    pub evidence: u8,
    pub completion: u8,
    pub safety: u8,
}

impl QualityRubricScore {
    pub fn validate(&self) -> Result<(), String> {
        if [self.correctness, self.evidence, self.completion, self.safety]
            .into_iter()
            .all(|score| score <= 5)
        {
            Ok(())
        } else {
            Err("quality rubric dimensions must use a 0-5 score".to_string())
        }
    }

    pub fn total(&self) -> u8 {
        self.correctness
            .saturating_add(self.evidence)
            .saturating_add(self.completion)
            .saturating_add(self.safety)
    }

    pub fn passes(&self) -> bool {
        self.validate().is_ok()
            && self.total() >= 15
            && self.correctness >= 3
            && self.evidence >= 3
            && self.completion >= 3
            && self.safety >= 3
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuleBasedRouter;

fn is_lightweight_direct(context: &RoutingContext) -> bool {
    let length_limit = match context.task_class {
        TaskClass::General => 400,
        TaskClass::Coding => 240,
        _ => return false,
    };
    context.prompt_length < length_limit
        && !context.needs_tools
        && !context.needs_retrieval
        && !context.needs_vision
        && !context.verification_required
        && context.estimated_steps <= 2
}

fn requires_ultra(context: &RoutingContext) -> bool {
    context.needs_multi_model
        || (!context.latency_sensitive
            && !matches!(context.task_class, TaskClass::Browser | TaskClass::Computer)
            && ((context.high_stakes && context.complexity_score >= 3)
                || (context.parallelizable
                    && context.complexity_score >= 4
                    && context.estimated_steps >= 4)))
}

fn ultra_candidate_count(context: &RoutingContext) -> usize {
    if context.needs_multi_model || context.high_stakes || context.complexity_score >= 6 {
        3
    } else {
        2
    }
}

impl RuleBasedRouter {
    pub fn route(&self, context: &RoutingContext) -> RoutingDecision {
        if let Some(policy) = context
            .user_policy_override
            .as_ref()
            .filter(|policy| **policy != OrchestrationPolicy::AutoRouter)
        {
            return self.decision(
                context,
                policy.clone(),
                "user override selected an explicit policy",
            );
        }

        if requires_ultra(context) {
            return self.decision(
                context,
                OrchestrationPolicy::BestOfN {
                    candidates: ultra_candidate_count(context),
                },
                "request complexity benefits from bounded adaptive collaboration",
            );
        }

        let (policy, reason) = match context.task_class {
            TaskClass::Computer | TaskClass::Browser => (
                OrchestrationPolicy::PlanExecuteReview,
                "interactive tool use needs planning and review",
            ),
            TaskClass::Coding if is_lightweight_direct(context) => (
                OrchestrationPolicy::Single,
                "short coding question does not need tools or workspace context",
            ),
            TaskClass::Coding => (
                OrchestrationPolicy::PlanExecuteReview,
                "coding tasks benefit from plan-execute-review",
            ),
            TaskClass::Retrieval => (
                OrchestrationPolicy::PlanExecuteReview,
                "retrieval tasks need grounded synthesis and review",
            ),
            TaskClass::Research => (
                OrchestrationPolicy::PlanExecuteReview,
                "ordinary research needs one planned execution path",
            ),
            TaskClass::General if is_lightweight_direct(context) => {
                (OrchestrationPolicy::Single, "short general prompt can run directly")
            }
            TaskClass::General => (
                OrchestrationPolicy::PlanExecuteReview,
                "long or tool-adjacent prompt gets reviewed execution",
            ),
        };

        self.decision(context, policy, reason)
    }

    pub fn explain(&self, context: &RoutingContext) -> String {
        self.route(context).explanation
    }

    fn decision(
        &self,
        context: &RoutingContext,
        policy: OrchestrationPolicy,
        reason: &str,
    ) -> RoutingDecision {
        let preferred_role = preferred_model_role(context);
        let model = select_model(
            context,
            &preferred_role,
            !matches!(policy, OrchestrationPolicy::Single),
        );
        let retrieval_mode = match (&context.task_class, context.needs_retrieval) {
            (_, false) => "none".to_string(),
            (TaskClass::Coding, true) => "semantic_literal_parallel".to_string(),
            (_, true)
                if context.high_stakes
                    || context.parallelizable
                    || context.complexity_score >= 3 =>
            {
                "four_way_parallel".to_string()
            }
            (_, true) => "semantic_literal_parallel".to_string(),
        };
        let verifier_role = if matches!(
            policy,
            OrchestrationPolicy::PlanExecuteReview | OrchestrationPolicy::BestOfN { .. }
        ) {
            Some(ModelRole::Reviewer)
        } else {
            None
        };
        let mut metadata = Metadata::new();
        metadata.insert("task_class".to_string(), context.task_class.label().to_string());
        metadata.insert("needs_tools".to_string(), context.needs_tools.to_string());
        metadata.insert(
            "needs_retrieval".to_string(),
            context.needs_retrieval.to_string(),
        );
        metadata.insert(
            "needs_multi_model".to_string(),
            context.needs_multi_model.to_string(),
        );
        metadata.insert("needs_vision".to_string(), context.needs_vision.to_string());
        metadata.insert("high_stakes".to_string(), context.high_stakes.to_string());
        metadata.insert(
            "complexity_score".to_string(),
            context.complexity_score.to_string(),
        );
        metadata.insert(
            "estimated_steps".to_string(),
            context.estimated_steps.to_string(),
        );
        metadata.insert(
            "parallelizable".to_string(),
            context.parallelizable.to_string(),
        );
        metadata.insert(
            "verification_required".to_string(),
            context.verification_required.to_string(),
        );
        metadata.insert(
            "latency_sensitive".to_string(),
            context.latency_sensitive.to_string(),
        );
        metadata.insert(
            "selected_model_role".to_string(),
            role_label(&preferred_role).to_string(),
        );
        metadata.insert(
            "route_tier".to_string(),
            match &policy {
                OrchestrationPolicy::Single => "direct",
                OrchestrationPolicy::PlanExecuteReview => "planned",
                OrchestrationPolicy::BestOfN { .. } => "adaptive",
                OrchestrationPolicy::AutoRouter => "auto",
            }
            .to_string(),
        );
        metadata.insert(
            "collaboration_budget".to_string(),
            match &policy {
                OrchestrationPolicy::BestOfN { candidates } => *candidates,
                _ => 1,
            }
            .to_string(),
        );
        metadata.insert("router".to_string(), "rule_based_v2".to_string());

        RoutingDecision {
            policy,
            model,
            verifier_role,
            retrieval_mode,
            explanation: format!(
                "class={} complexity={} estimated_steps={} parallelizable={} verification={} latency_sensitive={} reason={reason}",
                context.task_class.label(),
                context.complexity_score,
                context.estimated_steps,
                context.parallelizable,
                context.verification_required,
                context.latency_sensitive,
            ),
            metadata,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LearnedModelRouter {
    routes: BTreeMap<String, LearnedRoute>,
    fallback: RuleBasedRouter,
}

impl LearnedModelRouter {
    pub fn train(telemetry: &[RoutingTelemetry]) -> Self {
        let mut grouped: BTreeMap<String, BTreeMap<(String, String), RouteAccumulator>> =
            BTreeMap::new();
        for entry in telemetry.iter().filter(|entry| !entry.user_override) {
            let key = (
                entry.selected_policy.label().to_string(),
                entry.selected_model.clone(),
            );
            grouped
                .entry(if entry.context_signature.trim().is_empty() {
                    entry.task_class.label().to_string()
                } else {
                    entry.context_signature.clone()
                })
                .or_default()
                .entry(key)
                .or_default()
                .record(entry);
        }

        let mut routes = BTreeMap::new();
        for (context_signature, candidates) in grouped {
            if let Some(((policy_label, model), accumulator)) = candidates
                .into_iter()
                .max_by(|(_, left), (_, right)| left.score().cmp(&right.score()))
            {
                if let Some(policy) = parse_policy(&policy_label) {
                    routes.insert(
                        context_signature,
                        LearnedRoute {
                            task_class: accumulator.task_class.clone().unwrap_or(TaskClass::General),
                            policy,
                            model,
                            examples: accumulator.examples,
                            success_rate: accumulator.success_rate(),
                            average_latency_ms: accumulator.average_latency_ms(),
                            average_cost_proxy: accumulator.average_cost_proxy(),
                        },
                    );
                }
            }
        }

        Self {
            routes,
            fallback: RuleBasedRouter,
        }
    }

    pub fn route(&self, context: &RoutingContext) -> RoutingDecision {
        if context
            .user_policy_override
            .as_ref()
            .is_some_and(|policy| *policy != OrchestrationPolicy::AutoRouter)
        {
            return self.fallback.route(context);
        }
        let baseline = self.fallback.route(context);
        if is_lightweight_direct(context) || requires_ultra(context) {
            return baseline;
        }
        let Some(route) = self
            .routes
            .get(&context.learning_signature())
            .or_else(|| self.routes.get(context.task_class.label()))
        else {
            return baseline;
        };
        if !learned_policy_allowed(context, &route.policy)
            || !context
                .model_candidates
                .iter()
                .any(|candidate| candidate.name == route.model)
        {
            return baseline;
        }
        let mut decision = self.fallback.decision(
            context,
            route.policy.clone(),
            "historical executions selected a better policy and model for this context",
        );
        decision.model = route.model.clone();
        decision.explanation = format!(
            "class={} learned_policy={} learned_model={} examples={} success_rate={:.2}",
            context.task_class.label(),
            route.policy.label(),
            route.model,
            route.examples,
            route.success_rate
        );
        decision
            .metadata
            .insert("router".to_string(), "learned_conductor_v1".to_string());
        decision
            .metadata
            .insert("learned_examples".to_string(), route.examples.to_string());
        decision.metadata.insert(
            "learned_success_rate".to_string(),
            format!("{:.3}", route.success_rate),
        );
        decision
    }

    pub fn learned_route(&self, task_class: &TaskClass) -> Option<&LearnedRoute> {
        self.routes
            .values()
            .filter(|route| &route.task_class == task_class)
            .max_by_key(|route| route.examples)
    }

    pub fn learned_route_for_context(&self, context: &RoutingContext) -> Option<&LearnedRoute> {
        self.routes
            .get(&context.learning_signature())
            .or_else(|| self.routes.get(context.task_class.label()))
    }
}

#[derive(Debug, Clone, Default)]
struct RouteAccumulator {
    task_class: Option<TaskClass>,
    examples: usize,
    successes: usize,
    latency_ms: u64,
    cost_proxy: u64,
}

impl RouteAccumulator {
    fn record(&mut self, telemetry: &RoutingTelemetry) {
        self.task_class = Some(telemetry.task_class.clone());
        self.examples += 1;
        if telemetry.outcome.is_success() {
            self.successes += 1;
        }
        self.latency_ms = self.latency_ms.saturating_add(telemetry.latency_ms);
        self.cost_proxy = self.cost_proxy.saturating_add(telemetry.cost_proxy);
    }

    fn success_rate(&self) -> f32 {
        if self.examples == 0 {
            0.0
        } else {
            self.successes as f32 / self.examples as f32
        }
    }

    fn average_latency_ms(&self) -> u64 {
        if self.examples == 0 {
            0
        } else {
            self.latency_ms / self.examples as u64
        }
    }

    fn average_cost_proxy(&self) -> u64 {
        if self.examples == 0 {
            0
        } else {
            self.cost_proxy / self.examples as u64
        }
    }

    fn score(&self) -> i64 {
        (self.successes as i64 * 10_000)
            - (self.average_cost_proxy() as i64)
            - (self.average_latency_ms() as i64 / 100)
    }
}

fn learned_policy_allowed(context: &RoutingContext, policy: &OrchestrationPolicy) -> bool {
    if is_lightweight_direct(context) {
        return *policy == OrchestrationPolicy::Single;
    }
    if requires_ultra(context) {
        return matches!(policy, OrchestrationPolicy::BestOfN { .. });
    }
    match policy {
        OrchestrationPolicy::Single => {
            !context.needs_tools
                && !context.needs_retrieval
                && !context.needs_vision
                && !context.high_stakes
                && !context.verification_required
        }
        OrchestrationPolicy::BestOfN { .. } => {
            !context.latency_sensitive && context.parallelizable && context.complexity_score >= 3
        }
        OrchestrationPolicy::PlanExecuteReview => true,
        OrchestrationPolicy::AutoRouter => false,
    }
}

fn is_capability_question(prompt: &str) -> bool {
    let normalized = prompt.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.chars().count() > 80 {
        return false;
    }
    if contains_any(
        &normalized,
        &[
            "fix", "modify", "edit", "refactor", "debug", "run", "test", "workspace",
            "repo", "project", "修复", "修改", "重构", "调试", "运行", "测试", "工作区",
            "仓库", "项目", "这个文件", "这些文件",
        ],
    ) {
        return false;
    }
    let compact = normalized
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let compact = compact.trim_end_matches(|character| {
        matches!(character, '?' | '？' | '。' | '!' | '！')
    });
    if [
        "你会不会",
        "你能不能",
        "你能否",
        "你是否会",
        "你是否能",
        "你有什么本领",
        "你能做什么",
        "你会做什么",
        "你擅长什么",
    ]
    .iter()
    .any(|prefix| compact.starts_with(prefix))
    {
        return true;
    }
    if compact
        .chars()
        .last()
        .is_some_and(|character| matches!(character, '吗' | '嘛' | '么'))
        && ["你会", "你能", "你可以", "你擅长"]
            .iter()
            .any(|prefix| compact.starts_with(prefix))
    {
        return true;
    }

    let words = normalized.split_whitespace().collect::<Vec<_>>();
    matches!(
        normalized.trim_end_matches(|character| matches!(character, '?' | '!' | '.')),
        "what can you do" | "what are you capable of" | "do you know how to code"
    ) || (words.len() <= 8
        && (normalized.starts_with("can you code")
            || normalized.starts_with("can you write code")
            || normalized.starts_with("are you able to code")))
}

pub fn classify_task(prompt: &str) -> TaskClass {
    if is_capability_question(prompt) {
        TaskClass::General
    } else if contains_any(
        prompt,
        &[
            "computer", "desktop", "screenshot", "screen", "click", "keyboard", "电脑", "桌面",
            "截图", "屏幕", "点击", "键盘",
        ],
    ) {
        TaskClass::Computer
    } else if contains_any(
        prompt,
        &[
            "browser", "webpage", "website", "scroll", "form", "浏览器", "网页", "网站", "滚动",
            "表单",
        ],
    ) {
        TaskClass::Browser
    } else if contains_any(
        prompt,
        &[
            "rag", "retrieve", "search", "source", "sources", "citation", "docs", "检索", "搜索",
            "来源", "引用", "文档", "资料",
        ],
    ) {
        TaskClass::Retrieval
    } else if contains_any(
        prompt,
        &[
            "code", "rust", "typescript", "file", "test", "compile", "bug", "代码", "编程", "文件",
            "测试", "编译", "错误", "修复", "重构", "实现",
        ],
    ) {
        TaskClass::Coding
    } else if contains_any(
        prompt,
        &[
            "research", "compare", "investigate", "latest", "study", "collaboration", "multi-model",
            "multiple models", "orchestration", "orchestrator", "fugu", "reproduce", "replicate",
            "研究", "调研", "比较", "对比", "分析", "调查", "最新", "评估", "对标", "协同",
            "协作", "多模型", "多个模型", "复现",
        ],
    ) {
        TaskClass::Research
    } else {
        TaskClass::General
    }
}

pub fn evaluate_router_against_baseline(
    router: &LearnedModelRouter,
    contexts: &[RoutingContext],
    baseline_policy: OrchestrationPolicy,
) -> RoutingEvaluationReport {
    let mut matches_baseline = 0;
    for context in contexts {
        if router.route(context).policy.label() == baseline_policy.label() {
            matches_baseline += 1;
        }
    }
    let differs = contexts.len().saturating_sub(matches_baseline);

    RoutingEvaluationReport {
        examples: contexts.len(),
        baseline_policy: baseline_policy.label().to_string(),
        router_policy_matches_baseline: matches_baseline,
        router_policy_differs_from_baseline: differs,
        summary: format!(
            "Compared {} router decisions against baseline {}: {} same, {} different.",
            contexts.len(),
            baseline_policy.label(),
            matches_baseline,
            differs
        ),
    }
}

pub fn evaluate_routing_cases(cases: &[RoutingEvalCase]) -> RoutingBenchmarkReport {
    let router = RuleBasedRouter;
    let mut report = RoutingBenchmarkReport {
        cases: cases.len(),
        passed: 0,
        over_orchestrated: 0,
        under_orchestrated: 0,
        failures: Vec::new(),
    };
    for case in cases {
        let decision = router.route(&case.context);
        let policy_matches = decision.policy == case.expected_policy;
        let retrieval_matches = decision.retrieval_mode == case.expected_retrieval_mode;
        let model_matches = case
            .expected_model
            .as_ref()
            .map(|model| model == &decision.model)
            .unwrap_or(true);
        if policy_matches && retrieval_matches && model_matches {
            report.passed += 1;
            continue;
        }

        let actual_load = orchestration_load(&decision.policy);
        let expected_load = orchestration_load(&case.expected_policy);
        if actual_load > expected_load {
            report.over_orchestrated += 1;
        } else if actual_load < expected_load {
            report.under_orchestrated += 1;
        }
        report.failures.push(format!(
            "{} expected policy={} retrieval={} model={} but got policy={} retrieval={} model={}",
            case.id,
            case.expected_policy.label(),
            case.expected_retrieval_mode,
            case.expected_model.as_deref().unwrap_or("(any)"),
            decision.policy.label(),
            decision.retrieval_mode,
            decision.model
        ));
    }
    report
}

pub fn evaluate_routing_telemetry(
    telemetry: &[RoutingTelemetry],
) -> OperationalEvaluationReport {
    let runs = telemetry.len();
    let succeeded = telemetry
        .iter()
        .filter(|entry| entry.outcome == RoutingOutcome::Succeeded)
        .count();
    let failed = telemetry
        .iter()
        .filter(|entry| entry.outcome == RoutingOutcome::Failed)
        .count();
    let user_rejected = telemetry
        .iter()
        .filter(|entry| entry.outcome == RoutingOutcome::UserRejected)
        .count();
    let total_latency = telemetry
        .iter()
        .map(|entry| entry.latency_ms)
        .sum::<u64>();
    let total_cost = telemetry
        .iter()
        .map(|entry| entry.cost_proxy)
        .sum::<u64>();
    let total_tools = telemetry.iter().map(|entry| entry.tool_count).sum::<u64>();
    let total_retrievals = telemetry
        .iter()
        .map(|entry| entry.retrieval_count)
        .sum::<u64>();
    let mut policy_counts = BTreeMap::new();
    let mut model_counts = BTreeMap::new();
    for entry in telemetry {
        *policy_counts
            .entry(entry.selected_policy.label().to_string())
            .or_default() += 1;
        *model_counts.entry(entry.selected_model.clone()).or_default() += 1;
    }
    let divisor = runs.max(1) as u64;
    OperationalEvaluationReport {
        runs,
        succeeded,
        failed,
        user_rejected,
        success_rate: if runs == 0 {
            0.0
        } else {
            succeeded as f32 / runs as f32
        },
        average_latency_ms: total_latency / divisor,
        average_cost_proxy: total_cost / divisor,
        average_tool_calls: total_tools as f32 / divisor as f32,
        average_retrievals: total_retrievals as f32 / divisor as f32,
        policy_counts,
        model_counts,
    }
}

fn orchestration_load(policy: &OrchestrationPolicy) -> usize {
    match policy {
        OrchestrationPolicy::Single => 1,
        OrchestrationPolicy::PlanExecuteReview => 2,
        OrchestrationPolicy::BestOfN { candidates } => 2 + candidates,
        OrchestrationPolicy::AutoRouter => 1,
    }
}

fn preferred_model_role(context: &RoutingContext) -> ModelRole {
    if context.high_stakes {
        return ModelRole::Reviewer;
    }
    match context.task_class {
        TaskClass::Research | TaskClass::Retrieval => ModelRole::Planner,
        TaskClass::General | TaskClass::Coding | TaskClass::Browser | TaskClass::Computer => {
            ModelRole::Executor
        }
    }
}

fn select_model(
    context: &RoutingContext,
    preferred_role: &ModelRole,
    prefer_strong: bool,
) -> String {
    let mut candidates = context
        .model_candidates
        .iter()
        .filter(|candidate| !context.needs_tools || candidate.supports_tools)
        .filter(|candidate| !context.needs_vision || candidate.supports_vision)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        candidates = context.model_candidates.iter().collect();
    }
    let role_candidates = candidates
        .iter()
        .copied()
        .filter(|candidate| &candidate.role == preferred_role)
        .collect::<Vec<_>>();
    if !role_candidates.is_empty() {
        candidates = role_candidates;
    }
    if candidates.is_empty() {
        return "executor".to_string();
    }
    candidates.sort_by(|left, right| {
        if prefer_strong {
            right
                .cost_tier
                .cmp(&left.cost_tier)
                .then_with(|| left.latency_tier.cmp(&right.latency_tier))
        } else {
            left.latency_tier
                .cmp(&right.latency_tier)
                .then_with(|| left.cost_tier.cmp(&right.cost_tier))
        }
    });
    candidates
        .first()
        .map(|candidate| candidate.name.clone())
        .unwrap_or_else(|| "executor".to_string())
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    let normalized = value.to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| contains_keyword(&normalized, needle))
}

fn contains_keyword(normalized: &str, needle: &str) -> bool {
    let needle = needle.to_ascii_lowercase();
    if needle.chars().all(|character| character.is_ascii_alphanumeric()) {
        normalized
            .split(|character: char| !character.is_alphanumeric())
            .any(|token| token == needle)
    } else {
        normalized.contains(&needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<ModelCandidate> {
        vec![
            ModelCandidate {
                name: "fast-mini".to_string(),
                role: ModelRole::Executor,
                supports_tools: true,
                supports_vision: false,
                cost_tier: 1,
                latency_tier: 1,
            },
            ModelCandidate {
                name: "strong-vision".to_string(),
                role: ModelRole::Executor,
                supports_tools: true,
                supports_vision: true,
                cost_tier: 4,
                latency_tier: 3,
            },
        ]
    }

    fn workflow_plan(workflow_id: &str, with_verifier: bool) -> WorkflowPlanIr {
        let mut steps = vec![
            AdaptiveWorkflowStep {
                id: "approach_a".to_string(),
                role: "thinker".to_string(),
                model: "planner".to_string(),
                subtask: "develop the primary approach".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "approach_b".to_string(),
                role: "worker".to_string(),
                model: "reviewer".to_string(),
                subtask: "develop an independent alternative".to_string(),
                access: Vec::new(),
            },
        ];
        if with_verifier {
            steps.push(AdaptiveWorkflowStep {
                id: "verify".to_string(),
                role: "verifier".to_string(),
                model: "reviewer".to_string(),
                subtask: "cross-check both approaches".to_string(),
                access: vec!["approach_a".to_string(), "approach_b".to_string()],
            });
        }
        steps.push(AdaptiveWorkflowStep {
            id: "synthesize".to_string(),
            role: "synthesizer".to_string(),
            model: "planner".to_string(),
            subtask: "produce one execution brief".to_string(),
            access: if with_verifier {
                vec!["approach_a".to_string(), "approach_b".to_string(), "verify".to_string()]
            } else {
                vec!["approach_a".to_string(), "approach_b".to_string()]
            },
        });
        WorkflowPlanIr::from_adaptive(
            workflow_id,
            "Compare two implementation strategies",
            "pro",
            "best_of_n",
            "planner",
            &AdaptiveWorkflow { steps },
            WorkflowBudget {
                max_steps: 5,
                max_models: 2,
                max_model_turns_per_step: 5,
                max_tool_calls_per_step: 6,
                max_output_tokens_per_step: 4_096,
            },
        )
    }

    #[test]
    fn workflow_ir_round_trips_and_enforces_declared_budgets() {
        let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
        let plan = workflow_plan("workflow-1", false);
        plan.validate(&allowed_models).expect("workflow should be valid");

        let json = plan.to_json().expect("workflow should serialize");
        assert_eq!(WorkflowPlanIr::from_json(&json, &allowed_models).unwrap(), plan);

        let mut invalid = plan;
        invalid.budget.max_steps = 2;
        assert_eq!(
            invalid.validate(&allowed_models),
            Err("workflow exceeds its declared step budget".to_string())
        );
    }

    #[test]
    fn search_teacher_prefers_reliable_efficient_topology() {
        let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
        let fast_plan = workflow_plan("fast-1", false);
        let slow_plan = workflow_plan("slow-1", true);
        let telemetry = vec![
            WorkflowExecutionTelemetry {
                task_class: TaskClass::Research,
                plan: fast_plan.clone(),
                succeeded: true,
                quality_score: Some(0.92),
                latency_ms: 4_000,
                total_tokens: 4_000,
                tool_calls: 2,
                fallback_used: false,
            },
            WorkflowExecutionTelemetry {
                task_class: TaskClass::Research,
                plan: fast_plan,
                succeeded: true,
                quality_score: Some(0.88),
                latency_ms: 5_000,
                total_tokens: 5_000,
                tool_calls: 2,
                fallback_used: false,
            },
            WorkflowExecutionTelemetry {
                task_class: TaskClass::Research,
                plan: slow_plan.clone(),
                succeeded: false,
                quality_score: Some(0.30),
                latency_ms: 80_000,
                total_tokens: 20_000,
                tool_calls: 8,
                fallback_used: false,
            },
            WorkflowExecutionTelemetry {
                task_class: TaskClass::Research,
                plan: slow_plan,
                succeeded: true,
                quality_score: Some(0.45),
                latency_ms: 70_000,
                total_tokens: 18_000,
                tool_calls: 7,
                fallback_used: false,
            },
        ];

        let teacher = WorkflowSearchTeacher::train(&telemetry);
        let prior = teacher
            .best_prior(&TaskClass::Research, "pro", &allowed_models, 2)
            .expect("a stable prior should be available");
        assert_eq!(prior.steps.len(), 3);
        assert_eq!(prior.examples, 2);
        assert_eq!(prior.success_rate, 1.0);
        assert!(prior.prompt_hint().contains("Treat this only as a prior"));
    }

    #[test]
    fn labels_are_stable_for_persisted_traces() {
        assert_eq!(OrchestrationPolicy::Single.label(), "single");
        assert_eq!(
            OrchestrationPolicy::PlanExecuteReview.label(),
            "plan_execute_review"
        );
        assert_eq!(
            OrchestrationPolicy::BestOfN { candidates: 3 }.label(),
            "best_of_n"
        );
        assert_eq!(OrchestrationPolicy::AutoRouter.label(), "auto_router");
    }

    #[test]
    fn plan_execute_review_has_expected_roles() {
        let plan = default_plan(OrchestrationPolicy::PlanExecuteReview);
        let roles: Vec<ModelRole> = plan.steps.into_iter().map(|step| step.role).collect();

        assert_eq!(
            roles,
            vec![ModelRole::Planner, ModelRole::Executor, ModelRole::Reviewer]
        );
    }

    #[test]
    fn parses_policy_labels() {
        assert_eq!(parse_policy("single"), Some(OrchestrationPolicy::Single));
        assert_eq!(
            parse_policy("plan_execute_review"),
            Some(OrchestrationPolicy::PlanExecuteReview)
        );
        assert_eq!(
            parse_policy("best_of_n"),
            Some(OrchestrationPolicy::BestOfN { candidates: 3 })
        );
        assert_eq!(parse_policy("auto_router"), Some(OrchestrationPolicy::AutoRouter));
        assert_eq!(parse_policy("unknown"), None);
    }

    #[test]
    fn step_prompt_includes_previous_outputs() {
        let plan = default_plan(OrchestrationPolicy::PlanExecuteReview);
        let prompt = step_prompt(
            &plan,
            1,
            "Change README",
            &["Plan: inspect first".to_string()],
        )
        .expect("step should exist");

        assert!(prompt.contains("Role: executor"));
        assert!(prompt.contains("Change README"));
        assert!(prompt.contains("Plan: inspect first"));
    }

    #[test]
    fn adaptive_workflow_builds_parallel_dependency_layers() {
        let workflow = AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "research".to_string(),
                    role: "thinker".to_string(),
                    model: "strong-vision".to_string(),
                    subtask: "Research the primary approach.".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "challenge".to_string(),
                    role: "thinker".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "Find independent failure modes.".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "verify".to_string(),
                    role: "verifier".to_string(),
                    model: "strong-vision".to_string(),
                    subtask: "Verify the research.".to_string(),
                    access: vec!["research".to_string()],
                },
                AdaptiveWorkflowStep {
                    id: "synthesize".to_string(),
                    role: "synthesizer".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "Produce an execution brief.".to_string(),
                    access: vec!["challenge".to_string(), "verify".to_string()],
                },
            ],
        };

        validate_adaptive_workflow(
            &workflow,
            &["fast-mini".to_string(), "strong-vision".to_string()],
        )
        .expect("workflow should be valid");
        assert_eq!(
            adaptive_workflow_layers(&workflow).expect("layers should build"),
            vec![vec![0, 1], vec![2], vec![3]]
        );
    }

    #[test]
    fn fugu_step_budget_reuses_a_bounded_worker_pool() {
        assert_eq!(adaptive_workflow_step_budget(1), 1);
        assert_eq!(adaptive_workflow_step_budget(2), 3);
        assert_eq!(adaptive_workflow_step_budget(3), 5);
        assert_eq!(adaptive_workflow_step_budget(99), 5);
    }

    #[test]
    fn adaptive_workflow_rejects_a_branch_omitted_from_synthesis() {
        let workflow = AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "included".to_string(),
                    role: "thinker".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "Included branch".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "orphaned".to_string(),
                    role: "worker".to_string(),
                    model: "strong-vision".to_string(),
                    subtask: "Contradicting branch".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "Synthesize".to_string(),
                    access: vec!["included".to_string()],
                },
            ],
        };

        let error = validate_adaptive_workflow(
            &workflow,
            &["fast-mini".to_string(), "strong-vision".to_string()],
        )
        .expect_err("orphaned branch should be rejected");

        assert!(error.contains("incorporate every branch"));
        assert!(error.contains("orphaned"));
    }

    #[test]
    fn adaptive_worker_only_receives_authorized_outputs() {
        let workflow = AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "allowed".to_string(),
                    role: "thinker".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "First branch.".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "isolated".to_string(),
                    role: "worker".to_string(),
                    model: "strong-vision".to_string(),
                    subtask: "Independent branch.".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "consumer".to_string(),
                    role: "synthesizer".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "Use one branch.".to_string(),
                    access: vec!["allowed".to_string()],
                },
            ],
        };
        let outputs = [
            ("allowed".to_string(), "VISIBLE_FINDING".to_string()),
            ("isolated".to_string(), "HIDDEN_FINDING".to_string()),
        ]
        .into_iter()
        .collect();

        let prompt = adaptive_worker_prompt(&workflow, 2, "Investigate", "prior turn", &outputs)
            .expect("worker prompt should build");

        assert!(prompt.contains("VISIBLE_FINDING"));
        assert!(!prompt.contains("HIDDEN_FINDING"));
    }

    #[test]
    fn adaptive_workflow_rejects_forward_access_and_unknown_models() {
        let workflow = AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "first".to_string(),
                    role: "thinker".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "Try to read the future.".to_string(),
                    access: vec!["later".to_string()],
                },
                AdaptiveWorkflowStep {
                    id: "later".to_string(),
                    role: "synthesizer".to_string(),
                    model: "fast-mini".to_string(),
                    subtask: "Later work.".to_string(),
                    access: vec!["first".to_string()],
                },
            ],
        };

        let error = validate_adaptive_workflow(&workflow, &["fast-mini".to_string()])
            .expect_err("workflow should be rejected");
        assert!(error.contains("only access earlier steps"));

        let mut unknown_model_workflow = workflow;
        unknown_model_workflow.steps[0].access.clear();
        unknown_model_workflow.steps[0].model = "unknown".to_string();
        let error = validate_adaptive_workflow(
            &unknown_model_workflow,
            &["fast-mini".to_string()],
        )
        .expect_err("unknown model should be rejected");
        assert!(error.contains("unknown model"));
    }

    #[test]
    fn adaptive_workflow_rejects_more_than_three_models() {
        let workflow = AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "one".to_string(),
                    role: "thinker".to_string(),
                    model: "model-a".to_string(),
                    subtask: "First branch.".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "two".to_string(),
                    role: "worker".to_string(),
                    model: "model-b".to_string(),
                    subtask: "Second branch.".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "three".to_string(),
                    role: "verifier".to_string(),
                    model: "model-c".to_string(),
                    subtask: "Audit both branches.".to_string(),
                    access: vec!["one".to_string(), "two".to_string()],
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "model-d".to_string(),
                    subtask: "Synthesize the result.".to_string(),
                    access: vec!["one".to_string(), "two".to_string(), "three".to_string()],
                },
            ],
        };
        let models = ["model-a", "model-b", "model-c", "model-d"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        let error = validate_adaptive_workflow(&workflow, &models)
            .expect_err("four models should exceed the bounded agent budget");

        assert!(error.contains("3-agent budget"));
    }

    #[test]
    fn rule_router_explains_four_way_retrieval_choice() {
        let context = RoutingContext::from_prompt("Search the docs with RAG and cite sources", candidates());
        let router = RuleBasedRouter;
        let decision = router.route(&context);

        assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
        assert_eq!(decision.retrieval_mode, "four_way_parallel");
        assert!(decision.explanation.contains("class=retrieval"));
    }

    #[test]
    fn chinese_collaboration_request_routes_to_real_ensemble() {
        let context = RoutingContext::from_prompt(
            "分析多个模型协同，并对标 Sakana Fugu Ultra",
            candidates(),
        );
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(context.task_class, TaskClass::Research);
        assert!(context.needs_multi_model);
        assert_eq!(
            decision.policy,
            OrchestrationPolicy::BestOfN { candidates: 3 }
        );
    }

    #[test]
    fn fugu_reproduction_request_routes_to_adaptive_ensemble() {
        let context = RoutingContext::from_prompt(
            "继续完善 Sakana Fugu Ultra 的复现",
            candidates(),
        );
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(context.task_class, TaskClass::Research);
        assert!(context.needs_multi_model);
        assert_eq!(
            decision.policy,
            OrchestrationPolicy::BestOfN { candidates: 3 }
        );
    }

    #[test]
    fn ordinary_research_uses_one_planned_execution_path() {
        let context = RoutingContext::from_prompt("比较两个产品路线的优缺点", candidates());
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(context.task_class, TaskClass::Research);
        assert!(!context.needs_multi_model);
        assert!(!context.needs_retrieval);
        assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
    }

    #[test]
    fn high_stakes_architecture_work_routes_to_three_experts() {
        let context = RoutingContext::from_prompt(
            "Compare production migration architectures, investigate root causes, and propose a safe strategy",
            candidates(),
        );
        let decision = RuleBasedRouter.route(&context);

        assert!(context.high_stakes);
        assert!(context.complexity_score >= 3);
        assert_eq!(decision.policy, OrchestrationPolicy::BestOfN { candidates: 3 });
    }

    #[test]
    fn complex_non_critical_work_routes_to_two_experts() {
        let context = RoutingContext::from_prompt(
            "Investigate the root cause in this project, edit the files, and run tests",
            candidates(),
        );
        let decision = RuleBasedRouter.route(&context);

        assert!(!context.high_stakes);
        assert!(context.complexity_score >= 3);
        assert_eq!(decision.policy, OrchestrationPolicy::BestOfN { candidates: 2 });
    }

    #[test]
    fn chinese_tool_request_is_not_misclassified_as_general_chat() {
        let context = RoutingContext::from_prompt("你能不能修复这个项目并运行测试", candidates());
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(context.task_class, TaskClass::Coding);
        assert!(context.needs_tools);
        assert!(context.needs_retrieval);
        assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
        assert_eq!(decision.retrieval_mode, "semantic_literal_parallel");
    }

    #[test]
    fn coding_capability_question_stays_direct_without_retrieval() {
        let context = RoutingContext::from_prompt("你会不会写代码", candidates());
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(context.task_class, TaskClass::General);
        assert!(!context.needs_tools);
        assert!(!context.needs_retrieval);
        assert!(!context.needs_vision);
        assert_eq!(decision.policy, OrchestrationPolicy::Single);
        assert_eq!(decision.retrieval_mode, "none");
    }

    #[test]
    fn short_coding_explanation_does_not_require_workspace_context() {
        let context = RoutingContext::from_prompt("解释一下 Rust 所有权代码", candidates());
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(context.task_class, TaskClass::Coding);
        assert!(!context.needs_tools);
        assert!(!context.needs_retrieval);
        assert_eq!(decision.policy, OrchestrationPolicy::Single);
    }

    #[test]
    fn rule_router_respects_user_override() {
        let mut context = RoutingContext::from_prompt("Research three implementation options", candidates());
        context.user_policy_override = Some(OrchestrationPolicy::Single);
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(decision.policy, OrchestrationPolicy::Single);
        assert!(decision.explanation.contains("user override"));
    }

    #[test]
    fn learned_router_uses_successful_trace_table() {
        let context = RoutingContext::from_prompt(
            "Research and compare local agent routers",
            candidates(),
        );
        let context_signature = context.learning_signature();
        let telemetry = vec![
            RoutingTelemetry {
                task_class: TaskClass::Research,
                context_signature: context_signature.clone(),
                selected_policy: OrchestrationPolicy::PlanExecuteReview,
                selected_model: "strong-vision".to_string(),
                latency_ms: 900,
                outcome: RoutingOutcome::Succeeded,
                cost_proxy: 120,
                tool_count: 0,
                retrieval_count: 2,
                user_override: false,
            },
            RoutingTelemetry {
                task_class: TaskClass::Research,
                context_signature,
                selected_policy: OrchestrationPolicy::Single,
                selected_model: "fast-mini".to_string(),
                latency_ms: 200,
                outcome: RoutingOutcome::Failed,
                cost_proxy: 20,
                tool_count: 0,
                retrieval_count: 0,
                user_override: false,
            },
        ];
        let router = LearnedModelRouter::train(&telemetry);
        let decision = router.route(&context);

        assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
        assert_eq!(decision.model, "strong-vision");
        assert!(decision.explanation.contains("policy=plan_execute_review"));
        assert!(decision.explanation.contains("learned_model=strong-vision"));
        assert_eq!(
            decision.metadata.get("router").map(String::as_str),
            Some("learned_conductor_v1")
        );
    }

    #[test]
    fn learned_router_can_downshift_a_matching_context_without_tools() {
        let prompt = "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
        let context = RoutingContext::from_prompt(&prompt, candidates());
        assert_eq!(RuleBasedRouter.route(&context).policy, OrchestrationPolicy::PlanExecuteReview);
        let telemetry = (0..3).map(|_| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: context.learning_signature(),
            selected_policy: OrchestrationPolicy::Single,
            selected_model: "fast-mini".to_string(),
            latency_ms: 250,
            outcome: RoutingOutcome::Succeeded,
            cost_proxy: 80,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        }).collect::<Vec<_>>();

        let decision = LearnedModelRouter::train(&telemetry).route(&context);
        assert_eq!(decision.policy, OrchestrationPolicy::Single);
        assert_eq!(decision.model, "fast-mini");
        assert!(decision.explanation.contains("learned_policy=single"));
    }

    #[test]
    fn latency_sensitive_complex_request_does_not_spawn_an_ensemble() {
        let context = RoutingContext::from_prompt(
            "Quickly compare implementation alternatives and check the project",
            candidates(),
        );
        let decision = RuleBasedRouter.route(&context);

        assert!(context.latency_sensitive);
        assert!(context.parallelizable);
        assert_ne!(decision.policy, OrchestrationPolicy::BestOfN { candidates: 2 });
        assert_ne!(decision.policy, OrchestrationPolicy::BestOfN { candidates: 3 });
    }

    #[test]
    fn auto_router_selects_models_by_task_role() {
        let role_candidates = vec![
            ModelCandidate {
                name: "planner-model".to_string(),
                role: ModelRole::Planner,
                supports_tools: true,
                supports_vision: true,
                cost_tier: 3,
                latency_tier: 2,
            },
            ModelCandidate {
                name: "executor-model".to_string(),
                role: ModelRole::Executor,
                supports_tools: true,
                supports_vision: true,
                cost_tier: 2,
                latency_tier: 1,
            },
            ModelCandidate {
                name: "reviewer-model".to_string(),
                role: ModelRole::Reviewer,
                supports_tools: true,
                supports_vision: true,
                cost_tier: 4,
                latency_tier: 3,
            },
        ];
        let research = RoutingContext::from_prompt(
            "Research and compare local agent architectures",
            role_candidates.clone(),
        );
        let high_stakes = RoutingContext::from_prompt(
            "Review a critical production migration strategy",
            role_candidates,
        );

        assert_eq!(RuleBasedRouter.route(&research).model, "planner-model");
        assert_eq!(RuleBasedRouter.route(&high_stakes).model, "reviewer-model");
    }

    #[test]
    fn learned_router_cannot_upgrade_a_lightweight_coding_question() {
        let context = RoutingContext::from_prompt("解释一下 Rust 所有权代码", candidates());
        let router = LearnedModelRouter::train(&[RoutingTelemetry {
            task_class: TaskClass::Coding,
            context_signature: context.learning_signature(),
            selected_policy: OrchestrationPolicy::PlanExecuteReview,
            selected_model: "strong-vision".to_string(),
            latency_ms: 10_000,
            outcome: RoutingOutcome::Succeeded,
            cost_proxy: 1_000,
            tool_count: 4,
            retrieval_count: 4,
            user_override: false,
        }]);
        let decision = router.route(&context);

        assert_eq!(decision.policy, OrchestrationPolicy::Single);
        assert!(!decision.explanation.contains("learned_policy"));
    }

    #[test]
    fn learned_router_cannot_upgrade_ordinary_research_to_ultra() {
        let context = RoutingContext::from_prompt("比较两个产品路线的优缺点", candidates());
        let router = LearnedModelRouter::train(&[RoutingTelemetry {
            task_class: TaskClass::Research,
            context_signature: context.learning_signature(),
            selected_policy: OrchestrationPolicy::BestOfN { candidates: 3 },
            selected_model: "strong-vision".to_string(),
            latency_ms: 30_000,
            outcome: RoutingOutcome::Succeeded,
            cost_proxy: 4_000,
            tool_count: 0,
            retrieval_count: 4,
            user_override: false,
        }]);
        let decision = router.route(&context);

        assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
        assert!(!decision.explanation.contains("learned_policy"));
    }

    #[test]
    fn evaluation_report_compares_against_baseline_policy() {
        let router = LearnedModelRouter::train(&[]);
        let contexts = vec![
            RoutingContext::from_prompt("hello", candidates()),
            RoutingContext::from_prompt("Use computer screenshot to inspect the UI", candidates()),
        ];
        let report =
            evaluate_router_against_baseline(&router, &contexts, OrchestrationPolicy::Single);

        assert_eq!(report.examples, 2);
        assert_eq!(report.baseline_policy, "single");
        assert_eq!(report.router_policy_matches_baseline, 1);
        assert_eq!(report.router_policy_differs_from_baseline, 1);
        assert!(report.summary.contains("baseline single"));
    }

    #[test]
    fn evaluation_lab_detects_over_and_under_orchestration() {
        let cases = vec![
            RoutingEvalCase {
                id: "under".to_string(),
                context: RoutingContext::from_prompt("hello", candidates()),
                expected_policy: OrchestrationPolicy::BestOfN { candidates: 3 },
                expected_retrieval_mode: "none".to_string(),
                expected_model: None,
            },
            RoutingEvalCase {
                id: "over".to_string(),
                context: RoutingContext::from_prompt(
                    "Use multiple models to reproduce Fugu Ultra",
                    candidates(),
                ),
                expected_policy: OrchestrationPolicy::Single,
                expected_retrieval_mode: "none".to_string(),
                expected_model: None,
            },
        ];

        let report = evaluate_routing_cases(&cases);

        assert_eq!(report.cases, 2);
        assert_eq!(report.passed, 0);
        assert_eq!(report.over_orchestrated, 1);
        assert_eq!(report.under_orchestrated, 1);
        assert_eq!(report.failures.len(), 2);
    }

    #[test]
    fn operational_evaluation_aggregates_trace_cost_and_outcomes() {
        let telemetry = vec![
            RoutingTelemetry {
                task_class: TaskClass::Coding,
                context_signature: "coding".to_string(),
                selected_policy: OrchestrationPolicy::Single,
                selected_model: "fast-mini".to_string(),
                latency_ms: 100,
                outcome: RoutingOutcome::Succeeded,
                cost_proxy: 20,
                tool_count: 1,
                retrieval_count: 0,
                user_override: false,
            },
            RoutingTelemetry {
                task_class: TaskClass::Research,
                context_signature: "research".to_string(),
                selected_policy: OrchestrationPolicy::BestOfN { candidates: 2 },
                selected_model: "strong-vision".to_string(),
                latency_ms: 500,
                outcome: RoutingOutcome::Failed,
                cost_proxy: 180,
                tool_count: 3,
                retrieval_count: 4,
                user_override: false,
            },
        ];

        let report = evaluate_routing_telemetry(&telemetry);

        assert_eq!(report.runs, 2);
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.success_rate, 0.5);
        assert_eq!(report.average_latency_ms, 300);
        assert_eq!(report.average_cost_proxy, 100);
        assert_eq!(report.average_tool_calls, 2.0);
        assert_eq!(report.average_retrievals, 2.0);
        assert_eq!(report.policy_counts.get("single"), Some(&1));
        assert_eq!(report.policy_counts.get("best_of_n"), Some(&1));
    }

    #[test]
    fn quality_rubric_requires_balanced_evidence_and_safety() {
        let strong = QualityRubricScore {
            correctness: 4,
            evidence: 4,
            completion: 4,
            safety: 4,
        };
        let unsupported = QualityRubricScore {
            correctness: 5,
            evidence: 2,
            completion: 5,
            safety: 5,
        };

        assert!(strong.passes());
        assert!(!unsupported.passes());
        assert!(QualityRubricScore {
            correctness: 6,
            evidence: 4,
            completion: 4,
            safety: 4,
        }
        .validate()
        .is_err());
    }
}
