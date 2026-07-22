use agent_core::{Metadata, ModelRole};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod benchmark;
mod evaluation;
mod policy;
mod prompt_evolution;
mod routing;

pub use benchmark::*;
pub use evaluation::*;
pub use policy::*;
pub use prompt_evolution::*;
pub use routing::*;

#[cfg(test)]
use routing::LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE;


pub const MAX_ADAPTIVE_WORKFLOW_STEPS: usize = 5;
pub const MAX_ADAPTIVE_WORKFLOW_AGENTS: usize = 3;
pub const WORKFLOW_IR_SCHEMA: &str = "cindx.workflow.v1";
pub const WORKFLOW_CHECKPOINT_SCHEMA: &str = "cindx.workflow.checkpoint.v1";
pub const CONDUCTOR_MAX_ATTEMPTS: usize = 2;

fn default_prompt_profile() -> String {
    "legacy-baseline-v1".to_string()
}

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
    ReadOnlyExploration,
}

impl WorkflowToolPolicy {
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ReadOnlyEvidence => "read_only_evidence",
            Self::ReadOnlyExploration => "read_only_exploration",
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
    #[serde(default = "default_prompt_profile")]
    pub prompt_profile: String,
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
        Self::from_adaptive_with_profile(
            workflow_id,
            objective,
            effort,
            policy,
            coordinator_model,
            "legacy-baseline-v1",
            workflow,
            budget,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_adaptive_with_profile(
        workflow_id: impl Into<String>,
        objective: impl Into<String>,
        effort: impl Into<String>,
        policy: impl Into<String>,
        coordinator_model: impl Into<String>,
        prompt_profile: impl Into<String>,
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
            prompt_profile: prompt_profile.into(),
            steps: workflow
                .steps
                .iter()
                .map(|step| WorkflowPlanStep {
                    id: step.id.clone(),
                    role: step.role.clone(),
                    model: step.model.clone(),
                    subtask: step.subtask.clone(),
                    access: step.access.clone(),
                    tool_policy: WorkflowToolPolicy::ReadOnlyEvidence,
                })
                .collect(),
            budget,
        }
    }

    pub fn adaptive_workflow(&self) -> AdaptiveWorkflow {
        AdaptiveWorkflow {
            steps: self
                .steps
                .iter()
                .map(|step| AdaptiveWorkflowStep {
                    id: step.id.clone(),
                    role: step.role.clone(),
                    model: step.model.clone(),
                    subtask: step.subtask.clone(),
                    access: step.access.clone(),
                })
                .collect(),
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
            || self.prompt_profile.trim().is_empty()
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
        let selected_models = self
            .steps
            .iter()
            .map(|step| step.model.as_str())
            .collect::<BTreeSet<_>>();
        if selected_models.len() > self.budget.max_models {
            return Err("workflow exceeds its declared model budget".to_string());
        }
        if self.steps.iter().any(|step| {
            step.tool_policy != WorkflowToolPolicy::None
                && self.budget.max_tool_calls_per_step == 0
        }) {
            return Err("workflow enables evidence tools with a zero tool budget".to_string());
        }
        validate_adaptive_workflow(&self.adaptive_workflow(), allowed_models)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("workflow serialization failed: {error}"))
    }

    pub fn from_json(value: &str, allowed_models: &[String]) -> Result<Self, String> {
        let workflow = serde_json::from_str::<Self>(value)
            .map_err(|error| format!("workflow JSON is invalid: {error}"))?;
        workflow.validate(allowed_models)?;
        Ok(workflow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStepStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowStepCheckpoint {
    pub step_id: String,
    pub status: WorkflowStepStatus,
    pub attempts: usize,
    pub model: String,
    pub output: Option<String>,
    #[serde(default)]
    pub evidence_json: String,
    #[serde(default)]
    pub evidence_count: usize,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub credit: Option<f64>,
    pub error: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowExecutionCheckpoint {
    pub schema: String,
    pub resume_key: String,
    pub plan: WorkflowPlanIr,
    #[serde(default)]
    pub prompt_genome_json: String,
    pub steps: BTreeMap<String, WorkflowStepCheckpoint>,
    #[serde(default)]
    pub finalized: bool,
    #[serde(default)]
    pub continuations: usize,
    #[serde(default)]
    pub additional_model_turns_per_step: usize,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl WorkflowExecutionCheckpoint {
    pub fn new(resume_key: impl Into<String>, plan: WorkflowPlanIr, now_ms: u64) -> Self {
        let steps = plan
            .steps
            .iter()
            .map(|step| {
                (
                    step.id.clone(),
                    WorkflowStepCheckpoint {
                        step_id: step.id.clone(),
                        status: WorkflowStepStatus::Pending,
                        attempts: 0,
                        model: step.model.clone(),
                        output: None,
                        evidence_json: String::new(),
                        evidence_count: 0,
                        latency_ms: 0,
                        total_tokens: 0,
                        credit: None,
                        error: None,
                        updated_at_ms: now_ms,
                    },
                )
            })
            .collect();
        Self {
            schema: WORKFLOW_CHECKPOINT_SCHEMA.to_string(),
            resume_key: resume_key.into(),
            plan,
            prompt_genome_json: String::new(),
            steps,
            finalized: false,
            continuations: 0,
            additional_model_turns_per_step: 0,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    pub fn validate(&self, allowed_models: &[String]) -> Result<(), String> {
        if self.schema != WORKFLOW_CHECKPOINT_SCHEMA {
            return Err(format!("unsupported workflow checkpoint schema: {}", self.schema));
        }
        if self.resume_key.trim().is_empty() {
            return Err("workflow checkpoint resume key is empty".to_string());
        }
        self.plan.validate(allowed_models)?;
        let expected = self
            .plan
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<BTreeSet<_>>();
        let actual = self.steps.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if expected != actual {
            return Err("workflow checkpoint steps do not match the plan".to_string());
        }
        for step in &self.plan.steps {
            let checkpoint = self
                .steps
                .get(&step.id)
                .ok_or_else(|| format!("workflow checkpoint is missing step {}", step.id))?;
            if checkpoint.step_id != step.id
                || !allowed_models.iter().any(|model| model == &checkpoint.model)
            {
                return Err(format!("workflow checkpoint step {} changed identity", step.id));
            }
            if checkpoint.status == WorkflowStepStatus::Completed
                && checkpoint.output.as_deref().is_none_or(str::is_empty)
            {
                return Err(format!("completed workflow step {} has no output", step.id));
            }
        }
        Ok(())
    }

    pub fn begin_step(&mut self, step_id: &str, model: &str, now_ms: u64) -> Result<(), String> {
        let attempt_limit = self
            .plan
            .budget
            .max_model_turns_per_step
            .saturating_add(self.additional_model_turns_per_step);
        self.begin_step_with_attempt_limit(step_id, model, attempt_limit, now_ms)
    }

    pub fn begin_step_with_attempt_limit(
        &mut self,
        step_id: &str,
        model: &str,
        attempt_limit: usize,
        now_ms: u64,
    ) -> Result<(), String> {
        let attempt_limit = attempt_limit.max(1);
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        if step.status == WorkflowStepStatus::Completed {
            return Ok(());
        }
        if step.attempts >= attempt_limit {
            return Err(format!(
                "workflow step {step_id} exhausted its {attempt_limit}-turn budget"
            ));
        }
        step.status = WorkflowStepStatus::Running;
        step.attempts = step.attempts.saturating_add(1);
        step.model = model.to_string();
        step.error = None;
        step.updated_at_ms = now_ms;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn complete_step(
        &mut self,
        step_id: &str,
        model: &str,
        output: String,
        evidence_json: String,
        now_ms: u64,
    ) -> Result<(), String> {
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        step.status = WorkflowStepStatus::Completed;
        step.attempts = step.attempts.max(1);
        step.model = model.to_string();
        step.output = Some(output);
        step.evidence_count = serde_json::from_str::<serde_json::Value>(&evidence_json)
            .ok()
            .and_then(|value| value.as_array().map(Vec::len))
            .unwrap_or_default();
        step.evidence_json = evidence_json;
        step.error = None;
        step.updated_at_ms = now_ms;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn record_step_metrics(
        &mut self,
        step_id: &str,
        latency_ms: u64,
        total_tokens: u64,
    ) -> Result<(), String> {
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        step.latency_ms = step.latency_ms.saturating_add(latency_ms);
        step.total_tokens = step.total_tokens.saturating_add(total_tokens);
        Ok(())
    }

    pub fn assign_step_credits(&mut self, final_quality: f64) -> Vec<PromptStepCredit> {
        let quality = final_quality.clamp(0.0, 1.0);
        let roles = self
            .plan
            .steps
            .iter()
            .map(|step| (step.id.clone(), step.role.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut credits = Vec::new();
        for (step_id, step) in &mut self.steps {
            let role = roles
                .get(step_id)
                .cloned()
                .unwrap_or_else(|| "worker".to_string());
            let role_weight = match role.as_str() {
                "synthesizer" => 1.0,
                "verifier" => 0.9,
                "thinker" => 0.8,
                _ => 0.75,
            };
            let evidence_boost = (step.evidence_count as f64 * 0.025).min(0.15);
            let retry_penalty = step.attempts.saturating_sub(1) as f64 * 0.06;
            let succeeded = step.status == WorkflowStepStatus::Completed;
            let credit = if succeeded {
                (quality * role_weight + evidence_boost - retry_penalty).clamp(0.0, 1.0)
            } else {
                0.0
            };
            step.credit = Some(credit);
            credits.push(PromptStepCredit {
                step_id: step_id.clone(),
                role,
                succeeded,
                attempts: step.attempts,
                evidence_count: step.evidence_count,
                latency_ms: step.latency_ms,
                total_tokens: step.total_tokens,
                credit,
            });
        }
        credits.sort_by(|left, right| left.step_id.cmp(&right.step_id));
        credits
    }

    pub fn fail_step(
        &mut self,
        step_id: &str,
        error: impl Into<String>,
        now_ms: u64,
    ) -> Result<(), String> {
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        step.status = WorkflowStepStatus::Failed;
        step.attempts = step.attempts.max(1);
        step.error = Some(error.into());
        step.updated_at_ms = now_ms;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn completed_outputs(&self) -> BTreeMap<String, String> {
        self.steps
            .iter()
            .filter_map(|(id, step)| {
                (step.status == WorkflowStepStatus::Completed)
                    .then(|| step.output.clone().map(|output| (id.clone(), output)))
                    .flatten()
            })
            .collect()
    }

    pub fn runnable_step_indices(&self, layer: &[usize]) -> Result<Vec<usize>, String> {
        let mut runnable = Vec::new();
        for index in layer {
            let plan_step = self
                .plan
                .steps
                .get(*index)
                .ok_or_else(|| format!("workflow layer references unknown step index {index}"))?;
            let checkpoint = self
                .steps
                .get(&plan_step.id)
                .ok_or_else(|| format!("workflow checkpoint is missing step {}", plan_step.id))?;
            if checkpoint.status == WorkflowStepStatus::Completed {
                continue;
            }
            if plan_step.access.iter().any(|dependency| {
                self.steps.get(dependency).is_none_or(|step| {
                    step.status != WorkflowStepStatus::Completed
                })
            }) {
                return Err(format!(
                    "workflow step {} is blocked by an incomplete dependency",
                    plan_step.id
                ));
            }
            runnable.push(*index);
        }
        Ok(runnable)
    }

    pub fn completed_step_count(&self) -> usize {
        self.steps
            .values()
            .filter(|step| step.status == WorkflowStepStatus::Completed)
            .count()
    }

    pub fn is_complete(&self) -> bool {
        self.finalized && self.completed_step_count() == self.plan.steps.len()
    }

    pub fn continue_with_budget(&mut self, additional_turns_per_step: usize, now_ms: u64) {
        self.continuations = self.continuations.saturating_add(1);
        self.additional_model_turns_per_step = self
            .additional_model_turns_per_step
            .saturating_add(additional_turns_per_step.max(1));
        self.updated_at_ms = now_ms;
    }

    pub fn finalize(&mut self, final_output: String, now_ms: u64) -> Result<(), String> {
        let final_step = self
            .plan
            .steps
            .last()
            .ok_or_else(|| "workflow checkpoint has no final step".to_string())?;
        let checkpoint = self
            .steps
            .get_mut(&final_step.id)
            .ok_or_else(|| "workflow checkpoint is missing its final step".to_string())?;
        if checkpoint.status != WorkflowStepStatus::Completed {
            return Err("workflow checkpoint cannot finalize before its final step".to_string());
        }
        checkpoint.output = Some(final_output);
        checkpoint.updated_at_ms = now_ms;
        self.finalized = true;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("workflow checkpoint serialization failed: {error}"))
    }

    pub fn from_json(value: &str, allowed_models: &[String]) -> Result<Self, String> {
        let checkpoint = serde_json::from_str::<Self>(value)
            .map_err(|error| format!("workflow checkpoint JSON is invalid: {error}"))?;
        checkpoint.validate(allowed_models)?;
        Ok(checkpoint)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConductorRoleHints {
    pub planner: String,
    pub executor: String,
    pub reviewer: String,
    pub synthesizer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConductorRequest {
    pub workflow_id: String,
    pub objective: String,
    pub recent_context: String,
    pub effort: String,
    pub policy: String,
    pub conductor_model: String,
    pub worker_models: Vec<String>,
    pub role_hints: ConductorRoleHints,
    pub budget: WorkflowBudget,
    pub prior_hint: Option<String>,
    pub prompt_evolution_enabled: bool,
    pub prompt_genome: ConductorPromptGenome,
}

#[derive(Debug, Clone)]
pub struct ConductorHarness {
    request: ConductorRequest,
}

#[derive(Debug, Deserialize)]
struct ConductorWorkflowPayload {
    steps: Vec<ConductorWorkflowStepPayload>,
}

#[derive(Debug, Deserialize)]
struct ConductorWorkflowStepPayload {
    id: String,
    #[serde(default)]
    role: Option<String>,
    model: String,
    subtask: String,
    #[serde(default, alias = "access_list", alias = "accessList")]
    access: Vec<String>,
}

impl ConductorHarness {
    pub fn new(request: ConductorRequest) -> Self {
        Self { request }
    }

    pub fn request(&self) -> &ConductorRequest {
        &self.request
    }

    pub fn planning_prompt(&self) -> String {
        let request = &self.request;
        let worker_pool = request
            .worker_models
            .iter()
            .map(|model| format!("- {model}"))
            .collect::<Vec<_>>()
            .join("\n");
        let prior_hint = request
            .prior_hint
            .as_deref()
            .unwrap_or("(none - design from the current query)");
        let evolved_directive = if request.prompt_evolution_enabled {
            request.prompt_genome.conductor_directive()
        } else {
            "Prompt evolution is disabled. Use only the baseline harness constraints above."
                .to_string()
        };
        format!(
            concat!(
                "You are the Conductor Agent for a Fugu-style Cindx workflow. Design a query-specific dependency graph instead of answering the user. Return only strict JSON matching this example:\n",
                "{schema_example}\n\n",
                "Harness constraints:\n",
                "- Use between 1 and {max_steps} workflow steps, including the final synthesizer. Choose the smallest useful graph.\n",
                "- Use no more than {max_models} distinct worker models.\n",
                "- role must be exactly thinker, worker, verifier, or synthesizer.\n",
                "- Preserve listed order: access may reference only earlier step ids.\n",
                "- Obey the evolved profile's branch and verification policy; do not add decorative agents.\n",
                "- Keep workers isolated and expose an earlier result only through access.\n",
                "- Give independent root branches non-overlapping subtasks and use distinct models when the pool permits.\n",
                "- A verifier must directly access every independent root branch it audits.\n",
                "- Every branch must reach the final synthesizer; retain dissenting or failed branches.\n",
                "- Use exact model strings from the worker pool. The Conductor model is not implicitly a worker.\n",
                "- Do not include markdown fences, commentary, tool calls, or a user-facing answer.\n\n",
                "Configured worker role hints:\nPlanner: {planner}\nExecutor: {executor}\nReviewer: {reviewer}\nSynthesizer: {synthesizer}\n\n",
                "Allowed worker pool:\n{worker_pool}\n\n",
                "Historical execution prior:\n{prior_hint}\n\n",
                "Evolved orchestration directive:\n{evolved_directive}\n\n",
                "User request:\n{objective}\n\nRecent session memory:\n{recent_context}"
            ),
            schema_example = conductor_schema_example(
                request.budget.max_models,
                request.prompt_genome.max_parallel_branches,
                request.prompt_genome.verification,
                &request.role_hints,
            ),
            max_steps = request.budget.max_steps,
            max_models = request.budget.max_models,
            planner = request.role_hints.planner,
            executor = request.role_hints.executor,
            reviewer = request.role_hints.reviewer,
            synthesizer = request.role_hints.synthesizer,
            worker_pool = worker_pool,
            prior_hint = prior_hint,
            evolved_directive = evolved_directive,
            objective = request.objective,
            recent_context = if request.recent_context.trim().is_empty() {
                "(none)"
            } else {
                &request.recent_context
            },
        )
    }

    pub fn repair_prompt(&self, invalid_response: &str, validation_error: &str) -> String {
        format!(
            "Your previous WorkflowPlan was rejected by the deterministic Cindx Harness. Correct only the workflow structure and return strict JSON with no commentary.\n\nValidation error:\n{}\n\nRejected response:\n{}\n\nOriginal planning request:\n{}",
            validation_error,
            truncate_conductor_text(invalid_response, 6_000),
            self.planning_prompt()
        )
    }

    pub fn parse_plan(&self, response: &str) -> Result<WorkflowPlanIr, String> {
        if self.request.prompt_evolution_enabled {
            self.request.prompt_genome.validate()?;
        }
        let start = response
            .find('{')
            .ok_or_else(|| "conductor did not return a JSON object".to_string())?;
        let end = response
            .rfind('}')
            .filter(|end| *end >= start)
            .ok_or_else(|| "conductor returned incomplete JSON".to_string())?;
        let payload = serde_json::from_str::<ConductorWorkflowPayload>(&response[start..=end])
            .map_err(|error| format!("conductor workflow JSON is invalid: {error}"))?;
        let step_count = payload.steps.len();
        let workflow = AdaptiveWorkflow {
            steps: payload
                .steps
                .into_iter()
                .enumerate()
                .map(|(index, step)| {
                    let access = step
                        .access
                        .into_iter()
                        .map(|dependency| dependency.trim().to_string())
                        .collect::<Vec<_>>();
                    let role = step.role.unwrap_or_else(|| {
                        if index + 1 == step_count {
                            "synthesizer".to_string()
                        } else if access.is_empty() {
                            "thinker".to_string()
                        } else {
                            "worker".to_string()
                        }
                    });
                    AdaptiveWorkflowStep {
                        id: step.id.trim().to_string(),
                        role: role.trim().to_ascii_lowercase(),
                        model: step.model.trim().to_string(),
                        subtask: step.subtask.trim().to_string(),
                        access,
                    }
                })
                .collect(),
        };
        self.validate_shape(&workflow)?;
        let mut budget = self.request.budget.clone();
        if self.request.prompt_evolution_enabled {
            budget.max_model_turns_per_step = budget
                .max_model_turns_per_step
                .min(
                    self.request
                        .prompt_genome
                        .effective_max_model_turns_per_step(),
                )
                .max(1);
            budget.max_tool_calls_per_step = budget.max_tool_calls_per_step.min(
                self.request
                    .prompt_genome
                    .effective_max_tool_calls_per_step(),
            );
        }
        let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
            self.request.workflow_id.clone(),
            self.request.objective.clone(),
            self.request.effort.clone(),
            self.request.policy.clone(),
            self.request.conductor_model.clone(),
            if self.request.prompt_evolution_enabled {
                self.request.prompt_genome.id.clone()
            } else {
                "legacy-baseline-v1".to_string()
            },
            &workflow,
            budget,
        );
        if self.request.prompt_evolution_enabled {
            for step in &mut plan.steps {
                step.tool_policy = self
                    .request
                    .prompt_genome
                    .workflow_tool_policy(&step.role);
            }
        }
        plan.validate(&self.request.worker_models)?;
        Ok(plan)
    }

    fn validate_shape(&self, workflow: &AdaptiveWorkflow) -> Result<(), String> {
        if workflow.steps.len() > self.request.budget.max_steps {
            return Err(format!(
                "conductor workflow exceeds the {}-step budget",
                self.request.budget.max_steps
            ));
        }
        let selected_models = workflow
            .steps
            .iter()
            .map(|step| step.model.as_str())
            .collect::<BTreeSet<_>>();
        if selected_models.len() > self.request.budget.max_models {
            return Err(format!(
                "conductor workflow exceeds the {}-model budget",
                self.request.budget.max_models
            ));
        }
        let independent_branches = workflow
            .steps
            .iter()
            .take(workflow.steps.len().saturating_sub(1))
            .filter(|step| {
                step.access.is_empty() && matches!(step.role.as_str(), "thinker" | "worker")
            })
            .collect::<Vec<_>>();
        let branch_limit = self
            .request
            .prompt_genome
            .max_parallel_branches
            .min(self.request.budget.max_models)
            .max(1);
        let required_branches = if workflow.steps.len() == 1
            && self.request.prompt_genome.graph_depth == PromptGraphDepth::Lean
        {
            0
        } else {
            match self.request.prompt_genome.graph_depth {
                PromptGraphDepth::Lean => 1,
                PromptGraphDepth::Balanced | PromptGraphDepth::Deep => {
                    usize::from(self.request.budget.max_models >= 2) + 1
                }
            }
            .min(branch_limit)
        };
        if independent_branches.len() < required_branches {
            return Err(
                format!(
                    "conductor workflow requires at least {required_branches} independent branch{} for the selected prompt profile",
                    if required_branches == 1 { "" } else { "es" }
                ),
            );
        }
        if independent_branches.len() > branch_limit {
            return Err(format!(
                "conductor workflow exceeds the selected prompt profile's {branch_limit}-branch limit"
            ));
        }
        let distinct_available_models = self
            .request
            .worker_models
            .iter()
            .collect::<BTreeSet<_>>()
            .len();
        let required_branch_models = required_branches.min(distinct_available_models);
        let distinct_branch_models = independent_branches
            .iter()
            .map(|step| step.model.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        if required_branch_models >= 2 && distinct_branch_models < required_branch_models {
            return Err(format!(
                "conductor workflow requires {required_branch_models} distinct models across independent branches"
            ));
        }
        let distinct_branch_subtasks = independent_branches
            .iter()
            .map(|step| {
                step.subtask
                    .split_whitespace()
                    .flat_map(str::chars)
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .collect::<BTreeSet<_>>()
            .len();
        if independent_branches.len() >= 2
            && distinct_branch_subtasks < independent_branches.len()
        {
            return Err("conductor independent branches repeat the same subtask".to_string());
        }
        if self.request.prompt_genome.require_final_synthesis
            && workflow
                .steps
                .last()
                .is_some_and(|step| step.role != "synthesizer")
        {
            return Err("conductor workflow must end with a synthesizer".to_string());
        }
        if self.request.prompt_genome.verification == PromptVerification::Adversarial
            && self.request.budget.max_steps >= 4
        {
            let independent_ids = independent_branches
                .iter()
                .map(|step| step.id.as_str())
                .collect::<BTreeSet<_>>();
            let verifier_covers_branches = workflow.steps.iter().any(|step| {
                step.role == "verifier"
                    && independent_ids
                        .iter()
                        .all(|branch_id| step.access.iter().any(|access| access == branch_id))
            });
            if !verifier_covers_branches {
                return Err(
                    "adversarial prompt profile requires a verifier that directly audits every independent branch"
                        .to_string(),
                );
            }
        }
        if workflow
            .steps
            .last()
            .is_some_and(|step| required_branches >= 2 && step.access.len() < 2)
        {
            return Err("conductor synthesis must access at least two prior branches".to_string());
        }
        Ok(())
    }
}

fn conductor_schema_example(
    max_models: usize,
    max_parallel_branches: usize,
    verification: PromptVerification,
    role_hints: &ConductorRoleHints,
) -> String {
    let branch_executor = if role_hints.executor != role_hints.planner {
        &role_hints.executor
    } else if role_hints.reviewer != role_hints.planner {
        &role_hints.reviewer
    } else {
        &role_hints.executor
    };
    match max_models.min(max_parallel_branches.max(1)) {
        0 | 1 => serde_json::json!({
            "steps": [{
                "id": "synthesize",
                "role": "synthesizer",
                "model": role_hints.synthesizer,
                "subtask": "produce a checkable execution brief",
                "access": [],
            }]
        }),
        2 => serde_json::json!({
            "steps": [
                {
                    "id": "approach_a",
                    "role": "thinker",
                    "model": role_hints.planner,
                    "subtask": "analyze assumptions and the strongest approach",
                    "access": [],
                },
                {
                    "id": "approach_b",
                    "role": "worker",
                    "model": branch_executor,
                    "subtask": "develop a concrete independent implementation path",
                    "access": [],
                },
                {
                    "id": "synthesize",
                    "role": "synthesizer",
                    "model": role_hints.synthesizer,
                    "subtask": "resolve both branches into one execution brief",
                    "access": ["approach_a", "approach_b"],
                },
            ]
        }),
        _ if verification == PromptVerification::Adversarial => serde_json::json!({
            "steps": [
                {
                    "id": "approach_a",
                    "role": "thinker",
                    "model": role_hints.planner,
                    "subtask": "analyze assumptions and the strongest approach",
                    "access": [],
                },
                {
                    "id": "approach_b",
                    "role": "worker",
                    "model": branch_executor,
                    "subtask": "develop a concrete independent implementation path",
                    "access": [],
                },
                {
                    "id": "verify",
                    "role": "verifier",
                    "model": role_hints.reviewer,
                    "subtask": "cross-check both reports and identify unsupported claims",
                    "access": ["approach_a", "approach_b"],
                },
                {
                    "id": "synthesize",
                    "role": "synthesizer",
                    "model": role_hints.synthesizer,
                    "subtask": "resolve disagreements into one evidence-grounded execution brief",
                    "access": ["approach_a", "approach_b", "verify"],
                },
            ]
        }),
        _ => serde_json::json!({
            "steps": [
                {
                    "id": "approach_a",
                    "role": "thinker",
                    "model": role_hints.planner,
                    "subtask": "analyze assumptions and the strongest approach",
                    "access": [],
                },
                {
                    "id": "approach_b",
                    "role": "worker",
                    "model": branch_executor,
                    "subtask": "develop a concrete independent implementation path",
                    "access": [],
                },
                {
                    "id": "synthesize",
                    "role": "synthesizer",
                    "model": role_hints.synthesizer,
                    "subtask": "resolve both branches into one evidence-grounded execution brief",
                    "access": ["approach_a", "approach_b"],
                },
            ]
        }),
    }
    .to_string()
}

fn truncate_conductor_text(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
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

    let allowed_models = allowed_models
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut seen_ids = BTreeSet::new();
    let mut selected_models = BTreeSet::new();
    for step in &workflow.steps {
        if step.id.trim().is_empty() {
            return Err("adaptive workflow step id is empty".to_string());
        }
        if seen_ids.contains(step.id.as_str()) {
            return Err(format!(
                "adaptive workflow step id is duplicated: {}",
                step.id
            ));
        }
        if !allowed_models.contains(step.model.as_str()) {
            return Err(format!(
                "adaptive workflow selected an unknown model: {}",
                step.model
            ));
        }
        selected_models.insert(step.model.as_str());
        if step.subtask.trim().is_empty() {
            return Err(format!(
                "adaptive workflow step {} has an empty subtask",
                step.id
            ));
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
            return Err(format!(
                "adaptive workflow step id is duplicated: {}",
                step.id
            ));
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
    let (role_instruction, output_contract) = match step.role.as_str() {
        "thinker" => (
            "Explore an independent approach, decompose the problem, and expose assumptions without duplicating implementation work.",
            "Hypotheses; Assumptions; Recommended path; Failure modes.",
        ),
        "verifier" => (
            "Audit supplied work against evidence, identify disagreements, and state exact corrections without inventing a new unsupported solution.",
            "Agreements; Disagreements; Evidence verdicts; Required corrections.",
        ),
        "synthesizer" => (
            "Resolve disagreements and produce one checkable execution brief grounded in the supplied work.",
            "Decision; Integrated execution brief; Evidence basis; Unresolved risks.",
        ),
        _ => (
            "Produce concrete work for the assigned subtask and report evidence and uncertainty rather than repeating the planning branch.",
            "Work product; Evidence used or needed; Risks; Handoff.",
        ),
    };
    let mut prompt = format!(
        "You are isolated {} {} in a Cindx adaptive multi-model workflow. {} Complete only the assigned subtask. Do not assume you can see other agents unless their output is explicitly included below. Use exposed read-only evidence tools when the subtask depends on workspace facts. Return concrete findings for a later agent, not a user-facing answer. Do not merely restate authorized outputs; transform, test, or reconcile them for your role.\n\nOutput contract:\n{}\n\nUser request:\n{}\n\nAssigned subtask:\n{}\n\nShared memory from earlier user turns:\n{}",
        step.role,
        step.id,
        role_instruction,
        output_contract,
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
                vec![
                    "approach_a".to_string(),
                    "approach_b".to_string(),
                    "verify".to_string(),
                ]
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

    fn conductor_request() -> ConductorRequest {
        ConductorRequest {
            workflow_id: "workflow-conductor".to_string(),
            objective: "Compare implementation strategies".to_string(),
            recent_context: "The workspace uses Rust.".to_string(),
            effort: "pro".to_string(),
            policy: "best_of_n".to_string(),
            conductor_model: "conductor-only".to_string(),
            worker_models: vec!["planner".to_string(), "reviewer".to_string()],
            role_hints: ConductorRoleHints {
                planner: "planner".to_string(),
                executor: "planner".to_string(),
                reviewer: "reviewer".to_string(),
                synthesizer: "planner".to_string(),
            },
            budget: WorkflowBudget {
                max_steps: 3,
                max_models: 2,
                max_model_turns_per_step: 5,
                max_tool_calls_per_step: 6,
                max_output_tokens_per_step: 4_096,
            },
            prior_hint: Some("Prefer two independent branches.".to_string()),
            prompt_evolution_enabled: true,
            prompt_genome: ConductorPromptGenome::seed_for_effort("pro"),
        }
    }

    #[test]
    fn workflow_ir_round_trips_and_enforces_declared_budgets() {
        let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
        let plan = workflow_plan("workflow-1", false);
        plan.validate(&allowed_models)
            .expect("workflow should be valid");

        let json = plan.to_json().expect("workflow should serialize");
        assert_eq!(
            WorkflowPlanIr::from_json(&json, &allowed_models).unwrap(),
            plan
        );

        let mut invalid = plan;
        invalid.budget.max_steps = 2;
        assert_eq!(
            invalid.validate(&allowed_models),
            Err("workflow exceeds its declared step budget".to_string())
        );
    }

    #[test]
    fn workflow_checkpoint_resumes_only_incomplete_dependency_ready_steps() {
        let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
        let plan = workflow_plan("workflow-resume", false);
        let layers = adaptive_workflow_layers(&plan.adaptive_workflow()).unwrap();
        let mut checkpoint =
            WorkflowExecutionCheckpoint::new("resume-key", plan.clone(), 1_000);

        assert_eq!(checkpoint.runnable_step_indices(&layers[0]).unwrap(), vec![0, 1]);
        checkpoint.begin_step("approach_a", "planner", 1_010).unwrap();
        checkpoint
            .complete_step(
                "approach_a",
                "planner",
                "primary".to_string(),
                "[]".to_string(),
                1_020,
            )
            .unwrap();
        assert_eq!(checkpoint.runnable_step_indices(&layers[0]).unwrap(), vec![1]);
        assert!(checkpoint.runnable_step_indices(&layers[1]).is_err());

        let json = checkpoint.to_json().unwrap();
        let mut restored =
            WorkflowExecutionCheckpoint::from_json(&json, &allowed_models).unwrap();
        restored.begin_step("approach_b", "reviewer", 1_030).unwrap();
        restored
            .complete_step(
                "approach_b",
                "reviewer",
                "alternative".to_string(),
                "[]".to_string(),
                1_040,
            )
            .unwrap();
        assert_eq!(restored.runnable_step_indices(&layers[1]).unwrap(), vec![2]);
        restored.begin_step("synthesize", "planner", 1_050).unwrap();
        restored.fail_step("synthesize", "transient", 1_060).unwrap();
        assert_eq!(restored.runnable_step_indices(&layers[1]).unwrap(), vec![2]);
        restored
            .complete_step(
                "synthesize",
                "reviewer",
                "final".to_string(),
                "[]".to_string(),
                1_070,
            )
            .unwrap();
        restored.record_step_metrics("synthesize", 420, 900).unwrap();
        let credits = restored.assign_step_credits(0.9);
        assert_eq!(credits.len(), 3);
        assert!(credits
            .iter()
            .find(|step| step.step_id == "synthesize")
            .is_some_and(|step| step.credit > 0.7 && step.total_tokens == 900));
        assert!(!restored.is_complete());
        restored.finalize("quality-gated final".to_string(), 1_080).unwrap();
        assert!(restored.is_complete());
        assert_eq!(restored.completed_outputs().get("approach_a").map(String::as_str), Some("primary"));
        assert_eq!(
            restored.completed_outputs().get("synthesize").map(String::as_str),
            Some("quality-gated final")
        );
    }

    #[test]
    fn workflow_checkpoint_requires_an_explicit_budget_continuation() {
        let mut plan = workflow_plan("workflow-budget", false);
        plan.budget.max_model_turns_per_step = 1;
        let mut checkpoint = WorkflowExecutionCheckpoint::new("resume-budget", plan, 2_000);

        checkpoint.begin_step("approach_a", "planner", 2_010).unwrap();
        checkpoint.fail_step("approach_a", "timeout", 2_020).unwrap();
        assert!(checkpoint
            .begin_step("approach_a", "planner", 2_030)
            .unwrap_err()
            .contains("exhausted"));
        checkpoint.continue_with_budget(1, 2_040);
        checkpoint.begin_step("approach_a", "planner", 2_050).unwrap();
        assert_eq!(checkpoint.continuations, 1);
        assert_eq!(checkpoint.steps["approach_a"].attempts, 2);
    }

    #[test]
    fn workflow_checkpoint_accepts_an_explicit_attempt_budget() {
        let mut plan = workflow_plan("workflow-attempt-budget", false);
        plan.budget.max_model_turns_per_step = 1;
        let mut checkpoint = WorkflowExecutionCheckpoint::new("attempt-budget", plan, 3_000);

        checkpoint
            .begin_step_with_attempt_limit("approach_a", "planner", 2, 3_010)
            .unwrap();
        checkpoint.fail_step("approach_a", "retry", 3_020).unwrap();
        checkpoint
            .begin_step_with_attempt_limit("approach_a", "planner", 2, 3_030)
            .unwrap();
        assert!(checkpoint
            .begin_step_with_attempt_limit("approach_a", "planner", 2, 3_040)
            .unwrap_err()
            .contains("exhausted"));
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
                plan: fast_plan.clone(),
                succeeded: true,
                quality_score: Some(0.88),
                latency_ms: 5_000,
                total_tokens: 5_000,
                tool_calls: 2,
                fallback_used: false,
            },
            WorkflowExecutionTelemetry {
                task_class: TaskClass::Research,
                plan: fast_plan.clone(),
                succeeded: true,
                quality_score: Some(0.90),
                latency_ms: 4_500,
                total_tokens: 4_500,
                tool_calls: 2,
                fallback_used: false,
            },
            WorkflowExecutionTelemetry {
                task_class: TaskClass::Research,
                plan: fast_plan,
                succeeded: true,
                quality_score: Some(0.91),
                latency_ms: 4_250,
                total_tokens: 4_250,
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
        assert_eq!(prior.examples, 4);
        assert_eq!(prior.success_rate, 1.0);
        assert!(prior.success_confidence >= LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE);
        assert!(prior.prompt_hint().contains("Treat this only as a prior"));
    }

    #[test]
    fn search_teacher_withholds_under_evidenced_topology() {
        let allowed_models = vec!["planner".to_string(), "reviewer".to_string()];
        let plan = workflow_plan("under-evidenced", false);
        let telemetry = (0..3)
            .map(|_| WorkflowExecutionTelemetry {
                task_class: TaskClass::Research,
                plan: plan.clone(),
                succeeded: true,
                quality_score: Some(0.95),
                latency_ms: 4_000,
                total_tokens: 4_000,
                tool_calls: 2,
                fallback_used: false,
            })
            .collect::<Vec<_>>();

        let teacher = WorkflowSearchTeacher::train(&telemetry);
        assert!(teacher
            .best_prior(&TaskClass::Research, "pro", &allowed_models, 2)
            .is_none());
    }

    #[test]
    fn conductor_harness_builds_context_and_parses_a_valid_plan() {
        let harness = ConductorHarness::new(conductor_request());
        let prompt = harness.planning_prompt();
        assert!(prompt.contains("Allowed worker pool:\n- planner\n- reviewer"));
        assert!(prompt.contains("Prefer two independent branches"));
        assert!(!prompt.contains("conductor-only"));
        assert!(prompt.contains(r#""model":"planner""#));
        assert!(prompt.contains(r#""model":"reviewer""#));
        assert!(prompt.contains(r#""role":"thinker""#));
        assert!(prompt.contains(r#""role":"worker""#));

        let plan = harness.parse_plan(
            r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"alternative","access":[]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b"]}]}"#,
        ).expect("plan should parse");
        assert_eq!(plan.coordinator_model, "conductor-only");
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.schema, WORKFLOW_IR_SCHEMA);
    }

    #[test]
    fn conductor_harness_produces_a_bounded_repair_request() {
        let harness = ConductorHarness::new(conductor_request());
        let invalid = r#"{"steps":[{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":[]}]}"#;
        let error = harness
            .parse_plan(invalid)
            .expect_err("one branch should fail");
        let repair = harness.repair_prompt(invalid, &error);

        assert!(error.contains("at least 2 independent branches"));
        assert!(repair.contains("deterministic Cindx Harness"));
        assert!(repair.contains(&error));
    }

    #[test]
    fn conductor_harness_requires_diverse_root_branches_and_complete_review() {
        let mut request = conductor_request();
        request.budget.max_steps = 5;
        let harness = ConductorHarness::new(request);
        let same_model = harness
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary analysis","access":[]},{"id":"b","role":"worker","model":"planner","subtask":"independent implementation","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit both","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect_err("distinct models should cover independent branches");
        assert!(same_model.contains("distinct models"), "{same_model}");

        let duplicate_subtask = harness
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"Inspect the design","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":" inspect   THE design ","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit both","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect_err("duplicate branch assignments should be rejected");
        assert!(duplicate_subtask.contains("repeat the same subtask"));

        let incomplete_review = harness
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary analysis","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"independent implementation","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"audit one branch","access":["a"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect_err("adversarial review should cover every root branch");
        assert!(incomplete_review.contains("directly audits every independent branch"));
    }

    #[test]
    fn conductor_harness_enforces_the_selected_prompt_genome() {
        let mut lean_request = conductor_request();
        lean_request.prompt_genome = ConductorPromptGenome::seed_for_effort("fast");
        let lean = ConductorHarness::new(lean_request);
        let lean_plan = lean.parse_plan(
            r#"{"steps":[{"id":"final","role":"synthesizer","model":"planner","subtask":"direct answer","access":[]}]}"#,
        )
        .expect("lean profile should allow one direct branch");
        assert_eq!(lean_plan.budget.max_model_turns_per_step, 1);
        assert_eq!(lean_plan.budget.max_tool_calls_per_step, 0);
        assert_eq!(lean_plan.steps[0].tool_policy, WorkflowToolPolicy::None);

        let mut adversarial_request = conductor_request();
        adversarial_request.budget.max_steps = 5;
        let adversarial = ConductorHarness::new(adversarial_request);
        let error = adversarial
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"alternative","access":[]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b"]}]}"#,
            )
            .expect_err("adversarial profile should require a verifier");
        assert!(error.contains("requires a verifier"));

        let pro_plan = adversarial
            .parse_plan(
                r#"{"steps":[{"id":"a","role":"thinker","model":"planner","subtask":"primary","access":[]},{"id":"b","role":"worker","model":"reviewer","subtask":"alternative","access":[]},{"id":"verify","role":"verifier","model":"reviewer","subtask":"challenge","access":["a","b"]},{"id":"final","role":"synthesizer","model":"planner","subtask":"merge","access":["a","b","verify"]}]}"#,
            )
            .expect("pro profile should accept a verified graph");
        assert_eq!(pro_plan.budget.max_model_turns_per_step, 3);
        assert_eq!(pro_plan.budget.max_tool_calls_per_step, 6);
        assert_eq!(
            pro_plan.steps[0].tool_policy,
            WorkflowToolPolicy::ReadOnlyExploration
        );
        assert_eq!(
            pro_plan.steps.last().unwrap().tool_policy,
            WorkflowToolPolicy::None
        );
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
        assert_eq!(
            parse_policy("auto_router"),
            Some(OrchestrationPolicy::AutoRouter)
        );
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
        assert!(prompt.contains("Integrated execution brief"));
        assert!(prompt.contains("Do not merely restate authorized outputs"));

        let thinker = adaptive_worker_prompt(&workflow, 0, "Investigate", "", &BTreeMap::new())
            .expect("thinker prompt should build");
        let worker = adaptive_worker_prompt(&workflow, 1, "Investigate", "", &BTreeMap::new())
            .expect("worker prompt should build");
        assert!(thinker.contains("Hypotheses; Assumptions"));
        assert!(worker.contains("Work product; Evidence used or needed"));
        assert_ne!(thinker, worker);
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
        let error = validate_adaptive_workflow(&unknown_model_workflow, &["fast-mini".to_string()])
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
        let context =
            RoutingContext::from_prompt("Search the docs with RAG and cite sources", candidates());
        let router = RuleBasedRouter;
        let decision = router.route(&context);

        assert_eq!(decision.policy, OrchestrationPolicy::PlanExecuteReview);
        assert_eq!(decision.retrieval_mode, "four_way_parallel");
        assert!(decision.explanation.contains("class=retrieval"));
    }

    #[test]
    fn chinese_collaboration_request_routes_to_real_ensemble() {
        let context =
            RoutingContext::from_prompt("分析多个模型协同，并对标 Sakana Fugu Ultra", candidates());
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
        let context =
            RoutingContext::from_prompt("继续完善 Sakana Fugu Ultra 的复现", candidates());
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
        assert_eq!(
            decision.policy,
            OrchestrationPolicy::BestOfN { candidates: 3 }
        );
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
        assert_eq!(
            decision.policy,
            OrchestrationPolicy::BestOfN { candidates: 2 }
        );
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
        let mut context =
            RoutingContext::from_prompt("Research three implementation options", candidates());
        context.user_policy_override = Some(OrchestrationPolicy::Single);
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(decision.policy, OrchestrationPolicy::Single);
        assert!(decision.explanation.contains("user override"));
    }

    #[test]
    fn learned_router_uses_successful_trace_table() {
        let context =
            RoutingContext::from_prompt("Research and compare local agent routers", candidates());
        let context_signature = context.learning_signature();
        let mut telemetry = (0..4)
            .map(|_| RoutingTelemetry {
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
            })
            .collect::<Vec<_>>();
        telemetry.push(RoutingTelemetry {
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
        });
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
        let prompt =
            "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
        let context = RoutingContext::from_prompt(&prompt, candidates());
        assert_eq!(
            RuleBasedRouter.route(&context).policy,
            OrchestrationPolicy::PlanExecuteReview
        );
        let telemetry = (0..4)
            .map(|_| RoutingTelemetry {
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
            })
            .collect::<Vec<_>>();

        let decision = LearnedModelRouter::train(&telemetry).route(&context);
        assert_eq!(decision.policy, OrchestrationPolicy::Single);
        assert_eq!(decision.model, "fast-mini");
        assert!(decision.explanation.contains("learned_policy=single"));
    }

    #[test]
    fn learned_router_does_not_overfit_three_successful_traces() {
        let prompt =
            "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
        let context = RoutingContext::from_prompt(&prompt, candidates());
        let telemetry = (0..3)
            .map(|_| RoutingTelemetry {
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
            })
            .collect::<Vec<_>>();

        let router = LearnedModelRouter::train(&telemetry);
        let route = router
            .learned_route_for_context(&context)
            .expect("route evidence should remain observable");
        assert!(!route.evidence_ready());
        assert_eq!(router.route(&context), RuleBasedRouter.route(&context));
    }

    #[test]
    fn learned_router_does_not_transfer_coarse_task_class_evidence() {
        let prompt =
            "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
        let context = RoutingContext::from_prompt(&prompt, candidates());
        let telemetry = (0..4)
            .map(|_| RoutingTelemetry {
                task_class: TaskClass::General,
                context_signature: TaskClass::General.label().to_string(),
                selected_policy: OrchestrationPolicy::Single,
                selected_model: "fast-mini".to_string(),
                latency_ms: 250,
                outcome: RoutingOutcome::Succeeded,
                cost_proxy: 80,
                tool_count: 0,
                retrieval_count: 0,
                user_override: false,
            })
            .collect::<Vec<_>>();

        let router = LearnedModelRouter::train(&telemetry);
        assert!(router.learned_route_for_context(&context).is_none());
        assert_eq!(router.route(&context), RuleBasedRouter.route(&context));
    }

    #[test]
    fn learned_router_prefers_reliable_route_over_more_raw_successes() {
        let prompt =
            "Discuss this topic clearly and summarize the important distinctions. ".repeat(10);
        let context = RoutingContext::from_prompt(&prompt, candidates());
        let signature = context.learning_signature();
        let mut telemetry = (0..4)
            .map(|_| RoutingTelemetry {
                task_class: TaskClass::General,
                context_signature: signature.clone(),
                selected_policy: OrchestrationPolicy::Single,
                selected_model: "fast-mini".to_string(),
                latency_ms: 250,
                outcome: RoutingOutcome::Succeeded,
                cost_proxy: 80,
                tool_count: 0,
                retrieval_count: 0,
                user_override: false,
            })
            .collect::<Vec<_>>();
        telemetry.extend((0..10).map(|index| RoutingTelemetry {
            task_class: TaskClass::General,
            context_signature: signature.clone(),
            selected_policy: OrchestrationPolicy::PlanExecuteReview,
            selected_model: "strong-vision".to_string(),
            latency_ms: 900,
            outcome: if index < 5 {
                RoutingOutcome::Succeeded
            } else {
                RoutingOutcome::Failed
            },
            cost_proxy: 120,
            tool_count: 0,
            retrieval_count: 0,
            user_override: false,
        }));

        let decision = LearnedModelRouter::train(&telemetry).route(&context);
        assert_eq!(decision.policy, OrchestrationPolicy::Single);
        assert_eq!(decision.model, "fast-mini");
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
        assert_ne!(
            decision.policy,
            OrchestrationPolicy::BestOfN { candidates: 2 }
        );
        assert_ne!(
            decision.policy,
            OrchestrationPolicy::BestOfN { candidates: 3 }
        );
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
    fn english_workspace_actions_are_classified_as_coding() {
        for prompt in [
            "Implement a parser in this project and run tests.",
            "Edit these files to add pagination and check the result.",
            "Implement this function in the existing codebase.",
        ] {
            let context = RoutingContext::from_prompt(prompt, candidates());
            assert_eq!(context.task_class, TaskClass::Coding, "{prompt}");
            assert!(context.needs_tools, "{prompt}");
            assert!(context.needs_retrieval, "{prompt}");
        }
    }

    #[test]
    fn chinese_multi_phase_root_cause_work_routes_to_two_experts() {
        let context =
            RoutingContext::from_prompt("排查这个项目的根因，修改文件并运行测试。", candidates());
        let decision = RuleBasedRouter.route(&context);

        assert_eq!(context.task_class, TaskClass::Coding);
        assert_eq!(
            decision.policy,
            OrchestrationPolicy::BestOfN { candidates: 2 }
        );
        assert_eq!(decision.retrieval_mode, "four_way_parallel");
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
