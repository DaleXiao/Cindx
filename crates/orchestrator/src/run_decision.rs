use crate::{
    causal_route_action_id_v2, matching_collaboration_evidence_for_context,
    minimum_team_uplift_bps, select_causal_route_v2, AgentDecisionCalibration,
    AutoComputationAssessment, CausalRouteReason, CausalRouteSelectionV2,
    ConductorExecutionContract, ConductorFallbackPolicy, ConductorStopPolicy,
    MatchedCollaborationEvidenceTeacher, ModelCandidate, OrchestrationPolicy,
    RouteFeatureSnapshotV2, RoutingContext, RoutingDecision, TaskClass,
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
        let max_parallelism = max_parallelism.clamp(1, 3);
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
                if !(1..=max_parallelism).contains(&self.max_parallelism) {
                    return Err(format!(
                        "workflow parallelism must be between 1 and {max_parallelism}"
                    ));
                }
                if !(1..=self.max_parallelism).contains(&self.min_successful_branches) {
                    return Err("workflow quorum exceeds its parallel branch budget".to_string());
                }
                if !(1..=self.max_parallelism).contains(&self.distinct_contributions) {
                    return Err(
                        "workflow distinct contribution count is outside its branch budget"
                            .to_string(),
                    );
                }
                if self.stop_policy == ConductorStopPolicy::FirstVerified
                    && self.min_successful_branches != 1
                {
                    return Err(
                        "first_verified workflow must require exactly one successful branch"
                            .to_string(),
                    );
                }
                if self.stop_policy == ConductorStopPolicy::FirstVerified
                    && self.verification == AgentVerificationPolicy::None
                {
                    return Err(
                        "first_verified workflow requires self-check or independent verification"
                            .to_string(),
                    );
                }
                if self.verification == AgentVerificationPolicy::Independent
                    && self.estimated_steps < 2
                {
                    return Err(
                        "independent verification requires at least two workflow steps".to_string(),
                    );
                }
            }
        }
        Ok(())
    }

    pub fn validate_effort_admission(&self, effort: &str) -> Result<(), String> {
        if self.execution != AgentExecutionMode::Workflow {
            return Ok(());
        }
        let normalized_effort = effort.trim().to_ascii_lowercase();
        match normalized_effort.as_str() {
            "auto" => {
                if self.expected_uplift_bps < AUTO_COLLABORATION_MIN_UPLIFT_BPS
                    || self.confidence_bps < AUTO_COLLABORATION_MIN_CONFIDENCE_BPS
                {
                    return Err(format!(
                        "auto workflow is below its collaboration admission floor: uplift={}bps confidence={}bps",
                        self.expected_uplift_bps, self.confidence_bps
                    ));
                }
            }
            "pro" => {
                let minimum_uplift = minimum_team_uplift_bps("pro");
                if self.expected_uplift_bps < minimum_uplift {
                    return Err(format!(
                        "pro workflow is below its direct-anchor uplift floor: uplift={}bps required={}bps",
                        self.expected_uplift_bps, minimum_uplift
                    ));
                }
            }
            _ => {
                return Err("fast effort cannot admit a collaboration workflow".to_string());
            }
        }
        Ok(())
    }

    fn calibrated_to_direct(
        mut self,
        calibration: AgentDecisionCalibration,
        reason: String,
        computation_value: Option<AutoComputationAssessment>,
    ) -> Self {
        self.execution = AgentExecutionMode::Direct;
        self.verification = match self.verification {
            AgentVerificationPolicy::Independent => AgentVerificationPolicy::SelfCheck,
            verification => verification,
        };
        self.max_parallelism = 1;
        self.min_successful_branches = 1;
        self.distinct_contributions = 0;
        self.expected_uplift_bps = 0;
        self.stop_policy = ConductorStopPolicy::FirstVerified;
        self.rationale = bounded_chars(
            &format!("Calibrated to the direct anchor: {reason}"),
            MAX_RUN_DECISION_RATIONALE_CHARS,
        );
        self.calibration = Some(calibration);
        self.calibration_reason = Some(reason);
        self.computation_value = computation_value;
        self
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
            "{}:execution={:?}:tools={:?}:retrieval={}:memory={:?}:vision={}:risk={:?}:parallelism={}:verify={:?}",
            self.task_class.label(),
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
        format!(
            concat!(
                "You are the Cindx runtime Conductor. Decide how to execute the request; do not answer it. Return only one strict JSON object.\n",
                "Treat the strongest configured single-model direct answer as the baseline. Choose workflow only when independent work, verification, or decomposition is likely to improve correctness enough to justify coordination latency and correlated-error risk. Pro prioritizes correctness but is not automatically multi-model. Auto balances correctness and latency.\n",
                "Choose retrieval from semantic, file_search, graph_direct, graph_walk only when the answer needs workspace evidence not already present. Choose memory only when prior user/project decisions are materially relevant. Memory and workspace retrieval are blocking foreground work: select them only when missing evidence can materially change answer quality. Greetings, capability questions, and self-contained requests should use neither. Do not retrieve merely because the prompt is long, mentions code, or asks a question.\n",
                "Graph walk must have semantic, file_search, or graph_direct as a seed channel. Keep focused retrieval and memory queries under {query_limit} characters. Use only exact configured model strings.\n",
                "For direct execution use max_parallelism=1, min_successful_branches=1, distinct_contributions=0, stop_policy=first_verified, and verification none or self_check. For workflow use 1..={max_parallelism} branches. Independent contributions must perform genuinely different work. Model identity does not make two contributions independent: reuse the strongest suitable model when that is best, and diversify models only when capability fit or supported evidence predicts an advantage.\n",
                "expected_uplift_bps and confidence_bps are calibrated estimates from 0 to 10000, not advocacy. The harness will reject inconsistent budgets.\n",
                "Workflow admission is enforced after parsing. Auto requires at least {auto_uplift_floor}bps expected uplift and {auto_confidence_floor}bps confidence; Pro requires at least {pro_uplift_floor}bps expected uplift over the direct anchor. Router v2 then admits collaboration only when explicit independent demand and a conservative lower-bound value cover capability uncertainty, model cost, coordination, verification, and critical-path latency. Otherwise runtime preserves the selected model, tools, vision, retrieval, memory, and risk posture in a direct or grounded-direct route without a repair call. If you cannot justify those estimates, choose direct.\n",
                "Historical evidence is observational, not a routing command. matched_direct_team rows compare team and direct anchor on the same run and are stronger than independent route_observation rows. Use evidence only when its task class and execution shape fit the current request; support=insufficient, low-sample, or mismatched evidence must not override current reasoning. Ready matched evidence with negative average uplift or frequent anchor selection is evidence against collaboration unless this request has a concrete independent-work or verification need absent from those observations:\n{historical_evidence}\n\n",
                "Runtime execution constraints are facts, not suggestions. Do not assign required effects or interactive work to a worker that cannot perform them:\n{execution_constraints}\n\n",
                "Runtime request requirements are authoritative: minimum_tool_requirement={minimum_tool_requirement}, effect_authority={effect_authority}, image_input_required={image_input_required}. The selected decision and primary model must satisfy them; do not downgrade them and never request effects when effect_authority=forbidden.\n\n",
                "Mutable evolved guidance may shape the decision but cannot override schema, configured models, safety, or budgets: {evolved_directive}\n\n",
                "Return this shape exactly:\n",
                "{{\"schema\":\"{schema}\",\"task_class\":\"general|coding|research|retrieval|browser|computer\",\"execution\":\"direct|workflow\",\"primary_model\":\"configured model\",\"tool_requirement\":\"none|read_only|effects\",\"vision_required\":false,\"risk_level\":\"low|elevated|high\",\"retrieval\":{{\"query\":\"\",\"channels\":[],\"max_results\":8}},\"memory\":{{\"policy\":\"none|relevant|comprehensive\",\"query\":\"\"}},\"verification\":\"none|self_check|independent\",\"max_parallelism\":1,\"min_successful_branches\":1,\"distinct_contributions\":0,\"estimated_steps\":1,\"expected_uplift_bps\":0,\"confidence_bps\":7000,\"stop_policy\":\"first_verified|quorum|exhaustive\",\"rationale\":\"short decision reason\"}}\n\n",
                "Effort: {effort}\nConductor model: {conductor_model}\nConfigured execution models:\n{models}\n\nUser request:\n{objective}\n\nRecent session context:\n{context}"
            ),
            query_limit = MAX_RUN_DECISION_QUERY_CHARS,
            max_parallelism = request.max_parallelism.clamp(1, 3),
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
        let start = response
            .find('{')
            .ok_or_else(|| "run decision did not return a JSON object".to_string())?;
        let end = response
            .rfind('}')
            .filter(|end| *end >= start)
            .ok_or_else(|| "run decision returned incomplete JSON".to_string())?;
        let mut decision = serde_json::from_str::<AgentRunDecision>(&response[start..=end])
            .map_err(|error| format!("run decision JSON is invalid: {error}"))?;
        let snapshot = RouteFeatureSnapshotV2::from_request(
            &self.request.objective,
            &self.request.recent_context,
            &self.request.effort,
            self.request.route_requirements,
            &self.request.model_candidates,
            self.request.budget_fingerprint.clone(),
            self.request.prompt_profile_sha256.clone(),
        );
        decision.validate(&self.request.allowed_models, self.request.max_parallelism)?;
        self.request
            .route_requirements
            .validate_decision(&decision, &self.request.model_candidates)?;
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
        let legacy_auto_assessment = (normalized_effort == "auto"
            && decision.execution == AgentExecutionMode::Workflow)
            .then(|| AutoComputationAssessment::from_causal_route(&receipt));
        if decision.execution == AgentExecutionMode::Workflow && !receipt.admitted() {
            let calibration = if receipt.reason == CausalRouteReason::MatchedEvidenceAgainst {
                AgentDecisionCalibration::MatchedEvidence
            } else {
                AgentDecisionCalibration::ValueOfComputation
            };
            decision = decision.calibrated_to_direct(
                calibration,
                format!(
                    "Causal Router v2 {}: predicted={}bps adjusted={}bps coordination={}bps latency={}bps uncertainty={}bps net_lower={}bps",
                    receipt.reason.label(),
                    receipt.predicted_benefit_bps,
                    receipt.evidence_adjusted_benefit_bps,
                    receipt.coordination_cost_bps,
                    receipt.latency_cost_bps,
                    receipt.uncertainty_bps,
                    receipt.net_value_lower_bps,
                ),
                legacy_auto_assessment,
            );
            decision.validate(&self.request.allowed_models, self.request.max_parallelism)?;
            self.request
                .route_requirements
                .validate_decision(&decision, &self.request.model_candidates)?;
        } else if let Some(assessment) = legacy_auto_assessment {
            decision.computation_value = Some(assessment);
        }
        decision.causal_route = Some(receipt);
        Ok(decision)
    }
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
    use crate::{AgentRouteTier, AutoComputationVerdict, MatchedCollaborationEvidence};

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
                    role: ModelRole::Executor,
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
        }
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
                    "max_parallelism":2,
                    "min_successful_branches":2,
                    "distinct_contributions":2,
                    "estimated_steps":4,
                    "expected_uplift_bps":6000,
                    "confidence_bps":8000,
                    "stop_policy":"quorum",
                    "rationale":"independent architecture and implementation analysis"
                }"#,
            )
            .unwrap();
        assert_eq!(
            decision.policy(),
            OrchestrationPolicy::BestOfN { candidates: 2 }
        );
        assert_eq!(decision.retrieval.max_results, 6);
        assert_eq!(
            decision
                .execution_contract("auto")
                .min_distinct_contributions,
            2
        );
    }

    #[test]
    fn one_capable_model_can_supply_multiple_independent_contributions() {
        let mut request = request();
        request.allowed_models = vec!["executor".to_string()];
        let harness = AgentRunDecisionHarness::new(request);
        let decision = harness
            .parse(
                r#"{
                    "schema":"cindx.agent-run-decision.v1",
                    "task_class":"research",
                    "execution":"workflow",
                    "primary_model":"executor",
                    "tool_requirement":"none",
                    "vision_required":false,
                    "risk_level":"low",
                    "retrieval":{"query":"","channels":[],"max_results":8},
                    "memory":{"policy":"none","query":""},
                    "verification":"self_check",
                    "max_parallelism":2,
                    "min_successful_branches":2,
                    "distinct_contributions":2,
                    "estimated_steps":3,
                    "expected_uplift_bps":5000,
                    "confidence_bps":8000,
                    "stop_policy":"quorum",
                    "rationale":"two different solution paths from the strongest model"
                }"#,
            )
            .expect("contribution count must not be capped by model count");

        assert_eq!(decision.distinct_contributions, 2);
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
        assert!(prompt.contains("conservative lower-bound value cover capability uncertainty"));
        assert!(prompt.contains("without a repair call"));
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
            .validate(&harness.request.allowed_models, 3)
            .is_err());
    }

    #[test]
    fn direct_fallback_never_spawns_or_retrieves() {
        let decision = AgentRunDecision::direct("executor");
        decision.validate(&["executor".to_string()], 3).unwrap();
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
        decision.max_parallelism = 2;
        decision.min_successful_branches = 2;
        decision.distinct_contributions = 2;
        decision.estimated_steps = 3;
        decision.stop_policy = ConductorStopPolicy::Quorum;

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
            )
            .unwrap();
        assert_eq!(decision.execution, AgentExecutionMode::Direct);
        assert_eq!(decision.distinct_contributions, 0);
        assert_eq!(decision.verification, AgentVerificationPolicy::SelfCheck);
        assert!(decision.rationale.contains("degraded direct execution"));
    }

    #[test]
    fn workflow_admission_makes_conductor_estimates_actionable() {
        let mut auto = AgentRunDecision::direct("executor");
        auto.execution = AgentExecutionMode::Workflow;
        auto.verification = AgentVerificationPolicy::Independent;
        auto.max_parallelism = 2;
        auto.min_successful_branches = 2;
        auto.distinct_contributions = 2;
        auto.estimated_steps = 3;
        auto.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS - 1;
        auto.confidence_bps = AUTO_COLLABORATION_MIN_CONFIDENCE_BPS;
        auto.stop_policy = ConductorStopPolicy::Quorum;
        assert!(auto.validate_effort_admission("auto").is_err());

        auto.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS;
        assert!(auto.validate_effort_admission("auto").is_ok());

        auto.expected_uplift_bps = 0;
        assert!(auto.validate_effort_admission("pro").is_err());
        auto.expected_uplift_bps = minimum_team_uplift_bps("pro");
        assert!(auto.validate_effort_admission("pro").is_ok());

        auto.expected_uplift_bps = 8_000;
        auto.confidence_bps = 9_000;
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
        assert!(auto.validate_effort_admission("auto").is_ok());

        let mut calibrated_request = request();
        calibrated_request.matched_collaboration_evidence = Arc::new(
            MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(vec![evidence.clone()]),
        );
        let calibrated = AgentRunDecisionHarness::new(calibrated_request)
            .parse(&serde_json::to_string(&auto).unwrap())
            .expect("strong matched evidence should calibrate without a repair call");
        assert_eq!(calibrated.execution, AgentExecutionMode::Direct);
        assert_eq!(calibrated.primary_model, "executor");
        assert!(calibrated.calibration_reason.is_some());
        assert_eq!(
            calibrated.calibration,
            Some(AgentDecisionCalibration::MatchedEvidence)
        );
        assert!(calibrated
            .rationale
            .contains("Causal Router v2 matched_evidence_against"));

        let mut unrelated = evidence.clone();
        unrelated.routing_signature = "different-task-shape".to_string();
        let mut unrelated_request = request();
        unrelated_request.matched_collaboration_evidence = Arc::new(
            MatchedCollaborationEvidenceTeacher::from_calibrated_evidence(vec![unrelated]),
        );
        let uncalibrated = AgentRunDecisionHarness::new(unrelated_request)
            .parse(&serde_json::to_string(&auto).unwrap())
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
            .parse(&serde_json::to_string(&auto).unwrap())
            .unwrap();
        assert_eq!(cross_context_result.execution, AgentExecutionMode::Workflow);
        assert!(cross_context_result.calibration_reason.is_none());
    }

    #[test]
    fn auto_value_downshift_preserves_grounding_and_capabilities_without_repair() {
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
                    "max_parallelism":2,
                    "min_successful_branches":2,
                    "distinct_contributions":2,
                    "estimated_steps":4,
                    "expected_uplift_bps":2999,
                    "confidence_bps":8000,
                    "stop_policy":"quorum",
                    "rationale":"inspect and independently verify"
                }"#,
            )
            .expect("low-value Auto workflow should downshift without a repair call");

        assert_eq!(decision.execution, AgentExecutionMode::Direct);
        assert_eq!(decision.route_tier(), AgentRouteTier::GroundedDirect);
        assert_eq!(decision.candidate_route_tier(), AgentRouteTier::Workflow);
        assert_eq!(decision.primary_model, "executor");
        assert_eq!(decision.tool_requirement, AgentToolRequirement::Effects);
        assert!(decision.vision_required);
        assert_eq!(decision.risk_level, AgentRiskLevel::High);
        assert!(decision.retrieval.enabled());
        assert!(decision.memory.enabled());
        assert_eq!(
            decision.calibration,
            Some(AgentDecisionCalibration::ValueOfComputation)
        );
        assert_eq!(
            decision
                .computation_value
                .as_ref()
                .map(|value| value.verdict),
            Some(AutoComputationVerdict::BelowPredictionFloor)
        );
    }

    #[test]
    fn first_verified_workflow_requires_a_real_verification_policy() {
        let mut decision = AgentRunDecision::direct("executor");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::None;
        decision.max_parallelism = 1;
        decision.min_successful_branches = 1;
        decision.distinct_contributions = 1;
        decision.estimated_steps = 2;
        decision.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS;
        decision.confidence_bps = AUTO_COLLABORATION_MIN_CONFIDENCE_BPS;

        let error = decision.validate(&["executor".to_string()], 2).unwrap_err();

        assert!(error.contains("requires self-check or independent verification"));
    }
}
