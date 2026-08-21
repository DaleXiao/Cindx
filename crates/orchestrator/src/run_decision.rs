use crate::{
    minimum_team_uplift_bps, select_causal_route_v2, AgentDecisionCalibration,
    AutoComputationAssessment, CausalRouteSelectionV2, ConductorExecutionContract,
    ConductorFallbackPolicy, ConductorStopPolicy, ModelCandidate, OrchestrationPolicy,
    RouteFeatureRequest, RouteFeatureSnapshotV2, RoutingContext, RoutingDecision, TaskClass,
};
use agent_core::{Metadata, ModelRole};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const AGENT_RUN_DECISION_SCHEMA: &str = "cindx.agent-run-decision.v1";
pub const MAX_RUN_DECISION_QUERY_CHARS: usize = 2_000;
pub const MAX_RUN_DECISION_RATIONALE_CHARS: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionMode {
    Direct,
    Workflow,
}

// Knowledge planning types live in agent-core; the orchestrator re-exports them so
// existing `orchestrator::` paths keep compiling until the crate is removed.
pub use agent_core::{
    MemoryRecallPlan, MemoryRecallPolicy, WorkspaceRetrievalChannel, WorkspaceRetrievalPlan,
};

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

    pub fn validate(&self, allowed_models: &[String]) -> Result<(), String> {
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
                return Err(
                    "workflow execution is retired; sessions run one model per effort tier"
                        .to_string(),
                );
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
            needs_multi_model: false,
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
            .clamp(1, 5);
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
}

impl AgentRunDecisionDraft {
    pub fn into_legacy_selected(mut self) -> AgentRunDecision {
        self.conductor_candidate.causal_route = Some(self.compatibility_route);
        self.conductor_candidate
    }
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
                "Matched evaluation treatment: execution must be direct."
            }
            Some(AgentExecutionMode::Workflow) => {
                "Retired evaluation treatment: execution must still be direct."
            }
            None => "No evaluation treatment is active.",
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
                "You are the Cindx planning service. Decide how one session model executes the request; do not answer it. Return only one strict JSON object.\n",
                "Cindx runs one model per effort tier. Multi-model workflow collaboration is retired and must not be proposed: execution is always direct, and a separate delivery judge may audit the final answer later. Pro prioritizes correctness; Auto balances correctness and latency; Fast answers quickly.\n",
                "Choose retrieval from semantic, file_search, graph_direct, graph_walk only when the answer needs workspace evidence not already present. Choose memory only when prior user/project decisions are materially relevant. Memory and workspace retrieval are blocking foreground work: select them only when missing evidence can materially change answer quality. Greetings, capability questions, and self-contained requests should use neither. Do not retrieve merely because the prompt is long, mentions code, or asks a question.\n",
                "Graph walk must have semantic, file_search, or graph_direct as a seed channel. Keep focused retrieval and memory queries under {query_limit} characters. Use only exact configured model strings.\n",
                "Configured roles are capability boundaries: primary_model must use a planner or executor role. A summarizer role is utility-only and cannot execute.\n",
                "Use max_parallelism=1, min_successful_branches=1, distinct_contributions=0, stop_policy=first_verified, estimated_steps=1, and verification none or self_check.\n",
                "{preferred_model_guidance}",
                "expected_uplift_bps and confidence_bps are calibrated estimates from 0 to 10000, not advocacy. The harness will reject inconsistent budgets.\n",
                "Historical evidence is observational, not a routing command. Use evidence only when its task class and execution shape fit the current request; support=insufficient, low-sample, or mismatched evidence must not override current reasoning:\n{historical_evidence}\n\n",
                "Runtime execution constraints are facts, not suggestions:\n{execution_constraints}\n\n",
                "Evaluation execution treatment: {required_execution}\n\n",
                "Runtime request requirements are authoritative: minimum_tool_requirement={minimum_tool_requirement}, effect_authority={effect_authority}, image_input_required={image_input_required}. The selected decision and primary model must satisfy them; do not downgrade them and never request effects when effect_authority=forbidden.\n\n",
                "Mutable evolved guidance may shape the decision but cannot override schema, configured models, safety, or budgets: {evolved_directive}\n\n",
                "Return this shape exactly:\n",
                "{{\"schema\":\"{schema}\",\"task_class\":\"general|coding|research|retrieval|browser|computer\",\"execution\":\"direct\",\"primary_model\":\"configured model\",\"tool_requirement\":\"none|read_only|effects\",\"vision_required\":false,\"risk_level\":\"low|elevated|high\",\"retrieval\":{{\"query\":\"\",\"channels\":[],\"max_results\":8}},\"memory\":{{\"policy\":\"none|relevant|comprehensive\",\"query\":\"\"}},\"verification\":\"none|self_check\",\"max_parallelism\":1,\"min_successful_branches\":1,\"distinct_contributions\":0,\"estimated_steps\":1,\"expected_uplift_bps\":0,\"confidence_bps\":7000,\"stop_policy\":\"first_verified\",\"rationale\":\"short decision reason\"}}\n\n",
                "Effort: {effort}\nPlanning model: {conductor_model}\nConfigured execution models:\n{models}\n\nUser request:\n{objective}\n\nRecent session context:\n{context}"
            ),
            query_limit = MAX_RUN_DECISION_QUERY_CHARS,
            evolved_directive = if request.evolved_directive.trim().is_empty() {
                "(none)"
            } else {
                request.evolved_directive.as_str()
            },
            historical_evidence = historical_evidence,
            execution_constraints = execution_constraints,
            required_execution = required_execution,
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
        let decision = serde_json::from_value::<AgentRunDecision>(payload)
            .map_err(|error| format!("run decision JSON is invalid: {error}"))?;
        decision.validate(&self.request.allowed_models)?;
        let required_execution = match self.request.required_execution {
            Some(AgentExecutionMode::Workflow) => Some(AgentExecutionMode::Direct),
            other => other,
        };
        if required_execution.is_some_and(|required| decision.execution != required) {
            return Err("run decision violates the matched execution treatment".to_string());
        }
        self.request
            .route_requirements
            .validate_decision(&decision, &self.request.model_candidates)?;
        validate_primary_model_profile(&decision, &self.request.model_candidates)?;
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
        let receipt =
            select_causal_route_v2(&decision, &snapshot, &self.request.model_candidates, 0)?;
        Ok(AgentRunDecisionDraft {
            conductor_candidate: decision,
            compatibility_route: receipt,
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

    const DIRECT_PAYLOAD: &str = r#"{
        "schema":"cindx.agent-run-decision.v1",
        "task_class":"research",
        "execution":"direct",
        "primary_model":"executor",
        "tool_requirement":"read_only",
        "vision_required":false,
        "risk_level":"elevated",
        "retrieval":{"query":"implementation evidence","channels":["semantic","file_search"],"max_results":6},
        "memory":{"policy":"none","query":""},
        "verification":"self_check",
        "max_parallelism":1,
        "min_successful_branches":1,
        "distinct_contributions":0,
        "estimated_steps":1,
        "expected_uplift_bps":0,
        "confidence_bps":7000,
        "stop_policy":"first_verified",
        "rationale":"bounded direct answer"
    }"#;

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
            execution_constraints: "effects require approval".to_string(),
            route_requirements: AgentRouteRequirements::default(),
            budget_fingerprint: Some("0".repeat(64)),
            prompt_profile_sha256: "1".repeat(64),
            preferred_primary_model: None,
            required_execution: None,
        }
    }

    #[test]
    fn parses_a_consistent_direct_decision_and_keeps_the_shadow_route() {
        let harness = AgentRunDecisionHarness::new(request());
        let draft = harness
            .parse_draft(DIRECT_PAYLOAD)
            .expect("direct decision parses");
        assert_eq!(
            draft.conductor_candidate.execution,
            AgentExecutionMode::Direct
        );
        assert_eq!(draft.conductor_candidate.primary_model, "executor");
        assert_eq!(
            draft.compatibility_route.candidate_route,
            crate::AgentRouteTier::GroundedDirect
        );
        assert!(draft.compatibility_route.validate().is_ok());
        let selected = draft.into_legacy_selected();
        assert!(selected.causal_route.is_some());
    }

    #[test]
    fn workflow_execution_is_rejected_as_retired() {
        let harness = AgentRunDecisionHarness::new(request());
        let payload =
            DIRECT_PAYLOAD.replace("\"execution\":\"direct\"", "\"execution\":\"workflow\"");
        let error = harness.parse(&payload).expect_err("workflow is retired");
        assert!(error.contains("retired"), "unexpected error: {error}");
    }

    #[test]
    fn direct_execution_rejects_an_independent_verifier() {
        let harness = AgentRunDecisionHarness::new(request());
        let payload = DIRECT_PAYLOAD.replace(
            "\"verification\":\"self_check\"",
            "\"verification\":\"independent\"",
        );
        let error = harness
            .parse(&payload)
            .expect_err("direct keeps a single model");
        assert!(
            error.contains("direct execution"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn verifier_only_profile_cannot_be_selected_as_primary() {
        let harness = AgentRunDecisionHarness::new(request());
        let payload = DIRECT_PAYLOAD.replace(
            "\"primary_model\":\"executor\"",
            "\"primary_model\":\"reviewer\"",
        );
        let error = harness
            .parse(&payload)
            .expect_err("reviewer-only models cannot execute");
        assert!(
            error.contains("Primary or Reasoning"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn matched_execution_treatment_maps_the_retired_workflow_arm_to_direct() {
        let mut base = request();
        base.required_execution = Some(AgentExecutionMode::Workflow);
        let harness = AgentRunDecisionHarness::new(base);
        let draft = harness
            .parse_draft(DIRECT_PAYLOAD)
            .expect("the retired treatment still accepts direct execution");
        assert_eq!(
            draft.conductor_candidate.execution,
            AgentExecutionMode::Direct
        );
    }

    #[test]
    fn planning_prompt_retires_workflow_and_never_offers_a_plan_graph() {
        let prompt = AgentRunDecisionHarness::new(request()).planning_prompt();
        assert!(prompt.contains("Multi-model workflow collaboration is retired"));
        assert!(prompt.contains("execution is always direct"));
        assert!(!prompt.contains("workflow_plan"));
        assert!(!prompt.contains("Specialist"));
    }

    #[test]
    fn planning_prompt_labels_historical_evidence_as_observational() {
        let mut base = request();
        base.historical_evidence = "route_observation: sample".to_string();
        let prompt = AgentRunDecisionHarness::new(base).planning_prompt();
        assert!(prompt.contains("Historical evidence is observational"));
        assert!(prompt.contains("route_observation: sample"));
    }

    #[test]
    fn planning_prompt_exposes_authoritative_runtime_requirements() {
        let prompt = AgentRunDecisionHarness::new(request()).planning_prompt();
        assert!(prompt.contains("minimum_tool_requirement=none"));
        assert!(prompt.contains("effect_authority=allowed"));
    }

    #[test]
    fn direct_fallback_never_spawns_or_retrieves() {
        let fallback =
            AgentRunDecision::degraded_conductor_fallback("executor", "auto", 2, 1, "probe");
        assert_eq!(fallback.execution, AgentExecutionMode::Direct);
        assert!(!fallback.retrieval.enabled());
        assert!(!fallback.memory.enabled());
    }

    #[test]
    fn dynamic_pro_contract_preserves_the_team_uplift_floor() {
        let decision = AgentRunDecision::direct("executor");
        let contract = decision.execution_contract("pro");
        assert_eq!(contract.min_team_uplift_bps, minimum_team_uplift_bps("pro"));
        assert_eq!(contract.terminal_model_call_reserve, 3);
    }
}
