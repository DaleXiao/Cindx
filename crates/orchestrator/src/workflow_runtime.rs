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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    pub fn limited_by(self, ceiling: Self) -> Self {
        match (self, ceiling) {
            (Self::None, _) | (_, Self::None) => Self::None,
            (Self::ReadOnlyEvidence, _) | (_, Self::ReadOnlyEvidence) => Self::ReadOnlyEvidence,
            (Self::ReadOnlyExploration, Self::ReadOnlyExploration) => Self::ReadOnlyExploration,
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

    pub fn tool_call_budget_within(&self, hard_limit: usize) -> usize {
        match self {
            Self::None => 0,
            Self::ReadOnlyEvidence | Self::ReadOnlyExploration => hard_limit,
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
    #[serde(default)]
    pub minimum_direct_evidence_items: usize,
}

impl Default for WorkflowCompletionCriteria {
    fn default() -> Self {
        Self {
            require_non_empty_output: true,
            require_resolved_inputs: true,
            minimum_evidence_items: 0,
            minimum_direct_evidence_items: 0,
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
        workflow_contribution_key(&self.subtask)
    }
}

pub fn workflow_contribution_key(subtask: &str) -> String {
    subtask
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_lowercase())
        .collect::<Vec<_>>()
        .join("\u{1f}")
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
        if !steps
            .iter()
            .any(|step| step.contract.output_kind == WorkflowOutputKind::Synthesis)
        {
            if let Some(delivery) = steps.last_mut() {
                delivery.contract.output_kind = WorkflowOutputKind::Synthesis;
            }
        }
        for synthesis in steps
            .iter_mut()
            .filter(|step| step.contract.output_kind == WorkflowOutputKind::Synthesis)
        {
            synthesis.tool_policy = WorkflowToolPolicy::None;
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

    pub fn physical_model_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.contract.output_kind != WorkflowOutputKind::Synthesis)
            .map(|step| step.model.as_str())
            .collect::<BTreeSet<_>>()
            .len()
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
        if self.physical_model_count() > self.budget.max_models {
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
            if step.contract.output_kind == WorkflowOutputKind::Synthesis
                && step.tool_policy != WorkflowToolPolicy::None
            {
                return Err(format!(
                    "workflow synthesis step {} cannot execute tools",
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
        validate_typed_workflow_structure(&self.adaptive_workflow(), allowed_models)
    }

    pub fn validate_owner_execution_graph(
        &self,
        required_verification: bool,
    ) -> Result<(), String> {
        crate::owner_execution_graph::validate(self, required_verification)
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
    Inconclusive,
    Passed,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowEvidenceSummary {
    #[serde(default)]
    pub total_items: usize,
    #[serde(default)]
    pub direct_items: usize,
    #[serde(default)]
    pub item_ids_by_source: BTreeMap<String, BTreeSet<String>>,
}

impl WorkflowEvidenceSummary {
    pub fn from_items(
        items: impl IntoIterator<Item = (String, String)>,
        direct_step_id: &str,
    ) -> Self {
        let mut item_ids_by_source = BTreeMap::<String, BTreeSet<String>>::new();
        for (source, item_id) in items {
            let source = source.trim();
            let item_id = item_id.trim();
            if source.is_empty() || item_id.is_empty() {
                continue;
            }
            item_ids_by_source
                .entry(source.to_string())
                .or_default()
                .insert(item_id.to_string());
        }
        let total_items = item_ids_by_source.values().map(BTreeSet::len).sum();
        let direct_items = item_ids_by_source
            .get(direct_step_id)
            .map(BTreeSet::len)
            .unwrap_or_default();
        Self {
            total_items,
            direct_items,
            item_ids_by_source,
        }
    }

    fn validate_for_step(
        &self,
        step_id: &str,
        input_steps: &[String],
        serialized_item_count: usize,
    ) -> Result<(), String> {
        let allowed_sources = input_steps
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(step_id))
            .collect::<BTreeSet<_>>();
        if self.total_items != serialized_item_count
            || self.total_items
                != self
                    .item_ids_by_source
                    .values()
                    .map(BTreeSet::len)
                    .sum::<usize>()
            || self.direct_items
                != self
                    .item_ids_by_source
                    .get(step_id)
                    .map(BTreeSet::len)
                    .unwrap_or_default()
            || self
                .item_ids_by_source
                .keys()
                .any(|source| !allowed_sources.contains(source.as_str()))
        {
            return Err(format!(
                "workflow step {step_id} supplied an inconsistent evidence summary"
            ));
        }
        Ok(())
    }

    fn item_exists(&self, item_id: &str) -> bool {
        self.item_ids_by_source
            .values()
            .any(|items| items.contains(item_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowVerificationVerdict {
    Passed,
    NeedsRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowVerificationReceipt {
    pub schema: String,
    pub verdict: WorkflowVerificationVerdict,
    #[serde(default)]
    pub reviewed_steps: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub unresolved: Vec<String>,
}

impl WorkflowVerificationReceipt {
    pub fn from_worker_output(output: &str) -> Option<Self> {
        const MARKER: &str = "CINDX_VERIFICATION:";
        let mut lines = output.lines().filter(|line| !line.trim().is_empty());
        let receipt_line = lines.next_back()?.trim();
        if lines.any(|line| line.trim().starts_with(MARKER)) {
            return None;
        }
        let payload = receipt_line.strip_prefix(MARKER)?.trim();
        serde_json::from_str(payload).ok()
    }

    fn validate(
        &self,
        input_steps: &[String],
        evidence: &WorkflowEvidenceSummary,
    ) -> Result<(), String> {
        if self.schema != WORKFLOW_VERIFICATION_RECEIPT_SCHEMA {
            return Err("workflow verification receipt schema is unsupported".to_string());
        }
        let expected_steps = input_steps
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let reviewed_steps = self
            .reviewed_steps
            .iter()
            .map(|step| step.trim())
            .filter(|step| !step.is_empty())
            .collect::<BTreeSet<_>>();
        if reviewed_steps.len() != self.reviewed_steps.len() || reviewed_steps != expected_steps {
            return Err(
                "workflow verification receipt does not cover every input step".to_string(),
            );
        }
        let evidence_refs = self
            .evidence_refs
            .iter()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        if evidence_refs.len() != self.evidence_refs.len()
            || evidence_refs.iter().any(|item| !evidence.item_exists(item))
        {
            return Err("workflow verification receipt cites unknown evidence".to_string());
        }
        for input_step in input_steps {
            if let Some(items) = evidence.item_ids_by_source.get(input_step) {
                if !items.is_empty() && items.is_disjoint(&evidence_refs) {
                    return Err(format!(
                        "workflow verification receipt cites no evidence from input step {input_step}"
                    ));
                }
            }
        }
        if self.verdict == WorkflowVerificationVerdict::Passed
            && self.unresolved.iter().any(|item| !item.trim().is_empty())
        {
            return Err(
                "a passing workflow verification receipt cannot retain unresolved findings"
                    .to_string(),
            );
        }
        Ok(())
    }
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
    pub direct_evidence_count: usize,
    #[serde(default)]
    pub evidence_item_ids_by_source: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub verification: WorkflowVerificationState,
    #[serde(default)]
    pub verification_receipt: Option<WorkflowVerificationReceipt>,
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
    #[serde(default)]
    pub verification_repair_rounds: usize,
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
    pub fn workflow_verification_satisfied(&self, required: bool) -> bool {
        let verification_steps = self
            .plan
            .steps
            .iter()
            .filter(|step| step.contract.output_kind == WorkflowOutputKind::Verification)
            .collect::<Vec<_>>();
        if verification_steps.is_empty() {
            return !required;
        }
        verification_steps.iter().all(|plan_step| {
            self.steps.get(&plan_step.id).is_some_and(|step| {
                step.status == WorkflowStepStatus::Completed
                    && step.semantic.completion_satisfied
                    && step.semantic.verification == WorkflowVerificationState::Passed
            })
        })
    }

    pub fn begin_verification_repair(
        &mut self,
        verification_step_id: &str,
        now_ms: u64,
    ) -> Result<Vec<String>, String> {
        const MAX_VERIFICATION_REPAIR_ROUNDS: usize = 1;
        if !self.plan.steps.iter().any(|step| {
            step.id == verification_step_id
                && step.contract.output_kind == WorkflowOutputKind::Verification
        }) {
            return Err(format!(
                "workflow step {verification_step_id} is not a verification step"
            ));
        }
        let audited_steps = {
            let verification_step = self.steps.get(verification_step_id).ok_or_else(|| {
                format!("unknown workflow checkpoint step: {verification_step_id}")
            })?;
            if verification_step.status != WorkflowStepStatus::Completed {
                return Err(format!(
                    "workflow verification step {verification_step_id} has not completed"
                ));
            }
            if verification_step.verification_repair_rounds >= MAX_VERIFICATION_REPAIR_ROUNDS {
                return Err(format!(
                    "workflow verification step {verification_step_id} exhausted its one-repair budget"
                ));
            }
            if verification_step.semantic.verification != WorkflowVerificationState::Degraded {
                return Err(format!(
                    "workflow verification step {verification_step_id} is not awaiting revision"
                ));
            }
            let receipt = verification_step
                .semantic
                .verification_receipt
                .as_ref()
                .ok_or_else(|| {
                    format!("workflow verification step {verification_step_id} has no receipt")
                })?;
            if receipt.verdict != WorkflowVerificationVerdict::NeedsRevision
                || receipt.reviewed_steps.is_empty()
            {
                return Err(format!(
                    "workflow verification step {verification_step_id} verdict does not require revision"
                ));
            }
            let audited = receipt.reviewed_steps.clone();
            for audited_id in &audited {
                let audited_step = self.steps.get(audited_id).ok_or_else(|| {
                    format!("workflow verification receipt references missing step {audited_id}")
                })?;
                if audited_step.status != WorkflowStepStatus::Completed {
                    return Err(format!(
                        "audited workflow step {audited_id} is not completed"
                    ));
                }
            }
            audited
        };
        for audited_id in &audited_steps {
            let audited_step = self.steps.get_mut(audited_id).expect("validated above");
            audited_step.status = WorkflowStepStatus::Pending;
            audited_step.error = None;
            audited_step.updated_at_ms = now_ms;
        }
        let verification_step = self
            .steps
            .get_mut(verification_step_id)
            .expect("verification step validated above");
        verification_step.status = WorkflowStepStatus::Pending;
        verification_step.semantic.verification = WorkflowVerificationState::default();
        verification_step.verification_repair_rounds += 1;
        verification_step.updated_at_ms = now_ms;
        self.additional_model_turns_per_step += 1;
        self.updated_at_ms = now_ms;
        Ok(audited_steps)
    }

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
                        verification_repair_rounds: 0,
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
        self.complete_step_inner(
            step_id,
            Some(model),
            output,
            evidence_json,
            None,
            None,
            now_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_step_with_evidence(
        &mut self,
        step_id: &str,
        model: &str,
        output: String,
        evidence_json: String,
        evidence: WorkflowEvidenceSummary,
        verification_receipt: Option<WorkflowVerificationReceipt>,
        now_ms: u64,
    ) -> Result<(), String> {
        self.complete_step_inner(
            step_id,
            Some(model),
            output,
            evidence_json,
            Some(evidence),
            verification_receipt,
            now_ms,
        )
    }

    pub fn complete_owner_handoff(
        &mut self,
        step_id: &str,
        output: String,
        evidence_json: String,
        evidence: WorkflowEvidenceSummary,
        now_ms: u64,
    ) -> Result<(), String> {
        let required_verification = self
            .plan
            .steps
            .iter()
            .any(|step| step.contract.output_kind == WorkflowOutputKind::Verification);
        self.plan
            .validate_owner_execution_graph(required_verification)?;
        let plan_step = self
            .plan
            .steps
            .last()
            .filter(|step| {
                step.id == step_id && step.contract.output_kind == WorkflowOutputKind::Synthesis
            })
            .ok_or_else(|| "owner handoff must complete the final synthesis sink".to_string())?;
        let checkpoint = self
            .steps
            .get(&plan_step.id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        if checkpoint.attempts != 0 {
            return Err("owner handoff cannot consume a model attempt".to_string());
        }
        self.complete_step_inner(
            step_id,
            None,
            output,
            evidence_json,
            Some(evidence),
            None,
            now_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_step_inner(
        &mut self,
        step_id: &str,
        model: Option<&str>,
        output: String,
        evidence_json: String,
        evidence_summary: Option<WorkflowEvidenceSummary>,
        verification_receipt: Option<WorkflowVerificationReceipt>,
        now_ms: u64,
    ) -> Result<(), String> {
        let plan_step = self
            .plan
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .cloned()
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
        let serialized_evidence_count = serde_json::from_str::<serde_json::Value>(&evidence_json)
            .ok()
            .and_then(|value| value.as_array().map(Vec::len))
            .unwrap_or_default();
        let evidence = if let Some(evidence) = evidence_summary {
            evidence.validate_for_step(
                step_id,
                &plan_step.contract.input_steps,
                serialized_evidence_count,
            )?;
            evidence
        } else {
            WorkflowEvidenceSummary {
                total_items: serialized_evidence_count,
                ..WorkflowEvidenceSummary::default()
            }
        };
        if evidence.total_items < plan_step.contract.completion.minimum_evidence_items {
            return Err(format!(
                "workflow step {step_id} produced {} evidence items but requires {}",
                evidence.total_items, plan_step.contract.completion.minimum_evidence_items
            ));
        }
        if evidence.direct_items < plan_step.contract.completion.minimum_direct_evidence_items {
            return Err(format!(
                "workflow step {step_id} produced {} direct evidence items but requires {}",
                evidence.direct_items, plan_step.contract.completion.minimum_direct_evidence_items
            ));
        }
        let (verification, verification_receipt) = if plan_step.contract.output_kind
            == WorkflowOutputKind::Verification
        {
            match verification_receipt {
                Some(receipt)
                    if receipt
                        .validate(&plan_step.contract.input_steps, &evidence)
                        .is_ok() =>
                {
                    let verification = match &receipt.verdict {
                        WorkflowVerificationVerdict::Passed => WorkflowVerificationState::Passed,
                        WorkflowVerificationVerdict::NeedsRevision => {
                            WorkflowVerificationState::Degraded
                        }
                    };
                    (verification, Some(receipt))
                }
                _ => (WorkflowVerificationState::Inconclusive, None),
            }
        } else {
            (WorkflowVerificationState::NotRequired, None)
        };
        let semantic = WorkflowStepSemanticState {
            input_digests,
            output_digest: workflow_output_digest(&output),
            output_kind: plan_step.contract.output_kind.clone(),
            evidence_count: evidence.total_items,
            direct_evidence_count: evidence.direct_items,
            evidence_item_ids_by_source: evidence.item_ids_by_source,
            verification,
            verification_receipt,
            completion_satisfied: true,
            completed_at_ms: Some(now_ms),
        };
        let step = self
            .steps
            .get_mut(step_id)
            .ok_or_else(|| format!("unknown workflow checkpoint step: {step_id}"))?;
        step.status = WorkflowStepStatus::Completed;
        if let Some(model) = model {
            step.attempts = step.attempts.max(1);
            step.model = model.to_string();
        }
        step.output = Some(output);
        step.evidence_count = semantic.evidence_count;
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
        self.finalized && self.delivery_is_complete()
    }

    pub fn continue_with_budget(&mut self, additional_turns_per_step: usize, now_ms: u64) {
        self.continuations = self.continuations.saturating_add(1);
        self.additional_model_turns_per_step = self
            .additional_model_turns_per_step
            .saturating_add(additional_turns_per_step.max(1));
        self.updated_at_ms = now_ms;
    }

    pub fn finalize(&mut self, final_output: String, now_ms: u64) -> Result<(), String> {
        let final_step_id = self.delivery_target_step_id()?;
        let checkpoint = self
            .steps
            .get_mut(&final_step_id)
            .ok_or_else(|| "workflow checkpoint is missing its delivery target".to_string())?;
        if checkpoint.status != WorkflowStepStatus::Completed {
            return Err(
                "workflow checkpoint cannot finalize before its delivery target".to_string(),
            );
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

    pub fn from_json_for_untrusted_handoff(value: &str) -> Result<Self, String> {
        let mut checkpoint = serde_json::from_str::<Self>(value)
            .map_err(|error| format!("workflow checkpoint JSON is invalid: {error}"))?;
        checkpoint.plan.reconcile_step_contracts();
        checkpoint.reconcile_semantic_state()?;
        let embedded_models = checkpoint
            .plan
            .steps
            .iter()
            .map(|step| step.model.clone())
            .chain(checkpoint.steps.values().map(|step| step.model.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if embedded_models.iter().any(|model| model.trim().is_empty()) {
            return Err("workflow checkpoint contains an empty model identity".to_string());
        }
        checkpoint.validate(&embedded_models)?;
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
                if plan_step.contract.output_kind == WorkflowOutputKind::Verification {
                    let evidence = WorkflowEvidenceSummary {
                        total_items: step.semantic.evidence_count,
                        direct_items: step.semantic.direct_evidence_count,
                        item_ids_by_source: step.semantic.evidence_item_ids_by_source.clone(),
                    };
                    step.semantic.verification = match step.semantic.verification_receipt.as_ref() {
                        Some(receipt)
                            if receipt
                                .validate(&plan_step.contract.input_steps, &evidence)
                                .is_ok() =>
                        {
                            match &receipt.verdict {
                                WorkflowVerificationVerdict::Passed => {
                                    WorkflowVerificationState::Passed
                                }
                                WorkflowVerificationVerdict::NeedsRevision => {
                                    WorkflowVerificationState::Degraded
                                }
                            }
                        }
                        _ => WorkflowVerificationState::Inconclusive,
                    };
                }
            }
        }
        Ok(())
    }
}

fn workflow_output_digest(output: &str) -> String {
    format!("{:x}", Sha256::digest(output.as_bytes()))
}
