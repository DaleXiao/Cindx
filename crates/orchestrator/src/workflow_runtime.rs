use super::*;

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

    pub fn effective_model_turn_budget(&self, declared_turns: usize) -> usize {
        let minimum = match self {
            Self::None => 1,
            Self::ReadOnlyEvidence => 2,
            Self::ReadOnlyExploration => 3,
        };
        declared_turns.max(minimum)
    }

    pub fn effective_tool_call_budget(&self, declared_calls: usize) -> usize {
        match self {
            Self::None => 0,
            Self::ReadOnlyEvidence => declared_calls.max(4),
            Self::ReadOnlyExploration => declared_calls.max(6),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOutputKind {
    #[default]
    Analysis,
    Evidence,
    Verification,
    Synthesis,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCompletionCriteria {
    #[serde(default = "default_true")]
    pub require_non_empty_output: bool,
    #[serde(default = "default_true")]
    pub require_resolved_inputs: bool,
    #[serde(default)]
    pub minimum_evidence_items: usize,
}

impl Default for WorkflowCompletionCriteria {
    fn default() -> Self {
        Self {
            require_non_empty_output: true,
            require_resolved_inputs: true,
            minimum_evidence_items: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowStepContract {
    #[serde(default)]
    pub input_steps: Vec<String>,
    #[serde(default)]
    pub output_kind: WorkflowOutputKind,
    #[serde(default)]
    pub completion: WorkflowCompletionCriteria,
}

impl WorkflowStepContract {
    pub(crate) fn inferred(
        role: &str,
        access: &[String],
        tool_policy: &WorkflowToolPolicy,
    ) -> Self {
        let output_kind = match role {
            "verifier" => WorkflowOutputKind::Verification,
            "synthesizer" => WorkflowOutputKind::Synthesis,
            "worker" if *tool_policy != WorkflowToolPolicy::None => WorkflowOutputKind::Evidence,
            _ => WorkflowOutputKind::Analysis,
        };
        Self {
            input_steps: access.to_vec(),
            output_kind,
            completion: WorkflowCompletionCriteria::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPlanStep {
    pub id: String,
    pub role: String,
    pub model: String,
    pub subtask: String,
    pub access: Vec<String>,
    pub tool_policy: WorkflowToolPolicy,
    #[serde(default)]
    pub contract: WorkflowStepContract,
}

impl WorkflowPlanStep {
    pub fn independent_contribution_key(&self) -> String {
        self.subtask
            .split_whitespace()
            .flat_map(str::chars)
            .flat_map(char::to_lowercase)
            .collect()
    }
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
        let mut steps = workflow
            .steps
            .iter()
            .map(|step| WorkflowPlanStep {
                id: step.id.clone(),
                role: step.role.clone(),
                model: step.model.clone(),
                subtask: step.subtask.clone(),
                access: step.access.clone(),
                tool_policy: WorkflowToolPolicy::ReadOnlyEvidence,
                contract: WorkflowStepContract::inferred(
                    &step.role,
                    &step.access,
                    &WorkflowToolPolicy::ReadOnlyEvidence,
                ),
            })
            .collect::<Vec<_>>();
        if !steps.iter().any(|step| {
            matches!(
                step.contract.output_kind,
                WorkflowOutputKind::Synthesis | WorkflowOutputKind::Verification
            )
        }) {
            if let Some(delivery) = steps.last_mut() {
                delivery.contract.output_kind = WorkflowOutputKind::Synthesis;
            }
        }
        Self {
            schema: WORKFLOW_IR_SCHEMA.to_string(),
            workflow_id: workflow_id.into(),
            objective: objective.into(),
            effort: effort.into(),
            policy: policy.into(),
            coordinator_model: coordinator_model.into(),
            prompt_profile: prompt_profile.into(),
            steps,
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
            step.tool_policy != WorkflowToolPolicy::None && self.budget.max_tool_calls_per_step == 0
        }) {
            return Err("workflow enables evidence tools with a zero tool budget".to_string());
        }
        for step in &self.steps {
            if step.contract.input_steps != step.access {
                return Err(format!(
                    "workflow step {} contract inputs do not match its dependencies",
                    step.id
                ));
            }
        }
        if self
            .steps
            .last()
            .is_none_or(|step| step.contract.output_kind != WorkflowOutputKind::Synthesis)
        {
            return Err("workflow must end with a synthesis output contract".to_string());
        }
        validate_adaptive_workflow(&self.adaptive_workflow(), allowed_models)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("workflow serialization failed: {error}"))
    }

    pub fn from_json(value: &str, allowed_models: &[String]) -> Result<Self, String> {
        let mut workflow = serde_json::from_str::<Self>(value)
            .map_err(|error| format!("workflow JSON is invalid: {error}"))?;
        workflow.reconcile_step_contracts();
        workflow.validate(allowed_models)?;
        Ok(workflow)
    }

    fn reconcile_step_contracts(&mut self) {
        for step in &mut self.steps {
            if step.contract == WorkflowStepContract::default() {
                step.contract =
                    WorkflowStepContract::inferred(&step.role, &step.access, &step.tool_policy);
            } else if step.contract.input_steps.is_empty() && !step.access.is_empty() {
                step.contract.input_steps = step.access.clone();
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStepStatus {
    Pending,
    Running,
    Completed,
    Degraded,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowVerificationState {
    #[default]
    NotRequired,
    Passed,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowStepSemanticState {
    #[serde(default)]
    pub input_digests: BTreeMap<String, String>,
    #[serde(default)]
    pub output_digest: String,
    #[serde(default)]
    pub output_kind: WorkflowOutputKind,
    #[serde(default)]
    pub evidence_count: usize,
    #[serde(default)]
    pub verification: WorkflowVerificationState,
    #[serde(default)]
    pub completion_satisfied: bool,
    #[serde(default)]
    pub completed_at_ms: Option<u64>,
}

impl WorkflowStepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Degraded => "degraded",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    fn dependency_resolved(&self) -> bool {
        matches!(self, Self::Completed | Self::Degraded)
    }
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
    #[serde(default)]
    pub semantic: WorkflowStepSemanticState,
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
    #[serde(default)]
    pub anytime_controller_json: String,
    #[serde(default)]
    pub anytime_outputs: BTreeMap<String, String>,
    pub steps: BTreeMap<String, WorkflowStepCheckpoint>,
    #[serde(default)]
    pub finalized: bool,
    #[serde(default)]
    pub continuations: usize,
    #[serde(default)]
    pub additional_model_turns_per_step: usize,
    #[serde(default)]
    pub plan_revisions: Vec<WorkflowPlanRevisionRecord>,
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
                        semantic: WorkflowStepSemanticState {
                            output_kind: step.contract.output_kind.clone(),
                            ..WorkflowStepSemanticState::default()
                        },
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
            anytime_controller_json: String::new(),
            anytime_outputs: BTreeMap::new(),
            steps,
            finalized: false,
            continuations: 0,
            additional_model_turns_per_step: 0,
            plan_revisions: Vec::new(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    pub fn validate(&self, allowed_models: &[String]) -> Result<(), String> {
        if self.schema != WORKFLOW_CHECKPOINT_SCHEMA {
            return Err(format!(
                "unsupported workflow checkpoint schema: {}",
                self.schema
            ));
        }
        if self.resume_key.trim().is_empty() {
            return Err("workflow checkpoint resume key is empty".to_string());
        }
        self.plan.validate(allowed_models)?;
        if self.plan_revisions.len() > MAX_WORKFLOW_PLAN_REVISIONS {
            return Err("workflow checkpoint exceeds its plan revision budget".to_string());
        }
        let expected = self
            .plan
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<BTreeSet<_>>();
        let actual = self
            .steps
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if expected != actual {
            return Err("workflow checkpoint steps do not match the plan".to_string());
        }
        for step in &self.plan.steps {
            let checkpoint = self
                .steps
                .get(&step.id)
                .ok_or_else(|| format!("workflow checkpoint is missing step {}", step.id))?;
            if checkpoint.step_id != step.id
                || !allowed_models
                    .iter()
                    .any(|model| model == &checkpoint.model)
            {
                return Err(format!(
                    "workflow checkpoint step {} changed identity",
                    step.id
                ));
            }
            if checkpoint.status.dependency_resolved()
                && checkpoint.output.as_deref().is_none_or(str::is_empty)
            {
                return Err(format!("resolved workflow step {} has no output", step.id));
            }
            if checkpoint.status == WorkflowStepStatus::Completed
                && !checkpoint.semantic.output_digest.is_empty()
                && !checkpoint.semantic.completion_satisfied
            {
                return Err(format!(
                    "completed workflow step {} does not satisfy its semantic contract",
                    step.id
                ));
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
        if step.status.dependency_resolved() {
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
        let plan_step = self
            .plan
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .ok_or_else(|| format!("workflow plan is missing step: {step_id}"))?;
        let output = output.trim().to_string();
        if plan_step.contract.completion.require_non_empty_output && output.is_empty() {
            return Err(format!(
                "workflow step {step_id} cannot complete with an empty output"
            ));
        }
        let input_digests = if plan_step.contract.completion.require_resolved_inputs {
            let mut digests = BTreeMap::new();
            for dependency in &plan_step.contract.input_steps {
                let dependency_step = self.steps.get(dependency).ok_or_else(|| {
                    format!("workflow step {step_id} references missing input {dependency}")
                })?;
                if !dependency_step.status.dependency_resolved() {
                    return Err(format!(
                        "workflow step {step_id} cannot complete before input {dependency}"
                    ));
                }
                let digest = if dependency_step.semantic.output_digest.is_empty() {
                    dependency_step
                        .output
                        .as_deref()
                        .map(workflow_output_digest)
                        .unwrap_or_default()
                } else {
                    dependency_step.semantic.output_digest.clone()
                };
                digests.insert(dependency.clone(), digest);
            }
            digests
        } else {
            BTreeMap::new()
        };
        let evidence_count = serde_json::from_str::<serde_json::Value>(&evidence_json)
            .ok()
            .and_then(|value| value.as_array().map(Vec::len))
            .unwrap_or_default();
        if evidence_count < plan_step.contract.completion.minimum_evidence_items {
            return Err(format!(
                "workflow step {step_id} produced {evidence_count} evidence items but requires {}",
                plan_step.contract.completion.minimum_evidence_items
            ));
        }
        let semantic = WorkflowStepSemanticState {
            input_digests,
            output_digest: workflow_output_digest(&output),
            output_kind: plan_step.contract.output_kind.clone(),
            evidence_count,
            verification: if plan_step.contract.output_kind == WorkflowOutputKind::Verification {
                WorkflowVerificationState::Passed
            } else {
                WorkflowVerificationState::NotRequired
            },
            completion_satisfied: true,
            completed_at_ms: Some(now_ms),
        };
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        step.status = WorkflowStepStatus::Completed;
        step.attempts = step.attempts.max(1);
        step.model = model.to_string();
        step.output = Some(output);
        step.evidence_count = evidence_count;
        step.evidence_json = evidence_json;
        step.semantic = semantic;
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

    pub fn cancel_step(
        &mut self,
        step_id: &str,
        reason: impl Into<String>,
        now_ms: u64,
    ) -> Result<(), String> {
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        step.status = WorkflowStepStatus::Cancelled;
        step.attempts = step.attempts.max(1);
        step.error = Some(reason.into());
        step.updated_at_ms = now_ms;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn degrade_step(
        &mut self,
        step_id: &str,
        output: String,
        error: impl Into<String>,
        now_ms: u64,
    ) -> Result<(), String> {
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        step.status = WorkflowStepStatus::Degraded;
        step.attempts = step.attempts.max(1);
        let output = output.trim().to_string();
        step.output = Some(output.clone());
        step.semantic.output_digest = workflow_output_digest(&output);
        step.semantic.output_kind = self
            .plan
            .steps
            .iter()
            .find(|plan_step| plan_step.id == step_id)
            .map(|plan_step| plan_step.contract.output_kind.clone())
            .unwrap_or_default();
        step.semantic.verification = WorkflowVerificationState::Degraded;
        step.semantic.completion_satisfied = false;
        step.semantic.completed_at_ms = Some(now_ms);
        step.error = Some(error.into());
        step.updated_at_ms = now_ms;
        self.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn completed_outputs(&self) -> BTreeMap<String, String> {
        self.steps
            .iter()
            .filter_map(|(id, step)| {
                step.status
                    .dependency_resolved()
                    .then(|| step.output.clone().map(|output| (id.clone(), output)))
                    .flatten()
            })
            .collect()
    }

    pub fn runnable_step_indices(&self, layer: &[usize]) -> Result<Vec<usize>, String> {
        let mut runnable = Vec::new();
        for index in layer {
            let plan_step =
                self.plan.steps.get(*index).ok_or_else(|| {
                    format!("workflow layer references unknown step index {index}")
                })?;
            let checkpoint = self
                .steps
                .get(&plan_step.id)
                .ok_or_else(|| format!("workflow checkpoint is missing step {}", plan_step.id))?;
            if checkpoint.status.dependency_resolved() {
                continue;
            }
            if plan_step.access.iter().any(|dependency| {
                self.steps
                    .get(dependency)
                    .is_none_or(|step| !step.status.dependency_resolved())
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

    pub fn resolved_step_count(&self) -> usize {
        self.steps
            .values()
            .filter(|step| step.status.dependency_resolved())
            .count()
    }

    pub fn is_complete(&self) -> bool {
        self.finalized && self.resolved_step_count() == self.plan.steps.len()
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
        let mut checkpoint = serde_json::from_str::<Self>(value)
            .map_err(|error| format!("workflow checkpoint JSON is invalid: {error}"))?;
        checkpoint.plan.reconcile_step_contracts();
        checkpoint.reconcile_semantic_state()?;
        checkpoint.validate(allowed_models)?;
        Ok(checkpoint)
    }

    fn reconcile_semantic_state(&mut self) -> Result<(), String> {
        for plan_step in &self.plan.steps {
            let recovered_input_digests = plan_step
                .contract
                .input_steps
                .iter()
                .filter_map(|dependency| {
                    self.steps.get(dependency).and_then(|input| {
                        input
                            .output
                            .as_deref()
                            .map(|output| (dependency.clone(), workflow_output_digest(output)))
                    })
                })
                .collect::<BTreeMap<_, _>>();
            let step = self
                .steps
                .get_mut(&plan_step.id)
                .ok_or_else(|| format!("workflow checkpoint is missing step {}", plan_step.id))?;
            step.semantic.output_kind = plan_step.contract.output_kind.clone();
            if step.status.dependency_resolved() {
                if step.semantic.output_digest.is_empty() {
                    step.semantic.output_digest = step
                        .output
                        .as_deref()
                        .map(workflow_output_digest)
                        .unwrap_or_default();
                }
                if step.semantic.input_digests.is_empty() {
                    step.semantic.input_digests = recovered_input_digests;
                }
                step.semantic.evidence_count = step.evidence_count;
                step.semantic.completion_satisfied = step.status == WorkflowStepStatus::Completed;
                step.semantic
                    .completed_at_ms
                    .get_or_insert(step.updated_at_ms);
                if plan_step.contract.output_kind == WorkflowOutputKind::Verification
                    && step.status == WorkflowStepStatus::Completed
                    && step.semantic.verification == WorkflowVerificationState::NotRequired
                {
                    step.semantic.verification = WorkflowVerificationState::Passed;
                }
            }
        }
        Ok(())
    }
}

fn workflow_output_digest(output: &str) -> String {
    format!("{:x}", Sha256::digest(output.as_bytes()))
}
