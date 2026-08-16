use crate::{
    causal_route_action_id_v2, matching_collaboration_evidence_for_context,
    minimum_team_uplift_bps, select_causal_route_v2, AgentDecisionCalibration,
    AutoComputationAssessment, CausalRouteSelectionV2, ConductorExecutionContract,
    ConductorFallbackPolicy, ConductorStopPolicy, MatchedCollaborationEvidenceTeacher,
    ModelCandidate, OrchestrationPolicy, RouteFeatureRequest, RouteFeatureSnapshotV2,
    RoutingContext, RoutingDecision, TaskClass, WorkflowOutputKind, WorkflowToolPolicy,
    AUTO_COLLABORATION_MIN_CONFIDENCE_BPS, AUTO_COLLABORATION_MIN_UPLIFT_BPS,
};
use agent_core::{Metadata, ModelRole};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

pub const AGENT_RUN_DECISION_SCHEMA: &str = "cindx.agent-run-decision.v1";
pub const MAX_RUN_DECISION_QUERY_CHARS: usize = 2_000;
pub const MAX_RUN_DECISION_RATIONALE_CHARS: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionMode {
    Direct,
    Workflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRetrievalChannel {
    Semantic,
    FileSearch,
    GraphDirect,
    GraphWalk,
}

impl WorkspaceRetrievalChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Semantic => "semantic_rag",
            Self::FileSearch => "file_search",
            Self::GraphDirect => "graph_recall",
            Self::GraphWalk => "graph_walk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecallPolicy {
    None,
    Relevant,
    Comprehensive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentVerificationPolicy {
    None,
    SelfCheck,
    Independent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolRequirement {
    #[default]
    None,
    ReadOnly,
    Effects,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEffectAuthority {
    Forbidden,
    #[default]
    Allowed,
    Required,
}

impl AgentEffectAuthority {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::Allowed => "allowed",
            Self::Required => "required",
        }
    }
}

impl AgentToolRequirement {
    const fn strength(self) -> u8 {
        match self {
            Self::None => 0,
            Self::ReadOnly => 1,
            Self::Effects => 2,
        }
    }

    const fn satisfies(self, minimum: Self) -> bool {
        self.strength() >= minimum.strength()
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ReadOnly => "read_only",
            Self::Effects => "effects",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentRouteRequirements {
    pub minimum_tool_requirement: AgentToolRequirement,
    pub effect_authority: AgentEffectAuthority,
    pub image_input_required: bool,
}

impl AgentRouteRequirements {
    /// The only authority under which the owner execution graph may widen to
    /// two read-only specialist roots: the prompt explicitly forbids every
    /// effect, so the whole run (workers and Owner) is read-only. Allowed and
    /// Required authorities keep the single-specialist invariant fail-closed.
    pub fn parallel_read_only_exploration_authorized(self) -> bool {
        self.effect_authority == AgentEffectAuthority::Forbidden
    }

    pub fn apply_to_direct(self, mut decision: AgentRunDecision) -> AgentRunDecision {
        if !decision
            .tool_requirement
            .satisfies(self.minimum_tool_requirement)
        {
            decision.tool_requirement = self.minimum_tool_requirement;
        }
        decision.vision_required |= self.image_input_required;
        decision
    }

    pub fn model_satisfies(self, model: &str, model_candidates: &[ModelCandidate]) -> bool {
        let needs_tools = self.minimum_tool_requirement != AgentToolRequirement::None;
        (!needs_tools
            || model_candidates
                .iter()
                .any(|candidate| candidate.name.trim() == model.trim() && candidate.supports_tools))
            && (!self.image_input_required
                || model_candidates.iter().any(|candidate| {
                    candidate.name.trim() == model.trim() && candidate.supports_vision
                }))
    }

    pub fn validate_decision(
        self,
        decision: &AgentRunDecision,
        model_candidates: &[ModelCandidate],
    ) -> Result<(), String> {
        if self.effect_authority == AgentEffectAuthority::Forbidden
            && decision.tool_requirement == AgentToolRequirement::Effects
        {
            return Err("run decision exceeds the prompt effect authority".to_string());
        }
        if self.effect_authority == AgentEffectAuthority::Required
            && decision.tool_requirement != AgentToolRequirement::Effects
        {
            return Err("run decision omitted required effect authority".to_string());
        }
        if !decision
            .tool_requirement
            .satisfies(self.minimum_tool_requirement)
        {
            return Err(format!(
                "run decision tool requirement {} is below the runtime minimum {}",
                decision.tool_requirement.label(),
                self.minimum_tool_requirement.label(),
            ));
        }
        if self.image_input_required && !decision.vision_required {
            return Err("run decision omitted vision for the active image input".to_string());
        }

        if decision.tool_requirement != AgentToolRequirement::None
            && !model_candidates.iter().any(|candidate| {
                candidate.name.trim() == decision.primary_model.trim() && candidate.supports_tools
            })
        {
            return Err(format!(
                "selected model {} is not configured with tool capability",
                decision.primary_model
            ));
        }
        if decision.vision_required
            && !model_candidates.iter().any(|candidate| {
                candidate.name.trim() == decision.primary_model.trim() && candidate.supports_vision
            })
        {
            return Err(format!(
                "selected model {} is not configured with vision capability",
                decision.primary_model
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRiskLevel {
    Low,
    Elevated,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRetrievalPlan {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub channels: BTreeSet<WorkspaceRetrievalChannel>,
    #[serde(default = "default_retrieval_limit")]
    pub max_results: usize,
}

fn default_retrieval_limit() -> usize {
    8
}

impl WorkspaceRetrievalPlan {
    pub fn none() -> Self {
        Self {
            query: String::new(),
            channels: BTreeSet::new(),
            max_results: default_retrieval_limit(),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.channels.is_empty()
    }

    pub fn mode_label(&self) -> String {
        if self.channels.is_empty() {
            return "none".to_string();
        }
        self.channels
            .iter()
            .copied()
            .map(WorkspaceRetrievalChannel::label)
            .collect::<Vec<_>>()
            .join("+")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecallPlan {
    pub policy: MemoryRecallPolicy,
    #[serde(default)]
    pub query: String,
}

impl MemoryRecallPlan {
    pub fn none() -> Self {
        Self {
            policy: MemoryRecallPolicy::None,
            query: String::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.policy != MemoryRecallPolicy::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunDecision {
    pub schema: String,
    pub task_class: TaskClass,
    pub execution: AgentExecutionMode,
    pub primary_model: String,
    pub tool_requirement: AgentToolRequirement,
    #[serde(default)]
    pub vision_required: bool,
    pub risk_level: AgentRiskLevel,
    pub retrieval: WorkspaceRetrievalPlan,
    pub memory: MemoryRecallPlan,
    pub verification: AgentVerificationPolicy,
    pub max_parallelism: usize,
    pub min_successful_branches: usize,
    pub distinct_contributions: usize,
    pub estimated_steps: usize,
    pub expected_uplift_bps: u16,
    pub confidence_bps: u16,
    pub stop_policy: ConductorStopPolicy,
    #[serde(default)]
    pub rationale: String,
    #[serde(skip)]
    pub calibration_reason: Option<String>,
    #[serde(skip)]
    pub calibration: Option<AgentDecisionCalibration>,
    #[serde(skip)]
    pub computation_value: Option<AutoComputationAssessment>,
    #[serde(skip)]
    pub causal_route: Option<CausalRouteSelectionV2>,
}

impl AgentRunDecision {
    pub fn direct(primary_model: impl Into<String>) -> Self {
        Self {
            schema: AGENT_RUN_DECISION_SCHEMA.to_string(),
            task_class: TaskClass::General,
            execution: AgentExecutionMode::Direct,
            primary_model: primary_model.into(),
            tool_requirement: AgentToolRequirement::None,
            vision_required: false,
            risk_level: AgentRiskLevel::Low,
            retrieval: WorkspaceRetrievalPlan::none(),
            memory: MemoryRecallPlan::none(),
            verification: AgentVerificationPolicy::SelfCheck,
            max_parallelism: 1,
            min_successful_branches: 1,
            distinct_contributions: 0,
            estimated_steps: 1,
            expected_uplift_bps: 0,
            confidence_bps: 0,
            stop_policy: ConductorStopPolicy::FirstVerified,
            rationale: "safe direct fallback".to_string(),
            calibration_reason: None,
            calibration: None,
            computation_value: None,
            causal_route: None,
        }
    }

    pub fn degraded_conductor_fallback(
        primary_model: impl Into<String>,
        _effort: &str,
        _configured_model_count: usize,
        _max_parallelism: usize,
        reason: impl Into<String>,
    ) -> Self {
        let primary_model = primary_model.into();
        let reason = reason.into();
        let mut fallback = Self::direct(primary_model);
        fallback.rationale = bounded_chars(
            &format!(
                "Conductor unavailable; preserving the strongest executable baseline with degraded direct execution: {reason}"
            ),
            MAX_RUN_DECISION_RATIONALE_CHARS,
        );
        fallback
    }

    pub fn validate(
        &self,
        allowed_models: &[String],
        max_parallelism: usize,
        parallel_read_only_authorized: bool,
    ) -> Result<(), String> {
        if self.schema != AGENT_RUN_DECISION_SCHEMA {
            return Err(format!(
                "unsupported agent run decision schema: {}",
                self.schema
            ));
        }
        if self.primary_model.trim().is_empty()
            || !allowed_models
                .iter()
                .any(|model| model.trim() == self.primary_model.trim())
        {
            return Err(
                "run decision primary_model is not in the configured model pool".to_string(),
            );
        }
        if self.retrieval.query.chars().count() > MAX_RUN_DECISION_QUERY_CHARS
            || self.memory.query.chars().count() > MAX_RUN_DECISION_QUERY_CHARS
        {
            return Err("run decision query exceeds the harness limit".to_string());
        }
        if self.rationale.chars().count() > MAX_RUN_DECISION_RATIONALE_CHARS {
            return Err("run decision rationale exceeds the harness limit".to_string());
        }
        if self.retrieval.enabled() && self.retrieval.query.trim().is_empty() {
            return Err("workspace retrieval requires a focused query".to_string());
        }
        if !(1..=24).contains(&self.retrieval.max_results) {
            return Err("workspace retrieval max_results must be between 1 and 24".to_string());
        }
        if self
            .retrieval
            .channels
            .contains(&WorkspaceRetrievalChannel::GraphWalk)
            && !self.retrieval.channels.iter().any(|channel| {
                matches!(
                    channel,
                    WorkspaceRetrievalChannel::Semantic
                        | WorkspaceRetrievalChannel::FileSearch
                        | WorkspaceRetrievalChannel::GraphDirect
                )
            })
        {
            return Err(
                "graph_walk requires at least one seed-producing retrieval channel".to_string(),
            );
        }
        if self.memory.enabled() && self.memory.query.trim().is_empty() {
            return Err("memory recall requires a focused query".to_string());
        }
        if !self.memory.enabled() && !self.memory.query.trim().is_empty() {
            return Err("memory query must be empty when memory recall is disabled".to_string());
        }
        if self.expected_uplift_bps > 10_000 || self.confidence_bps > 10_000 {
            return Err(
                "run decision probability scores must be between 0 and 10000 bps".to_string(),
            );
        }
        if !(1..=5).contains(&self.estimated_steps) {
            return Err("run decision estimated_steps must be between 1 and 5".to_string());
        }
        match self.execution {
            AgentExecutionMode::Direct => {
                if self.max_parallelism != 1
                    || self.min_successful_branches != 1
                    || self.distinct_contributions != 0
                    || self.stop_policy != ConductorStopPolicy::FirstVerified
                    || self.verification == AgentVerificationPolicy::Independent
                {
                    return Err("direct execution must use one branch, first_verified, and no independent verifier".to_string());
                }
            }
            AgentExecutionMode::Workflow => {
                let parallel_read_only = parallel_read_only_authorized
                    && self.tool_requirement != AgentToolRequirement::Effects
                    && self.max_parallelism == 2;
                let specialist_count = if parallel_read_only { 2 } else { 1 };
                if !(1..=2).contains(&self.max_parallelism)
                    || self.max_parallelism != specialist_count
                    || max_parallelism == 0
                    || self.min_successful_branches != 1
                    || self.distinct_contributions != specialist_count
                {
                    return Err("workflow execution must use one specialist contribution, or two read-only specialist contributions under a forbidden effect authority".to_string());
                }
                let expected_steps = specialist_count
                    + usize::from(self.verification == AgentVerificationPolicy::Independent)
                    + 1;
                if self.estimated_steps != expected_steps {
                    return Err(format!(
                        "workflow estimated_steps must be {expected_steps} for this owner handoff graph"
                    ));
                }
                match self.verification {
                    AgentVerificationPolicy::None
                        if self.stop_policy != ConductorStopPolicy::FirstVerified => {}
                    AgentVerificationPolicy::Independent => {}
                    AgentVerificationPolicy::None => {
                        return Err(
                            "workflow without an independent verifier cannot use first_verified"
                                .to_string(),
                        );
                    }
                    AgentVerificationPolicy::SelfCheck => {
                        return Err("workflow execution supports only no verifier or one independent verifier".to_string());
                    }
                }
            }
        }
        Ok(())
    }

    pub fn policy(&self) -> OrchestrationPolicy {
        match self.execution {
            AgentExecutionMode::Direct => OrchestrationPolicy::Single,
            AgentExecutionMode::Workflow => OrchestrationPolicy::BestOfN {
                candidates: self.max_parallelism.max(1),
            },
        }
    }

    pub fn learning_signature(&self) -> String {
        format!(
            "{}:model={}:execution={:?}:tools={:?}:retrieval={}:memory={:?}:vision={}:risk={:?}:parallelism={}:verify={:?}",
            self.task_class.label(),
            self.primary_model.trim(),
            self.execution,
            self.tool_requirement,
            self.retrieval.mode_label(),
            self.memory.policy,
            u8::from(self.vision_required),
            self.risk_level,
            self.max_parallelism,
            self.verification,
        )
        .to_ascii_lowercase()
    }

    pub fn routing_context(
        &self,
        prompt: &str,
        model_candidates: Vec<ModelCandidate>,
    ) -> RoutingContext {
        RoutingContext {
            task_class: self.task_class.clone(),
            prompt_length: prompt.chars().count(),
            needs_tools: self.tool_requirement != AgentToolRequirement::None,
            needs_retrieval: self.retrieval.enabled(),
            needs_multi_model: self.execution == AgentExecutionMode::Workflow
                && self.max_parallelism > 1,
            needs_vision: self.vision_required,
            high_stakes: self.risk_level == AgentRiskLevel::High,
            complexity_score: u8::try_from(
                self.estimated_steps
                    .saturating_add(self.distinct_contributions)
                    .saturating_sub(1),
            )
            .unwrap_or(u8::MAX)
            .min(8),
            estimated_steps: u8::try_from(self.estimated_steps).unwrap_or(5).min(5),
            parallelizable: self.max_parallelism > 1,
            verification_required: self.verification != AgentVerificationPolicy::None,
            latency_sensitive: self.execution == AgentExecutionMode::Direct,
            user_policy_override: None,
            model_candidates,
        }
    }

    pub fn routing_decision(&self) -> RoutingDecision {
        let mut metadata = Metadata::new();
        metadata.insert("router".to_string(), "dynamic_conductor_v1".to_string());
        metadata.insert(
            "task_class".to_string(),
            self.task_class.label().to_string(),
        );
        metadata.insert(
            "execution".to_string(),
            format!("{:?}", self.execution).to_ascii_lowercase(),
        );
        metadata.insert("retrieval_mode".to_string(), self.retrieval.mode_label());
        metadata.insert(
            "memory_policy".to_string(),
            format!("{:?}", self.memory.policy).to_ascii_lowercase(),
        );
        metadata.insert(
            "max_parallelism".to_string(),
            self.max_parallelism.to_string(),
        );
        metadata.insert(
            "estimated_steps".to_string(),
            self.estimated_steps.to_string(),
        );
        metadata.insert(
            "expected_uplift_bps".to_string(),
            self.expected_uplift_bps.to_string(),
        );
        metadata.insert(
            "confidence_bps".to_string(),
            self.confidence_bps.to_string(),
        );
        metadata.extend(self.route_observability_metadata());
        RoutingDecision {
            policy: self.policy(),
            model: self.primary_model.clone(),
            verifier_role: (self.verification == AgentVerificationPolicy::Independent)
                .then_some(ModelRole::Reviewer),
            retrieval_mode: self.retrieval.mode_label(),
            explanation: self.rationale.clone(),
            metadata,
        }
    }

    pub fn execution_contract(&self, effort: &str) -> ConductorExecutionContract {
        let effort = match effort.trim().to_ascii_lowercase().as_str() {
            "fast" => "fast",
            "pro" => "pro",
            _ => "auto",
        }
        .to_string();
        let verification_required = self.verification != AgentVerificationPolicy::None;
        let max_workflow_steps = self
            .estimated_steps
            .max(crate::minimum_workflow_steps(
                self.distinct_contributions,
                verification_required,
            ))
            .clamp(1, crate::MAX_ADAPTIVE_WORKFLOW_STEPS);
        ConductorExecutionContract {
            task_class: self.task_class.clone(),
            effort: effort.clone(),
            policy: self.policy(),
            expected_uplift_bps: self.expected_uplift_bps,
            confidence_bps: self.confidence_bps,
            max_parallelism: self.max_parallelism,
            max_workflow_steps,
            min_successful_branches: self.min_successful_branches,
            verification_required,
            terminal_model_call_reserve: match effort.as_str() {
                "fast" => 1,
                "pro" => 3,
                _ => 2,
            },
            stop_policy: self.stop_policy,
            fallback_policy: if self.retrieval.enabled()
                || self.tool_requirement != AgentToolRequirement::None
            {
                ConductorFallbackPolicy::BestKnownResult
            } else {
                ConductorFallbackPolicy::SinglePath
            },
            min_team_uplift_bps: minimum_team_uplift_bps(&effort),
            min_distinct_contributions: self.distinct_contributions,
            requires_synthesis: self.distinct_contributions > 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunDecisionRequest {
    pub objective: String,
    pub recent_context: String,
    pub effort: String,
    pub conductor_model: String,
    pub allowed_models: Vec<String>,
    pub model_candidates: Vec<ModelCandidate>,
    pub max_parallelism: usize,
    pub evolved_directive: String,
    pub historical_evidence: String,
    pub matched_collaboration_evidence: Arc<MatchedCollaborationEvidenceTeacher>,
    pub execution_constraints: String,
    pub route_requirements: AgentRouteRequirements,
    pub budget_fingerprint: Option<String>,
    pub prompt_profile_sha256: String,
    /// Configured default model for the requested effort tier, when the user
    /// pinned one. Anchors the conductor's primary_model choice without
    /// removing its authority to pick another configured model.
    pub preferred_primary_model: Option<String>,
    /// Evaluation-only treatment constraint. Production requests leave this
    /// unset and retain Conductor authority.
    pub required_execution: Option<AgentExecutionMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunDecisionDraft {
    pub conductor_candidate: AgentRunDecision,
    pub compatibility_route: CausalRouteSelectionV2,
    pub workflow_plan: Option<WorkflowPlanProposal>,
}

impl AgentRunDecisionDraft {
    pub fn into_legacy_selected(mut self) -> AgentRunDecision {
        self.conductor_candidate.causal_route = Some(self.compatibility_route);
        self.conductor_candidate
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPlanProposal {
    pub steps: Vec<WorkflowPlanProposalStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPlanProposalStep {
    pub id: String,
    pub role: String,
    pub model: String,
    pub subtask: String,
    #[serde(default)]
    pub access: Vec<String>,
    pub output_kind: WorkflowOutputKind,
    pub tool_policy: WorkflowToolPolicy,
}

impl WorkflowPlanProposal {
    /// Validate the stable proposal shape retained by persisted v1 receipts.
    pub fn validate_v1(
        &self,
        decision: &AgentRunDecision,
        allowed_models: &[String],
    ) -> Result<(), String> {
        if decision.execution != AgentExecutionMode::Workflow {
            return Err("a workflow proposal requires workflow execution".to_string());
        }
        if self.steps.is_empty() || self.steps.len() != decision.estimated_steps {
            return Err(
                "workflow proposal step count must match the run decision estimate".to_string(),
            );
        }

        let mut prior_ids = BTreeSet::new();
        for (index, step) in self.steps.iter().enumerate() {
            let id = step.id.trim();
            if id.is_empty()
                || step.role.trim().is_empty()
                || step.model.trim().is_empty()
                || step.subtask.trim().is_empty()
                || prior_ids.contains(id)
            {
                return Err(
                    "workflow proposal steps require unique ids, roles, and subtasks".to_string(),
                );
            }
            if !allowed_models
                .iter()
                .any(|model| model.trim() == step.model.trim())
            {
                return Err(format!(
                    "workflow proposal model {} is not configured",
                    step.model
                ));
            }
            if step
                .access
                .iter()
                .any(|dependency| !prior_ids.contains(dependency.trim()))
            {
                return Err(format!(
                    "workflow proposal step {} references a missing or later dependency",
                    step.id
                ));
            }
            prior_ids.insert(id.to_string());
            if index + 1 == self.steps.len() {
                if step.output_kind != WorkflowOutputKind::Synthesis
                    || step.tool_policy != WorkflowToolPolicy::None
                {
                    return Err(
                        "workflow proposal must end with a tool-free synthesis step".to_string()
                    );
                }
            } else if step.output_kind == WorkflowOutputKind::Synthesis {
                return Err(
                    "workflow proposal can contain only one final synthesis step".to_string(),
                );
            }
        }

        let root_steps = self
            .steps
            .iter()
            .take(self.steps.len().saturating_sub(1))
            .filter(|step| step.access.is_empty())
            .collect::<Vec<_>>();
        if root_steps.len() != decision.distinct_contributions
            || root_steps.len() > decision.max_parallelism
        {
            return Err(
                "workflow proposal roots must match the declared independent contributions"
                    .to_string(),
            );
        }
        let contribution_keys = root_steps
            .iter()
            .map(|step| crate::workflow_contribution_key(&step.subtask))
            .collect::<BTreeSet<_>>();
        if contribution_keys.len() != root_steps.len() {
            return Err("workflow proposal repeats an independent contribution".to_string());
        }

        let final_step = self
            .steps
            .last()
            .ok_or_else(|| "workflow proposal has no synthesis step".to_string())?;
        let reachable = final_step
            .access
            .iter()
            .map(|dependency| dependency.trim())
            .collect::<BTreeSet<_>>();
        if root_steps
            .iter()
            .any(|root| !proposal_step_reaches(&self.steps, root.id.trim(), &reachable))
        {
            return Err("every workflow proposal branch must reach synthesis".to_string());
        }

        if decision.verification == AgentVerificationPolicy::Independent {
            let root_ids = root_steps
                .iter()
                .map(|step| step.id.trim())
                .collect::<BTreeSet<_>>();
            let covers_roots = self.steps.iter().any(|step| {
                step.output_kind == WorkflowOutputKind::Verification
                    && root_ids.iter().all(|root| {
                        step.access
                            .iter()
                            .any(|dependency| dependency.trim() == *root)
                    })
                    && proposal_step_reaches(&self.steps, step.id.trim(), &reachable)
            });
            if !covers_roots {
                return Err(
                    "independent verification must audit every proposal root and reach synthesis"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    /// Validate the current production Owner execution topology.
    pub fn validate_owner_execution_graph(
        &self,
        decision: &AgentRunDecision,
        allowed_models: &[String],
        parallel_read_only_authorized: bool,
    ) -> Result<(), String> {
        self.validate_v1(decision, allowed_models)?;
        let specialist_steps = self
            .steps
            .iter()
            .filter(|step| {
                matches!(
                    step.output_kind,
                    WorkflowOutputKind::Analysis | WorkflowOutputKind::Evidence
                )
            })
            .collect::<Vec<_>>();
        let verification_steps = self
            .steps
            .iter()
            .filter(|step| step.output_kind == WorkflowOutputKind::Verification)
            .collect::<Vec<_>>();
        let parallel_read_only = parallel_read_only_authorized
            && decision.max_parallelism == 2
            && decision.tool_requirement != AgentToolRequirement::Effects;
        let expected_specialists = if parallel_read_only { 2 } else { 1 };
        if specialist_steps.len() != expected_specialists
            || specialist_steps.iter().any(|step| !step.access.is_empty())
            || verification_steps.len()
                != usize::from(decision.verification == AgentVerificationPolicy::Independent)
        {
            return Err(
                if parallel_read_only {
                    "workflow proposal must contain two read-only specialists and at most one required independent verifier"
                } else {
                    "workflow proposal must contain one specialist and at most one required independent verifier"
                }
                .to_string(),
            );
        }
        let specialist_ids = specialist_steps
            .iter()
            .map(|step| step.id.trim().to_string())
            .collect::<BTreeSet<_>>();
        if specialist_ids.len() != specialist_steps.len() {
            return Err("workflow proposal specialists must have distinct step ids".to_string());
        }
        if verification_steps.first().is_some_and(|verification| {
            let audited = verification
                .access
                .iter()
                .map(|dependency| dependency.trim().to_string())
                .collect::<BTreeSet<_>>();
            verification.tool_policy != WorkflowToolPolicy::None
                || verification.access.len() != specialist_ids.len()
                || audited != specialist_ids
                || specialist_steps
                    .iter()
                    .any(|specialist| specialist.model == verification.model)
        }) {
            return Err(
                "independent verifier must use a different configured model and audit only the specialist output without tools"
                    .to_string(),
            );
        }
        let final_step = self
            .steps
            .last()
            .expect("v1 validation requires a final synthesis step");
        let required_final_inputs: Vec<String> = match verification_steps.first() {
            Some(verification) => vec![verification.id.trim().to_string()],
            None => specialist_ids.into_iter().collect(),
        };
        if final_step.access.len() != required_final_inputs.len()
            || !required_final_inputs.iter().all(|required| {
                final_step
                    .access
                    .iter()
                    .any(|dependency| dependency.trim() == required)
            })
        {
            return Err(
                "workflow owner handoff must depend on the final specialist or verifier output"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn validate(
        &self,
        decision: &AgentRunDecision,
        allowed_models: &[String],
        parallel_read_only_authorized: bool,
    ) -> Result<(), String> {
        self.validate_owner_execution_graph(decision, allowed_models, parallel_read_only_authorized)
    }
}

fn proposal_step_reaches(
    steps: &[WorkflowPlanProposalStep],
    target: &str,
    frontier: &BTreeSet<&str>,
) -> bool {
    if frontier.contains(target) {
        return true;
    }
    let next = steps
        .iter()
        .filter(|step| frontier.contains(step.id.trim()))
        .flat_map(|step| step.access.iter().map(|dependency| dependency.trim()))
        .collect::<BTreeSet<_>>();
    !next.is_empty() && proposal_step_reaches(steps, target, &next)
}

#[derive(Debug, Clone)]
pub struct AgentRunDecisionHarness {
    request: AgentRunDecisionRequest,
}

impl AgentRunDecisionHarness {
    pub fn new(request: AgentRunDecisionRequest) -> Self {
        Self { request }
    }

    pub fn planning_prompt(&self) -> String {
        let request = &self.request;
        let models = request
            .allowed_models
            .iter()
            .map(|model| {
                let candidates = request
                    .model_candidates
                    .iter()
                    .filter(|candidate| candidate.name.trim() == model.trim())
                    .collect::<Vec<_>>();
                let roles = candidates
                    .iter()
                    .map(|candidate| match candidate.role {
                        ModelRole::Planner => "planner",
                        ModelRole::Executor => "executor",
                        ModelRole::Reviewer => "reviewer",
                        ModelRole::Summarizer => "summarizer",
                        ModelRole::Embedder => "embedder",
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(",");
                let supports_tools = candidates.iter().any(|candidate| candidate.supports_tools);
                let supports_vision = candidates.iter().any(|candidate| candidate.supports_vision);
                format!(
                    "- {model} | configured_roles={} | tools={} | vision={}",
                    if roles.is_empty() {
                        "unassigned"
                    } else {
                        &roles
                    },
                    supports_tools,
                    supports_vision,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let context = if request.recent_context.trim().is_empty() {
            "(none)"
        } else {
            request.recent_context.as_str()
        };
        let historical_evidence = if request.historical_evidence.trim().is_empty() {
            "(no sufficiently supported observations yet)"
        } else {
            request.historical_evidence.as_str()
        };
        let execution_constraints = if request.execution_constraints.trim().is_empty() {
            "(none)"
        } else {
            request.execution_constraints.as_str()
        };
        let required_execution = match request.required_execution {
            Some(AgentExecutionMode::Direct) => {
                "Matched causal treatment: execution must be direct and workflow_plan must be null."
            }
            Some(AgentExecutionMode::Workflow) => {
                "Matched causal treatment: execution must be workflow and workflow_plan must contain the best bounded graph for this request."
            }
            None => "No evaluation treatment is active; choose Direct or Workflow autonomously.",
        };
        let read_only_parallel_guidance = if request
            .route_requirements
            .parallel_read_only_exploration_authorized()
        {
            "This request forbids all effects, so the whole run is read-only. For this read-only request class one bounded widening is permitted: max_parallelism=2, min_successful_branches=1, distinct_contributions=2, and estimated_steps=3 without a verifier or estimated_steps=4 with a model-distinct independent verifier. workflow_plan then carries two dependency-free Specialist roots with genuinely distinct bounded subtasks, the optional Verifier depending on both roots, and the final tool-free synthesis compatibility node depending on the Verifier or on both Specialist roots. Both specialists still use read-only tool policy only. Choose this shape only when two distinct read-only investigations plausibly beat both the single-Specialist graph and the direct baseline; otherwise keep the single-Specialist graph or choose direct.\n"
        } else {
            ""
        };
        let preferred_model_guidance = match request.preferred_primary_model.as_deref() {
            Some(model) if !model.trim().is_empty() => {
                format!(
                    "Preferred primary model for this effort tier: {model}. Use it for primary_model unless the task evidence clearly favors another configured model.\n"
                )
            }
            _ => String::new(),
        };
        format!(
            concat!(
                "You are the Cindx runtime Conductor. Decide how to execute the request; do not answer it. Return only one strict JSON object.\n",
                "Treat the strongest configured single-model direct answer as the baseline. Choose workflow only when independent work, verification, or decomposition is likely to improve correctness enough to justify coordination latency and correlated-error risk. Pro prioritizes correctness but is not automatically multi-model. Auto balances correctness and latency.\n",
                "Choose retrieval from semantic, file_search, graph_direct, graph_walk only when the answer needs workspace evidence not already present. Choose memory only when prior user/project decisions are materially relevant. Memory and workspace retrieval are blocking foreground work: select them only when missing evidence can materially change answer quality. Greetings, capability questions, and self-contained requests should use neither. Do not retrieve merely because the prompt is long, mentions code, or asks a question.\n",
                "Graph walk must have semantic, file_search, or graph_direct as a seed channel. Keep focused retrieval and memory queries under {query_limit} characters. Use only exact configured model strings.\n",
                "Configured roles are capability boundaries: primary_model and Specialist work must use a planner or executor role; an Independent Verifier must use a reviewer role. A summarizer role is utility-only and cannot act in the execution graph. The final synthesis compatibility node is a deterministic non-model Owner handoff, so its model field is not dispatched.\n",
                "For direct execution use max_parallelism=1, min_successful_branches=1, distinct_contributions=0, stop_policy=first_verified, and verification none or self_check. A workflow uses max_parallelism=1, min_successful_branches=1, and distinct_contributions=1: one bounded Specialist, optionally followed by one Independent Verifier using a different configured model. Without a verifier use verification=none, estimated_steps=2, and stop_policy=exhaustive. With a genuinely model-distinct verifier use verification=independent and estimated_steps=3; if no second suitable configured model exists, do not manufacture independence.\n",
                "A workflow decision is valid only when you can name that executable graph now. Include workflow_plan with one Specialist root, the optional Verifier depending only on that root, and a final tool-free synthesis compatibility node depending on the Specialist or Verifier. The runtime materializes that final node as a deterministic handoff to the foreground Owner; it is not another model actor. Use output_kind=analysis|evidence for the Specialist, verification for the optional Verifier, synthesis for the final handoff, and tool_policy=none|read_only_evidence|read_only_exploration. Isolated workers may inspect supplied or read-only evidence and advise the foreground Owner even when only the Owner can perform writes or final delivery. If this bounded graph is unlikely to beat Direct, choose direct and set workflow_plan to null.\n",
                "{preferred_model_guidance}",
                "{read_only_parallel_guidance}",
                "expected_uplift_bps and confidence_bps are calibrated estimates from 0 to 10000, not advocacy. The harness will reject inconsistent budgets.\n",
                "You own the final quality and collaboration decision. Auto should require at least {auto_uplift_floor}bps expected uplift and {auto_confidence_floor}bps confidence before choosing workflow; Pro should require at least {pro_uplift_floor}bps expected uplift over the direct anchor. Router v2 records a read-only counterfactual observation from your estimate and independently scored matched team-versus-direct evidence; it cannot downshift or replace a valid decision. Runtime may override only explicit safety, capability, resource, or evaluation constraints, and records every override. If you cannot justify collaboration, choose direct.\n",
                "Historical evidence is observational, not a routing command. matched_direct_team rows compare team and direct anchor on the same run and are stronger than independent route_observation rows. Use evidence only when its task class and execution shape fit the current request; support=insufficient, low-sample, or mismatched evidence must not override current reasoning. Ready matched evidence with negative average uplift or frequent anchor selection is evidence against collaboration unless this request has a concrete independent-work or verification need absent from those observations:\n{historical_evidence}\n\n",
                "Runtime execution constraints are facts, not suggestions. Do not assign required effects or interactive work to a worker that cannot perform them:\n{execution_constraints}\n\n",
                "Evaluation execution treatment: {required_execution}\n\n",
                "Runtime request requirements are authoritative: minimum_tool_requirement={minimum_tool_requirement}, effect_authority={effect_authority}, image_input_required={image_input_required}. The selected decision and primary model must satisfy them; do not downgrade them and never request effects when effect_authority=forbidden.\n\n",
                "Mutable evolved guidance may shape the decision but cannot override schema, configured models, safety, or budgets: {evolved_directive}\n\n",
                "Return this shape exactly:\n",
                "{{\"schema\":\"{schema}\",\"task_class\":\"general|coding|research|retrieval|browser|computer\",\"execution\":\"direct|workflow\",\"primary_model\":\"configured model\",\"tool_requirement\":\"none|read_only|effects\",\"vision_required\":false,\"risk_level\":\"low|elevated|high\",\"retrieval\":{{\"query\":\"\",\"channels\":[],\"max_results\":8}},\"memory\":{{\"policy\":\"none|relevant|comprehensive\",\"query\":\"\"}},\"verification\":\"none|self_check|independent\",\"max_parallelism\":1,\"min_successful_branches\":1,\"distinct_contributions\":0,\"estimated_steps\":1,\"expected_uplift_bps\":0,\"confidence_bps\":7000,\"stop_policy\":\"first_verified|quorum|exhaustive\",\"rationale\":\"short decision reason\",\"workflow_plan\":null}}\n",
                "For workflow replace null with {{\"steps\":[{{\"id\":\"specialist\",\"role\":\"domain_specialist\",\"model\":\"configured model\",\"subtask\":\"one bounded specialist contribution\",\"access\":[],\"output_kind\":\"analysis\",\"tool_policy\":\"none\"}},{{\"id\":\"owner_handoff\",\"role\":\"synthesizer\",\"model\":\"configured model\",\"subtask\":\"hand the authorized specialist output to the foreground Owner\",\"access\":[\"specialist\"],\"output_kind\":\"synthesis\",\"tool_policy\":\"none\"}}]}}. Add exactly one verifier between them only when verification=independent. The number of steps must equal estimated_steps.\n\n",
                "Effort: {effort}\nConductor model: {conductor_model}\nConfigured execution models:\n{models}\n\nUser request:\n{objective}\n\nRecent session context:\n{context}"
            ),
            query_limit = MAX_RUN_DECISION_QUERY_CHARS,
            auto_uplift_floor = AUTO_COLLABORATION_MIN_UPLIFT_BPS,
            auto_confidence_floor = AUTO_COLLABORATION_MIN_CONFIDENCE_BPS,
            pro_uplift_floor = minimum_team_uplift_bps("pro"),
            evolved_directive = if request.evolved_directive.trim().is_empty() {
                "(none)"
            } else {
                request.evolved_directive.as_str()
            },
            historical_evidence = historical_evidence,
            execution_constraints = execution_constraints,
            required_execution = required_execution,
            read_only_parallel_guidance = read_only_parallel_guidance,
            preferred_model_guidance = preferred_model_guidance,
            minimum_tool_requirement = request.route_requirements.minimum_tool_requirement.label(),
            effect_authority = request.route_requirements.effect_authority.label(),
            image_input_required = request.route_requirements.image_input_required,
            schema = AGENT_RUN_DECISION_SCHEMA,
            effort = request.effort,
            conductor_model = request.conductor_model,
            models = models,
            objective = request.objective,
            context = context,
        )
    }

    pub fn repair_prompt(&self, rejected: &str, error: &str) -> String {
        format!(
            "Repair the rejected Cindx run decision. Correct only schema, configured model, enum, query, or budget consistency. Return strict JSON only.\n\nValidation error:\n{}\n\nRejected decision:\n{}\n\nOriginal request:\n{}",
            error,
            bounded_chars(rejected, 6_000),
            self.planning_prompt(),
        )
    }

    pub fn parse(&self, response: &str) -> Result<AgentRunDecision, String> {
        self.parse_draft(response)
            .map(AgentRunDecisionDraft::into_legacy_selected)
    }

    pub fn parse_draft(&self, response: &str) -> Result<AgentRunDecisionDraft, String> {
        let start = response
            .find('{')
            .ok_or_else(|| "run decision did not return a JSON object".to_string())?;
        let end = response
            .rfind('}')
            .filter(|end| *end >= start)
            .ok_or_else(|| "run decision returned incomplete JSON".to_string())?;
        let payload = serde_json::from_str::<serde_json::Value>(&response[start..=end])
            .map_err(|error| format!("run decision JSON is invalid: {error}"))?;
        let mut decision = serde_json::from_value::<AgentRunDecision>(payload.clone())
            .map_err(|error| format!("run decision JSON is invalid: {error}"))?;
        let parallel_read_only_authorized = self
            .request
            .route_requirements
            .parallel_read_only_exploration_authorized();
        decision.validate(
            &self.request.allowed_models,
            self.request.max_parallelism,
            parallel_read_only_authorized,
        )?;
        if self
            .request
            .required_execution
            .is_some_and(|required| decision.execution != required)
        {
            return Err("run decision violates the matched execution treatment".to_string());
        }
        self.request
            .route_requirements
            .validate_decision(&decision, &self.request.model_candidates)?;
        validate_primary_model_profile(&decision, &self.request.model_candidates)?;
        let workflow_plan = payload
            .get("workflow_plan")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value::<WorkflowPlanProposal>)
            .transpose()
            .map_err(|error| format!("workflow proposal JSON is invalid: {error}"))?;
        match decision.execution {
            AgentExecutionMode::Direct if workflow_plan.is_some() => {
                return Err("direct execution must set workflow_plan to null".to_string())
            }
            AgentExecutionMode::Workflow => {
                let workflow_plan = workflow_plan
                    .as_ref()
                    .ok_or_else(|| "workflow execution requires workflow_plan".to_string())?;
                workflow_plan.validate_owner_execution_graph(
                    &decision,
                    &self.request.allowed_models,
                    parallel_read_only_authorized,
                )?;
                validate_workflow_model_profiles(workflow_plan, &self.request.model_candidates)?;
            }
            AgentExecutionMode::Direct => {}
        }
        let snapshot = RouteFeatureSnapshotV2::from_decision_request(
            &decision,
            RouteFeatureRequest {
                objective: &self.request.objective,
                recent_context: &self.request.recent_context,
                effort: &self.request.effort,
                requirements: self.request.route_requirements,
                budget_fingerprint: self.request.budget_fingerprint.as_deref(),
                prompt_profile_sha256: &self.request.prompt_profile_sha256,
            },
            &self.request.model_candidates,
        );
        let normalized_effort = self.request.effort.trim().to_ascii_lowercase();
        let candidate_action_id = causal_route_action_id_v2(&decision)?;
        let (matched, evidence_key_lookups) = matching_collaboration_evidence_for_context(
            &decision,
            &normalized_effort,
            &snapshot.context_fingerprint,
            &candidate_action_id,
            &self.request.matched_collaboration_evidence,
        );
        let receipt = select_causal_route_v2(
            &decision,
            &snapshot,
            &self.request.model_candidates,
            matched,
            evidence_key_lookups,
        )?;
        let compatibility_assessment = (normalized_effort == "auto"
            && decision.execution == AgentExecutionMode::Workflow)
            .then(|| AutoComputationAssessment::from_causal_route(&receipt));
        if let Some(assessment) = compatibility_assessment {
            decision.computation_value = Some(assessment);
        }
        Ok(AgentRunDecisionDraft {
            conductor_candidate: decision,
            compatibility_route: receipt,
            workflow_plan,
        })
    }
}

fn model_has_profile(
    model: &str,
    candidates: &[ModelCandidate],
    eligible: impl Fn(&ModelRole) -> bool,
) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.name.trim() == model.trim() && eligible(&candidate.role))
}

fn execution_profile(role: &ModelRole) -> bool {
    matches!(role, ModelRole::Planner | ModelRole::Executor)
}

pub fn validate_primary_model_profile(
    decision: &AgentRunDecision,
    candidates: &[ModelCandidate],
) -> Result<(), String> {
    if model_has_profile(&decision.primary_model, candidates, execution_profile) {
        Ok(())
    } else {
        Err(
            "run decision primary_model must use a configured Primary or Reasoning profile"
                .to_string(),
        )
    }
}

pub fn validate_workflow_model_profiles(
    workflow_plan: &WorkflowPlanProposal,
    candidates: &[ModelCandidate],
) -> Result<(), String> {
    for step in &workflow_plan.steps {
        validate_workflow_step_model_profile(&step.id, &step.model, &step.output_kind, candidates)?;
    }
    Ok(())
}

pub fn validate_workflow_step_model_profile(
    step_id: &str,
    model: &str,
    output_kind: &WorkflowOutputKind,
    candidates: &[ModelCandidate],
) -> Result<(), String> {
    if *output_kind == WorkflowOutputKind::Synthesis {
        return Ok(());
    }
    let valid = match output_kind {
        WorkflowOutputKind::Verification => model_has_profile(model, candidates, |role| {
            matches!(role, ModelRole::Reviewer)
        }),
        WorkflowOutputKind::Analysis | WorkflowOutputKind::Evidence => {
            model_has_profile(model, candidates, execution_profile)
        }
        WorkflowOutputKind::Synthesis => unreachable!("synthesis handled above"),
    };
    if valid {
        return Ok(());
    }
    let required_profile = match output_kind {
        WorkflowOutputKind::Verification => "Verifier",
        WorkflowOutputKind::Analysis | WorkflowOutputKind::Evidence => "Primary or Reasoning",
        WorkflowOutputKind::Synthesis => unreachable!("synthesis handled above"),
    };
    Err(format!(
        "workflow step {step_id} model must use a configured {required_profile} profile"
    ))
}

fn bounded_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let bounded = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{bounded}\n[truncated]")
    } else {
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentRouteTier, AutoComputationVerdict, CausalRouteReason, MatchedCollaborationEvidence,
    };

    fn request() -> AgentRunDecisionRequest {
        AgentRunDecisionRequest {
            objective: "Compare two implementation strategies".to_string(),
            recent_context: String::new(),
            effort: "auto".to_string(),
            conductor_model: "planner".to_string(),
            allowed_models: vec!["executor".to_string(), "reviewer".to_string()],
            model_candidates: ["executor", "reviewer"]
                .into_iter()
                .map(|name| ModelCandidate {
                    name: name.to_string(),
                    role: if name == "reviewer" {
                        ModelRole::Reviewer
                    } else {
                        ModelRole::Executor
                    },
                    supports_tools: true,
                    supports_vision: true,
                    tools_capability_source: crate::ModelCapabilitySource::Configured,
                    vision_capability_source: crate::ModelCapabilitySource::Configured,
                    cost_tier: 1,
                    latency_tier: 1,
                })
                .collect(),
            max_parallelism: 3,
            evolved_directive: String::new(),
            historical_evidence: String::new(),
            matched_collaboration_evidence: Arc::new(Default::default()),
            execution_constraints: "isolated workers are read-only".to_string(),
            route_requirements: AgentRouteRequirements::default(),
            budget_fingerprint: Some("0".repeat(64)),
            prompt_profile_sha256: "1".repeat(64),
            preferred_primary_model: None,
            required_execution: None,
        }
    }

    fn workflow_payload(decision: &AgentRunDecision, steps: serde_json::Value) -> String {
        let mut payload = serde_json::to_value(decision).expect("decision JSON");
        payload.as_object_mut().expect("decision object").insert(
            "workflow_plan".to_string(),
            serde_json::json!({ "steps": steps }),
        );
        serde_json::to_string(&payload).expect("workflow payload")
    }

    #[test]
    fn parses_a_consistent_dynamic_workflow_decision() {
        let harness = AgentRunDecisionHarness::new(request());
        let decision = harness
            .parse(
                r#"{
                    "schema":"cindx.agent-run-decision.v1",
                    "task_class":"research",
                    "execution":"workflow",
                    "primary_model":"executor",
                    "tool_requirement":"read_only",
                    "vision_required":false,
                    "risk_level":"elevated",
                    "retrieval":{"query":"implementation evidence","channels":["semantic","file_search"],"max_results":6},
                    "memory":{"policy":"relevant","query":"prior architecture constraints"},
                    "verification":"independent",
                    "max_parallelism":1,
                    "min_successful_branches":1,
                    "distinct_contributions":1,
                    "estimated_steps":3,
                    "expected_uplift_bps":6000,
                    "confidence_bps":8000,
                    "stop_policy":"exhaustive",
                    "rationale":"specialist architecture analysis with independent verification",
                    "workflow_plan":{"steps":[
                        {"id":"specialist","role":"architecture_specialist","model":"executor","subtask":"analyze the architecture and implementation tradeoffs","access":[],"output_kind":"analysis","tool_policy":"read_only_evidence"},
                        {"id":"verify","role":"independent_verifier","model":"reviewer","subtask":"audit the specialist contribution","access":["specialist"],"output_kind":"verification","tool_policy":"none"},
                        {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the verified result to the foreground Owner","access":["verify"],"output_kind":"synthesis","tool_policy":"none"}
                    ]}
                }"#,
            )
            .unwrap();
        assert_eq!(
            decision.policy(),
            OrchestrationPolicy::BestOfN { candidates: 1 }
        );
        assert_eq!(decision.retrieval.max_results, 6);
        assert_eq!(
            decision
                .execution_contract("auto")
                .min_distinct_contributions,
            1
        );
    }

    fn forbidden_route_request() -> AgentRunDecisionRequest {
        let mut request = request();
        request.route_requirements = AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::ReadOnly,
            effect_authority: AgentEffectAuthority::Forbidden,
            image_input_required: false,
        };
        request
    }

    const PARALLEL_READ_ONLY_DECISION: &str = r#"{
        "schema":"cindx.agent-run-decision.v1",
        "task_class":"research",
        "execution":"workflow",
        "primary_model":"executor",
        "tool_requirement":"read_only",
        "vision_required":false,
        "risk_level":"low",
        "retrieval":{"query":"read-only survey evidence","channels":["file_search"],"max_results":6},
        "memory":{"policy":"none","query":""},
        "verification":"independent",
        "max_parallelism":2,
        "min_successful_branches":1,
        "distinct_contributions":2,
        "estimated_steps":4,
        "expected_uplift_bps":6000,
        "confidence_bps":8000,
        "stop_policy":"exhaustive",
        "rationale":"two distinct read-only investigations under a forbidden effect authority",
        "workflow_plan":{"steps":[
            {"id":"specialist","role":"survey_specialist","model":"executor","subtask":"survey the public module surface","access":[],"output_kind":"analysis","tool_policy":"read_only_exploration"},
            {"id":"specialist_two","role":"survey_specialist","model":"executor","subtask":"survey the internal test surface","access":[],"output_kind":"analysis","tool_policy":"read_only_exploration"},
            {"id":"verify","role":"independent_verifier","model":"reviewer","subtask":"audit both survey branches","access":["specialist","specialist_two"],"output_kind":"verification","tool_policy":"none"},
            {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the verified survey to the foreground Owner","access":["verify"],"output_kind":"synthesis","tool_policy":"none"}
        ]}
    }"#;

    #[test]
    fn forbidden_effect_authority_allows_a_two_specialist_read_only_workflow() {
        let harness = AgentRunDecisionHarness::new(forbidden_route_request());
        let decision = harness.parse(PARALLEL_READ_ONLY_DECISION).unwrap();
        assert_eq!(
            decision.policy(),
            OrchestrationPolicy::BestOfN { candidates: 2 }
        );
        assert_eq!(decision.max_parallelism, 2);
        assert_eq!(decision.distinct_contributions, 2);
        let parallel = harness
            .parse_draft(PARALLEL_READ_ONLY_DECISION)
            .expect("parallel draft parses");
        let plan = parallel.workflow_plan.expect("dual proposal is retained");
        assert_eq!(plan.steps.len(), 4);
    }

    #[test]
    fn allowed_effect_authority_rejects_the_two_specialist_read_only_workflow() {
        let harness = AgentRunDecisionHarness::new(request());
        let error = harness
            .parse(PARALLEL_READ_ONLY_DECISION)
            .expect_err("dual specialists require a forbidden effect authority");
        assert!(error.contains("forbidden effect authority"), "{error}");

        let forbidden = AgentRunDecisionHarness::new(forbidden_route_request());
        let mut payload: serde_json::Value =
            serde_json::from_str(PARALLEL_READ_ONLY_DECISION).unwrap();
        payload["tool_requirement"] = serde_json::json!("effects");
        payload["workflow_plan"] = serde_json::json!(null);
        let error = forbidden
            .parse(&payload.to_string())
            .expect_err("effects exceed the forbidden authority");
        assert!(error.contains("effect authority"), "{error}");
    }

    #[test]
    fn dual_read_only_decision_budgets_are_enforced_with_the_authorization() {
        let allowed_models = vec!["executor".to_string(), "reviewer".to_string()];
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.tool_requirement = AgentToolRequirement::ReadOnly;
        decision.verification = AgentVerificationPolicy::Independent;
        decision.max_parallelism = 2;
        decision.min_successful_branches = 1;
        decision.distinct_contributions = 2;
        decision.estimated_steps = 4;
        decision.stop_policy = ConductorStopPolicy::Exhaustive;
        decision
            .validate(&allowed_models, 3, true)
            .expect("authorized dual shape is valid");

        let mut underdeclared = decision.clone();
        underdeclared.distinct_contributions = 1;
        assert!(underdeclared.validate(&allowed_models, 3, true).is_err());

        let mut wrong_steps = decision.clone();
        wrong_steps.estimated_steps = 3;
        assert!(wrong_steps.validate(&allowed_models, 3, true).is_err());

        assert!(decision.validate(&allowed_models, 3, false).is_err());

        let mut triple = decision.clone();
        triple.max_parallelism = 3;
        triple.distinct_contributions = 3;
        assert!(triple.validate(&allowed_models, 3, true).is_err());
    }

    #[test]
    fn planning_prompt_offers_the_dual_read_only_shape_only_under_forbidden_authority() {
        let authorized = AgentRunDecisionHarness::new(forbidden_route_request());
        let prompt = authorized.planning_prompt();
        assert!(prompt.contains("max_parallelism=2"));
        assert!(prompt.contains("two dependency-free Specialist roots"));

        let baseline = AgentRunDecisionHarness::new(request());
        let prompt = baseline.planning_prompt();
        assert!(!prompt.contains("two dependency-free Specialist roots"));
    }

    #[test]
    fn verifier_only_profile_cannot_be_selected_as_primary() {
        let harness = AgentRunDecisionHarness::new(request());
        let payload =
            serde_json::to_string(&AgentRunDecision::direct("reviewer")).expect("direct decision");

        let error = harness
            .parse_draft(&payload)
            .expect_err("a Verifier-only model cannot own direct execution");

        assert!(error.contains("Primary or Reasoning profile"));
    }

    #[test]
    fn utility_only_profile_cannot_be_selected_as_primary() {
        let mut request = request();
        request.allowed_models.push("utility".to_string());
        request.model_candidates.push(ModelCandidate {
            name: "utility".to_string(),
            role: ModelRole::Summarizer,
            supports_tools: true,
            supports_vision: true,
            tools_capability_source: crate::ModelCapabilitySource::Configured,
            vision_capability_source: crate::ModelCapabilitySource::Configured,
            cost_tier: 1,
            latency_tier: 1,
        });
        let harness = AgentRunDecisionHarness::new(request);
        let payload =
            serde_json::to_string(&AgentRunDecision::direct("utility")).expect("direct decision");

        let error = harness
            .parse_draft(&payload)
            .expect_err("a Utility-only model cannot own direct execution");

        assert!(error.contains("Primary or Reasoning profile"));
    }

    #[test]
    fn verifier_only_profile_cannot_be_used_as_a_specialist() {
        let harness = AgentRunDecisionHarness::new(request());
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::None;
        decision.max_parallelism = 1;
        decision.min_successful_branches = 1;
        decision.distinct_contributions = 1;
        decision.estimated_steps = 2;
        decision.stop_policy = ConductorStopPolicy::Exhaustive;
        let payload = workflow_payload(
            &decision,
            serde_json::json!([
                {
                    "id":"specialist",
                    "role":"domain_specialist",
                    "model":"reviewer",
                    "subtask":"derive the solution",
                    "access":[],
                    "output_kind":"analysis",
                    "tool_policy":"none"
                },
                {
                    "id":"owner_handoff",
                    "role":"synthesizer",
                    "model":"executor",
                    "subtask":"hand the result to the Owner",
                    "access":["specialist"],
                    "output_kind":"synthesis",
                    "tool_policy":"none"
                }
            ]),
        );

        let error = harness
            .parse_draft(&payload)
            .expect_err("a Verifier-only model cannot act as Specialist");

        assert!(error.contains("workflow step specialist"), "{error}");
        assert!(error.contains("Primary or Reasoning"), "{error}");
    }

    #[test]
    fn shared_concrete_model_keeps_each_independently_configured_profile() {
        let mut request = request();
        request.model_candidates.push(ModelCandidate {
            name: "reviewer".to_string(),
            role: ModelRole::Executor,
            supports_tools: true,
            supports_vision: true,
            tools_capability_source: crate::ModelCapabilitySource::Configured,
            vision_capability_source: crate::ModelCapabilitySource::Configured,
            cost_tier: 1,
            latency_tier: 1,
        });
        let harness = AgentRunDecisionHarness::new(request);
        let payload =
            serde_json::to_string(&AgentRunDecision::direct("reviewer")).expect("direct decision");

        harness
            .parse_draft(&payload)
            .expect("the Executor binding independently authorizes the shared model");
    }

    #[test]
    fn workflow_proposal_v1_receipts_remain_distinct_from_the_current_owner_graph() {
        let mut legacy = AgentRunDecision::direct("executor");
        legacy.execution = AgentExecutionMode::Workflow;
        legacy.verification = AgentVerificationPolicy::SelfCheck;
        legacy.max_parallelism = 2;
        legacy.min_successful_branches = 2;
        legacy.distinct_contributions = 2;
        legacy.estimated_steps = 3;
        legacy.stop_policy = ConductorStopPolicy::Quorum;
        let proposal = WorkflowPlanProposal {
            steps: vec![
                WorkflowPlanProposalStep {
                    id: "analysis".to_string(),
                    role: "analyst".to_string(),
                    model: "executor".to_string(),
                    subtask: "derive the primary solution".to_string(),
                    access: Vec::new(),
                    output_kind: WorkflowOutputKind::Analysis,
                    tool_policy: WorkflowToolPolicy::None,
                },
                WorkflowPlanProposalStep {
                    id: "counterexample".to_string(),
                    role: "critic".to_string(),
                    model: "executor".to_string(),
                    subtask: "search for an independent counterexample".to_string(),
                    access: Vec::new(),
                    output_kind: WorkflowOutputKind::Verification,
                    tool_policy: WorkflowToolPolicy::None,
                },
                WorkflowPlanProposalStep {
                    id: "synthesis".to_string(),
                    role: "synthesizer".to_string(),
                    model: "executor".to_string(),
                    subtask: "reconcile both contributions".to_string(),
                    access: vec!["analysis".to_string(), "counterexample".to_string()],
                    output_kind: WorkflowOutputKind::Synthesis,
                    tool_policy: WorkflowToolPolicy::None,
                },
            ],
        };
        let models = ["executor".to_string()];

        proposal
            .validate_v1(&legacy, &models)
            .expect("persisted v1 proposal topology must remain readable");
        assert!(proposal
            .validate_owner_execution_graph(&legacy, &models, false)
            .expect_err("legacy competition must not re-enter the current production graph")
            .contains("one specialist"));
    }

    #[test]
    fn matched_execution_treatment_is_explicit_and_fails_closed() {
        let mut constrained = request();
        constrained.required_execution = Some(AgentExecutionMode::Workflow);
        let harness = AgentRunDecisionHarness::new(constrained);
        assert!(harness
            .planning_prompt()
            .contains("execution must be workflow"));

        let direct = serde_json::to_string(&AgentRunDecision::direct("executor"))
            .expect("direct decision should serialize");
        let error = harness
            .parse(&direct)
            .expect_err("a Direct response must not enter the Workflow treatment arm");

        assert!(error.contains("matched execution treatment"));
    }

    #[test]
    fn decision_draft_keeps_compatibility_value_as_a_shadow_observation() {
        let mut candidate = AgentRunDecision::direct("executor");
        candidate.execution = AgentExecutionMode::Workflow;
        candidate.verification = AgentVerificationPolicy::Independent;
        candidate.max_parallelism = 1;
        candidate.min_successful_branches = 1;
        candidate.distinct_contributions = 1;
        candidate.estimated_steps = 3;
        candidate.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS - 1;
        candidate.confidence_bps = AUTO_COLLABORATION_MIN_CONFIDENCE_BPS;
        candidate.stop_policy = ConductorStopPolicy::Exhaustive;
        candidate.rationale = "specialist contribution with independent verification".to_string();

        let payload = workflow_payload(
            &candidate,
            serde_json::json!([
                {"id":"specialist","role":"implementation_specialist","model":"executor","subtask":"derive the architecture and implementation path","access":[],"output_kind":"analysis","tool_policy":"none"},
                {"id":"verify","role":"independent_verifier","model":"reviewer","subtask":"audit the specialist path","access":["specialist"],"output_kind":"verification","tool_policy":"none"},
                {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the audited path to the foreground Owner","access":["verify"],"output_kind":"synthesis","tool_policy":"none"}
            ]),
        );
        let draft = AgentRunDecisionHarness::new(request())
            .parse_draft(&payload)
            .expect("low-value workflow should produce an auditable compatibility draft");

        assert_eq!(
            draft.conductor_candidate.execution,
            AgentExecutionMode::Workflow
        );
        assert_eq!(
            draft.compatibility_route.reason,
            CausalRouteReason::BelowPredictionFloor
        );
        assert_eq!(
            draft.compatibility_route.selected_route,
            AgentRouteTier::Direct
        );
        assert_eq!(
            draft.workflow_plan.as_ref().map(|plan| plan.steps.len()),
            Some(3)
        );
        assert!(draft.conductor_candidate.causal_route.is_none());
    }

    #[test]
    fn one_capable_model_runs_one_specialist_without_a_pseudo_verifier() {
        let mut request = request();
        request.allowed_models = vec!["executor".to_string()];
        let harness = AgentRunDecisionHarness::new(request);
        let mut candidate = AgentRunDecision::direct("executor");
        candidate.task_class = TaskClass::Research;
        candidate.execution = AgentExecutionMode::Workflow;
        candidate.verification = AgentVerificationPolicy::None;
        candidate.max_parallelism = 1;
        candidate.min_successful_branches = 1;
        candidate.distinct_contributions = 1;
        candidate.estimated_steps = 2;
        candidate.expected_uplift_bps = 5_000;
        candidate.confidence_bps = 8_000;
        candidate.stop_policy = ConductorStopPolicy::Exhaustive;
        candidate.rationale = "one bounded specialist contribution".to_string();
        let payload = workflow_payload(
            &candidate,
            serde_json::json!([
                {"id":"specialist","role":"domain_specialist","model":"executor","subtask":"derive the primary solution path","access":[],"output_kind":"analysis","tool_policy":"none"},
                {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the specialist path to the foreground Owner","access":["specialist"],"output_kind":"synthesis","tool_policy":"none"}
            ]),
        );
        let decision = harness
            .parse(&payload)
            .expect("one model should still support one bounded specialist");

        assert_eq!(decision.distinct_contributions, 1);
        assert_eq!(decision.verification, AgentVerificationPolicy::None);
    }

    #[test]
    fn planning_prompt_labels_historical_evidence_as_observational() {
        let mut request = request();
        request.historical_evidence =
            "coding direct executor: 8/10 verified, median latency 1200 ms".to_string();
        let prompt = AgentRunDecisionHarness::new(request).planning_prompt();

        assert!(prompt.contains("Historical evidence is observational, not a routing command"));
        assert!(prompt.contains("matched_direct_team rows compare team and direct anchor"));
        assert!(prompt.contains("8/10 verified"));
        assert!(prompt.contains("low-sample"));
        assert!(prompt.contains("must not override current reasoning"));
    }

    #[test]
    fn planning_prompt_treats_memory_and_retrieval_as_blocking_work() {
        let prompt = AgentRunDecisionHarness::new(request()).planning_prompt();

        assert!(prompt.contains("Memory and workspace retrieval are blocking foreground work"));
        assert!(prompt.contains("missing evidence can materially change answer quality"));
        assert!(prompt.contains("Greetings, capability questions, and self-contained requests"));
        assert!(prompt.contains("read-only counterfactual observation"));
        assert!(prompt.contains("cannot downshift or replace a valid decision"));
    }

    #[test]
    fn planning_prompt_exposes_worker_execution_constraints() {
        let prompt = AgentRunDecisionHarness::new(request()).planning_prompt();

        assert!(prompt.contains("Runtime execution constraints are facts"));
        assert!(prompt.contains("isolated workers are read-only"));
    }

    #[test]
    fn planning_prompt_exposes_authoritative_runtime_requirements() {
        let mut request = request();
        request.route_requirements = AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::Effects,
            effect_authority: AgentEffectAuthority::Required,
            image_input_required: true,
        };

        let prompt = AgentRunDecisionHarness::new(request).planning_prompt();

        assert!(prompt.contains("minimum_tool_requirement=effects"));
        assert!(prompt.contains("image_input_required=true"));
        assert!(prompt.contains("must satisfy them"));
    }

    #[test]
    fn planning_prompt_anchors_configured_effort_model_only_when_pinned() {
        let prompt = AgentRunDecisionHarness::new(request()).planning_prompt();
        assert!(!prompt.contains("Preferred primary model"));

        let mut pinned = request();
        pinned.preferred_primary_model = Some("deepseek-v4-pro-0813".to_string());
        let prompt = AgentRunDecisionHarness::new(pinned).planning_prompt();
        assert!(
            prompt.contains("Preferred primary model for this effort tier: deepseek-v4-pro-0813")
        );

        let mut blank = request();
        blank.preferred_primary_model = Some("   ".to_string());
        let prompt = AgentRunDecisionHarness::new(blank).planning_prompt();
        assert!(!prompt.contains("Preferred primary model"));
    }

    #[test]
    fn runtime_requirements_reject_tool_and_vision_downgrades() {
        let mut request = request();
        request.route_requirements = AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::Effects,
            effect_authority: AgentEffectAuthority::Required,
            image_input_required: true,
        };
        let harness = AgentRunDecisionHarness::new(request);
        let decision = serde_json::to_string(&AgentRunDecision::direct("executor")).unwrap();

        let error = harness.parse(&decision).unwrap_err();

        assert!(error.contains("omitted required effect authority"));
    }

    #[test]
    fn runtime_effect_authority_rejects_an_unrequested_side_effect_upgrade() {
        let mut request = request();
        request.route_requirements = AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::ReadOnly,
            effect_authority: AgentEffectAuthority::Forbidden,
            image_input_required: false,
        };
        let mut decision = AgentRunDecision::direct("executor");
        decision.tool_requirement = AgentToolRequirement::Effects;

        let error = AgentRunDecisionHarness::new(request)
            .parse(&serde_json::to_string(&decision).unwrap())
            .unwrap_err();

        assert!(error.contains("exceeds the prompt effect authority"));
    }

    #[test]
    fn ambiguous_effect_authority_preserves_permission_gated_effects() {
        let mut request = request();
        request.route_requirements = AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::ReadOnly,
            effect_authority: AgentEffectAuthority::Allowed,
            image_input_required: false,
        };
        let mut decision = AgentRunDecision::direct("executor");
        decision.tool_requirement = AgentToolRequirement::Effects;

        let parsed = AgentRunDecisionHarness::new(request)
            .parse(&serde_json::to_string(&decision).unwrap())
            .expect("ambiguous intent should preserve the existing permission-gated effect path");

        assert_eq!(parsed.tool_requirement, AgentToolRequirement::Effects);
    }

    #[test]
    fn active_image_input_cannot_be_omitted_or_sent_to_a_text_only_model() {
        let mut omitted_request = request();
        omitted_request.route_requirements.image_input_required = true;
        let omitted = AgentRunDecisionHarness::new(omitted_request)
            .parse(&serde_json::to_string(&AgentRunDecision::direct("executor")).unwrap())
            .unwrap_err();
        assert!(omitted.contains("omitted vision"));

        let mut incapable_request = request();
        incapable_request.route_requirements.image_input_required = true;
        for candidate in &mut incapable_request.model_candidates {
            if candidate.name == "executor" {
                candidate.supports_vision = false;
            }
        }
        let decision = incapable_request
            .route_requirements
            .apply_to_direct(AgentRunDecision::direct("executor"));
        let incapable = AgentRunDecisionHarness::new(incapable_request)
            .parse(&serde_json::to_string(&decision).unwrap())
            .unwrap_err();
        assert!(incapable.contains("not configured with vision capability"));
    }

    #[test]
    fn selected_model_must_declare_every_requested_capability() {
        let mut request = request();
        request.route_requirements.minimum_tool_requirement = AgentToolRequirement::ReadOnly;
        for candidate in &mut request.model_candidates {
            if candidate.name == "executor" {
                candidate.supports_tools = false;
            }
        }
        let harness = AgentRunDecisionHarness::new(request);
        let decision = AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::ReadOnly,
            effect_authority: AgentEffectAuthority::Forbidden,
            image_input_required: false,
        }
        .apply_to_direct(AgentRunDecision::direct("executor"));

        let error = harness
            .parse(&serde_json::to_string(&decision).unwrap())
            .unwrap_err();

        assert!(error.contains("not configured with tool capability"));
    }

    #[test]
    fn rejects_decorative_collaboration_and_unknown_models() {
        let harness = AgentRunDecisionHarness::new(request());
        let invalid = AgentRunDecision {
            primary_model: "unknown".to_string(),
            execution: AgentExecutionMode::Workflow,
            max_parallelism: 3,
            min_successful_branches: 1,
            distinct_contributions: 0,
            stop_policy: ConductorStopPolicy::Quorum,
            ..AgentRunDecision::direct("executor")
        };
        assert!(invalid
            .validate(&harness.request.allowed_models, 3, false)
            .is_err());
    }

    #[test]
    fn direct_fallback_never_spawns_or_retrieves() {
        let decision = AgentRunDecision::direct("executor");
        decision
            .validate(&["executor".to_string()], 3, false)
            .unwrap();
        assert_eq!(decision.execution, AgentExecutionMode::Direct);
        assert!(!decision.retrieval.enabled());
        assert!(!decision.memory.enabled());
        assert_eq!(decision.distinct_contributions, 0);
    }

    #[test]
    fn dynamic_pro_contract_preserves_the_team_uplift_floor() {
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::Independent;
        decision.max_parallelism = 1;
        decision.min_successful_branches = 1;
        decision.distinct_contributions = 1;
        decision.estimated_steps = 3;
        decision.stop_policy = ConductorStopPolicy::Exhaustive;

        assert_eq!(
            decision.execution_contract("pro").min_team_uplift_bps,
            crate::PRO_MIN_TEAM_UPLIFT_BPS
        );
        assert_eq!(decision.execution_contract("auto").min_team_uplift_bps, 0);
    }

    #[test]
    fn degraded_pro_fallback_preserves_the_direct_baseline() {
        let decision = AgentRunDecision::degraded_conductor_fallback(
            "executor",
            "pro",
            3,
            3,
            "planner response was invalid",
        );

        decision
            .validate(
                &[
                    "executor".to_string(),
                    "reviewer".to_string(),
                    "planner".to_string(),
                ],
                3,
                false,
            )
            .unwrap();
        assert_eq!(decision.execution, AgentExecutionMode::Direct);
        assert_eq!(decision.distinct_contributions, 0);
        assert_eq!(decision.verification, AgentVerificationPolicy::SelfCheck);
        assert!(decision.rationale.contains("degraded direct execution"));
    }

    #[test]
    fn matched_evidence_remains_shadow_only_after_a_conductor_workflow_decision() {
        let mut auto = AgentRunDecision::direct("executor");
        auto.execution = AgentExecutionMode::Workflow;
        auto.verification = AgentVerificationPolicy::Independent;
        auto.max_parallelism = 1;
        auto.min_successful_branches = 1;
        auto.distinct_contributions = 1;
        auto.estimated_steps = 3;
        auto.expected_uplift_bps = 8_000;
        auto.confidence_bps = 9_000;
        auto.stop_policy = ConductorStopPolicy::Exhaustive;
        let payload = workflow_payload(
            &auto,
            serde_json::json!([
                {"id":"specialist","role":"domain_specialist","model":"executor","subtask":"derive the primary solution","access":[],"output_kind":"analysis","tool_policy":"none"},
                {"id":"verify","role":"independent_verifier","model":"reviewer","subtask":"audit the specialist solution","access":["specialist"],"output_kind":"verification","tool_policy":"none"},
                {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the verified result to the foreground Owner","access":["verify"],"output_kind":"synthesis","tool_policy":"none"}
            ]),
        );
        let evidence = MatchedCollaborationEvidence {
            task_class: auto.task_class.clone(),
            effort: "auto".to_string(),
            pre_decision_context_fingerprint: String::new(),
            route_action_id: String::new(),
            routing_signature: auto.learning_signature(),
            examples: 8,
            team_wins: 0,
            team_win_rate: 0.0,
            team_win_confidence: 0.0,
            below_admission_floor: 8,
            below_admission_floor_confidence: 0.67,
            anchor_selections: 8,
            average_uplift_bps: -500,
            average_team_latency_ms: 4_000,
            average_anchor_latency_ms: Some(1_000),
        };
        let mut calibrated_request = request();
        calibrated_request.matched_collaboration_evidence = Arc::new(
            MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(vec![evidence.clone()]),
        );
        let observed = AgentRunDecisionHarness::new(calibrated_request)
            .parse(&payload)
            .expect("strong matched evidence should remain an auditable shadow observation");
        assert_eq!(observed.execution, AgentExecutionMode::Workflow);
        assert_eq!(observed.primary_model, "executor");
        assert!(observed.calibration_reason.is_none());
        assert!(observed.calibration.is_none());
        assert_eq!(
            observed.causal_route.as_ref().map(|receipt| receipt.reason),
            Some(CausalRouteReason::MatchedEvidenceAgainst)
        );

        let mut unrelated = evidence.clone();
        unrelated.routing_signature = "different-task-shape".to_string();
        let mut unrelated_request = request();
        unrelated_request.matched_collaboration_evidence = Arc::new(
            MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(vec![unrelated]),
        );
        let uncalibrated = AgentRunDecisionHarness::new(unrelated_request)
            .parse(&payload)
            .unwrap();
        assert_eq!(uncalibrated.execution, AgentExecutionMode::Workflow);
        assert!(uncalibrated.calibration_reason.is_none());
        assert!(uncalibrated
            .computation_value
            .as_ref()
            .is_some_and(AutoComputationAssessment::admitted));

        let mut cross_context = evidence.clone();
        cross_context.pre_decision_context_fingerprint = "f".repeat(64);
        cross_context.route_action_id = causal_route_action_id_v2(&auto).unwrap();
        let mut cross_context_request = request();
        cross_context_request.matched_collaboration_evidence = Arc::new(
            MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(vec![cross_context]),
        );
        let cross_context_result = AgentRunDecisionHarness::new(cross_context_request)
            .parse(&payload)
            .unwrap();
        assert_eq!(cross_context_result.execution, AgentExecutionMode::Workflow);
        assert!(cross_context_result.calibration_reason.is_none());
    }

    #[test]
    fn auto_value_shadow_preserves_the_conductor_workflow_without_repair() {
        let mut request = request();
        request.objective =
            "Inspect the workspace evidence and image before applying the change".to_string();
        request.route_requirements = AgentRouteRequirements {
            minimum_tool_requirement: AgentToolRequirement::Effects,
            effect_authority: AgentEffectAuthority::Required,
            image_input_required: true,
        };
        let decision = AgentRunDecisionHarness::new(request)
            .parse(
                r#"{
                    "schema":"cindx.agent-run-decision.v1",
                    "task_class":"coding",
                    "execution":"workflow",
                    "primary_model":"executor",
                    "tool_requirement":"effects",
                    "vision_required":true,
                    "risk_level":"high",
                    "retrieval":{"query":"workspace implementation evidence","channels":["semantic","file_search"],"max_results":6},
                    "memory":{"policy":"relevant","query":"prior project constraints"},
                    "verification":"independent",
                    "max_parallelism":1,
                    "min_successful_branches":1,
                    "distinct_contributions":1,
                    "estimated_steps":3,
                    "expected_uplift_bps":2999,
                    "confidence_bps":8000,
                    "stop_policy":"exhaustive",
                    "rationale":"inspect and independently verify",
                    "workflow_plan":{"steps":[
                        {"id":"specialist","role":"evidence_specialist","model":"executor","subtask":"inspect workspace evidence and analyze the supplied image","access":[],"output_kind":"evidence","tool_policy":"read_only_evidence"},
                        {"id":"verify","role":"independent_verifier","model":"reviewer","subtask":"audit the specialist evidence","access":["specialist"],"output_kind":"verification","tool_policy":"none"},
                        {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the verified execution brief to the foreground Owner","access":["verify"],"output_kind":"synthesis","tool_policy":"none"}
                    ]}
                }"#,
            )
            .expect("low-value Auto workflow should remain conductor-owned");

        assert_eq!(decision.execution, AgentExecutionMode::Workflow);
        assert_eq!(decision.route_tier(), AgentRouteTier::Workflow);
        assert_eq!(decision.candidate_route_tier(), AgentRouteTier::Workflow);
        assert_eq!(decision.primary_model, "executor");
        assert_eq!(decision.tool_requirement, AgentToolRequirement::Effects);
        assert!(decision.vision_required);
        assert_eq!(decision.risk_level, AgentRiskLevel::High);
        assert!(decision.retrieval.enabled());
        assert!(decision.memory.enabled());
        assert!(decision.calibration.is_none());
        assert_eq!(
            decision
                .computation_value
                .as_ref()
                .map(|value| value.verdict),
            Some(AutoComputationVerdict::BelowPredictionFloor)
        );
    }

    #[test]
    fn independent_verification_must_be_the_owner_handoff_input() {
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::Independent;
        decision.max_parallelism = 1;
        decision.min_successful_branches = 1;
        decision.distinct_contributions = 1;
        decision.estimated_steps = 3;
        decision.expected_uplift_bps = 6_000;
        decision.confidence_bps = 8_000;
        decision.stop_policy = ConductorStopPolicy::Exhaustive;
        let payload = workflow_payload(
            &decision,
            serde_json::json!([
                {"id":"specialist","role":"domain_specialist","model":"executor","subtask":"derive the primary solution","access":[],"output_kind":"analysis","tool_policy":"none"},
                {"id":"verify","role":"independent_verifier","model":"reviewer","subtask":"audit the specialist solution","access":["specialist"],"output_kind":"verification","tool_policy":"none"},
                {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand both results to the foreground Owner","access":["specialist","verify"],"output_kind":"synthesis","tool_policy":"none"}
            ]),
        );

        let error = AgentRunDecisionHarness::new(request())
            .parse_draft(&payload)
            .expect_err("a disconnected verifier must fail closed");

        assert!(error
            .contains("workflow owner handoff must depend on the final specialist or verifier"));

        let same_model_payload = workflow_payload(
            &decision,
            serde_json::json!([
                {"id":"specialist","role":"domain_specialist","model":"executor","subtask":"derive the primary solution","access":[],"output_kind":"analysis","tool_policy":"none"},
                {"id":"verify","role":"independent_verifier","model":"executor","subtask":"audit the specialist solution","access":["specialist"],"output_kind":"verification","tool_policy":"none"},
                {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the verified result to the foreground Owner","access":["verify"],"output_kind":"synthesis","tool_policy":"none"}
            ]),
        );
        let error = AgentRunDecisionHarness::new(request())
            .parse_draft(&same_model_payload)
            .expect_err("the same model must not masquerade as an independent verifier");
        assert!(error.contains("different configured model"));
    }

    #[test]
    fn workflow_proposal_rejects_a_self_dependency() {
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::None;
        decision.distinct_contributions = 1;
        decision.estimated_steps = 2;
        decision.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS;
        decision.confidence_bps = AUTO_COLLABORATION_MIN_CONFIDENCE_BPS;
        decision.stop_policy = ConductorStopPolicy::Exhaustive;
        let payload = workflow_payload(
            &decision,
            serde_json::json!([
                {"id":"analysis","role":"analyst","model":"executor","subtask":"derive the primary solution","access":[],"output_kind":"analysis","tool_policy":"none"},
                {"id":"owner_handoff","role":"synthesizer","model":"executor","subtask":"hand the result to the foreground Owner","access":["owner_handoff"],"output_kind":"synthesis","tool_policy":"none"}
            ]),
        );

        let error = AgentRunDecisionHarness::new(request())
            .parse_draft(&payload)
            .expect_err("a self-dependent step must fail closed");

        assert!(error.contains("missing or later dependency"));
    }

    #[test]
    fn workflow_without_a_verifier_rejects_first_verified() {
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::None;
        decision.max_parallelism = 1;
        decision.min_successful_branches = 1;
        decision.distinct_contributions = 1;
        decision.estimated_steps = 2;
        decision.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS;
        decision.confidence_bps = AUTO_COLLABORATION_MIN_CONFIDENCE_BPS;

        let error = decision
            .validate(&["executor".to_string()], 2, false)
            .unwrap_err();

        assert!(error.contains("cannot use first_verified"));
    }
}
