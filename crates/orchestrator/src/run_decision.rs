use crate::{
    minimum_team_uplift_bps, ConductorExecutionContract, ConductorFallbackPolicy,
    ConductorStopPolicy, MatchedCollaborationEvidence, ModelCandidate, OrchestrationPolicy,
    RoutingContext, RoutingDecision, TaskClass, AUTO_COLLABORATION_MIN_CONFIDENCE_BPS,
    AUTO_COLLABORATION_MIN_UPLIFT_BPS,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolRequirement {
    None,
    ReadOnly,
    Effects,
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
                let configured_model_count = allowed_models
                    .iter()
                    .map(|model| model.trim())
                    .filter(|model| !model.is_empty())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                if self.distinct_contributions > configured_model_count {
                    return Err(format!(
                        "workflow requests {} distinct contributions but only {configured_model_count} distinct configured model(s) are available",
                        self.distinct_contributions
                    ));
                }
                if self.stop_policy == ConductorStopPolicy::FirstVerified
                    && self.min_successful_branches != 1
                {
                    return Err(
                        "first_verified workflow must require exactly one successful branch"
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

    pub fn validate_effort_admission(
        &self,
        effort: &str,
        matched_evidence: &[MatchedCollaborationEvidence],
    ) -> Result<(), String> {
        if self.execution != AgentExecutionMode::Workflow {
            return Ok(());
        }
        let normalized_effort = effort.trim().to_ascii_lowercase();
        let required_uplift = match normalized_effort.as_str() {
            "auto" => {
                if self.expected_uplift_bps < AUTO_COLLABORATION_MIN_UPLIFT_BPS
                    || self.confidence_bps < AUTO_COLLABORATION_MIN_CONFIDENCE_BPS
                {
                    return Err(format!(
                        "auto workflow is below its collaboration admission floor: uplift={}bps confidence={}bps",
                        self.expected_uplift_bps, self.confidence_bps
                    ));
                }
                AUTO_COLLABORATION_MIN_UPLIFT_BPS
            }
            "pro" => {
                let minimum_uplift = minimum_team_uplift_bps("pro");
                if self.expected_uplift_bps < minimum_uplift {
                    return Err(format!(
                        "pro workflow is below its direct-anchor uplift floor: uplift={}bps required={}bps",
                        self.expected_uplift_bps, minimum_uplift
                    ));
                }
                minimum_uplift
            }
            _ => {
                return Err("fast effort cannot admit a collaboration workflow".to_string());
            }
        };
        let signature = self.learning_signature();
        if let Some(evidence) = matched_evidence.iter().find(|evidence| {
            evidence.task_class == self.task_class
                && evidence.effort.eq_ignore_ascii_case(&normalized_effort)
                && evidence.routing_signature == signature
                && evidence.strong_evidence_against_collaboration(required_uplift)
        }) {
            return Err(format!(
                "matched direct-anchor evidence rejects collaboration for this task shape: samples={} average_uplift={}bps below_admission_floor_lower_confidence={:.2}",
                evidence.examples,
                evidence.average_uplift_bps,
                evidence.below_admission_floor_confidence
            ));
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
        ConductorExecutionContract {
            task_class: self.task_class.clone(),
            effort: effort.clone(),
            policy: self.policy(),
            expected_uplift_bps: self.expected_uplift_bps,
            confidence_bps: self.confidence_bps,
            max_parallelism: self.max_parallelism,
            min_successful_branches: self.min_successful_branches,
            verification_required: self.verification != AgentVerificationPolicy::None,
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
    pub max_parallelism: usize,
    pub evolved_directive: String,
    pub historical_evidence: String,
    pub matched_collaboration_evidence: Vec<MatchedCollaborationEvidence>,
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
            .map(|model| format!("- {model}"))
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
        format!(
            concat!(
                "You are the Cindx runtime Conductor. Decide how to execute the request; do not answer it. Return only one strict JSON object.\n",
                "Treat the strongest configured single-model direct answer as the baseline. Choose workflow only when independent work, verification, or decomposition is likely to improve correctness enough to justify coordination latency and correlated-error risk. Pro prioritizes correctness but is not automatically multi-model. Auto balances correctness and latency.\n",
                "Choose retrieval from semantic, file_search, graph_direct, graph_walk only when the answer needs workspace evidence not already present. Choose memory only when prior user/project decisions are materially relevant. Do not retrieve merely because the prompt is long, mentions code, or asks a question.\n",
                "Graph walk must have semantic, file_search, or graph_direct as a seed channel. Keep focused retrieval and memory queries under {query_limit} characters. Use only exact configured model strings.\n",
                "For direct execution use max_parallelism=1, min_successful_branches=1, distinct_contributions=0, stop_policy=first_verified, and verification none or self_check. For workflow use 1..={max_parallelism} branches. Independent contributions must perform genuinely different work and, when more than one distinct configured model is available, must use different model strings; never count duplicate calls to one model as model diversity. Never request more distinct contributions than the distinct configured model pool can supply.\n",
                "expected_uplift_bps and confidence_bps are calibrated estimates from 0 to 10000, not advocacy. The harness will reject inconsistent budgets.\n",
                "Workflow admission is enforced after parsing: Auto requires at least {auto_uplift_floor}bps expected uplift and {auto_confidence_floor}bps confidence; Pro requires at least {pro_uplift_floor}bps expected uplift over the direct anchor. If you cannot justify those estimates, choose direct.\n",
                "Historical evidence is observational, not a routing command. matched_direct_team rows compare team and direct anchor on the same run and are stronger than independent route_observation rows. Use evidence only when its task class and execution shape fit the current request; support=insufficient, low-sample, or mismatched evidence must not override current reasoning. Ready matched evidence with negative average uplift or frequent anchor selection is evidence against collaboration unless this request has a concrete independent-work or verification need absent from those observations:\n{historical_evidence}\n\n",
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
        let decision = serde_json::from_str::<AgentRunDecision>(&response[start..=end])
            .map_err(|error| format!("run decision JSON is invalid: {error}"))?;
        decision.validate(&self.request.allowed_models, self.request.max_parallelism)?;
        decision.validate_effort_admission(
            &self.request.effort,
            &self.request.matched_collaboration_evidence,
        )?;
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

    fn request() -> AgentRunDecisionRequest {
        AgentRunDecisionRequest {
            objective: "Compare two implementation strategies".to_string(),
            recent_context: String::new(),
            effort: "auto".to_string(),
            conductor_model: "planner".to_string(),
            allowed_models: vec!["executor".to_string(), "reviewer".to_string()],
            max_parallelism: 3,
            evolved_directive: String::new(),
            historical_evidence: String::new(),
            matched_collaboration_evidence: Vec::new(),
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
                    "expected_uplift_bps":3200,
                    "confidence_bps":7200,
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
        assert!(auto.validate_effort_admission("auto", &[]).is_err());

        auto.expected_uplift_bps = AUTO_COLLABORATION_MIN_UPLIFT_BPS;
        assert!(auto.validate_effort_admission("auto", &[]).is_ok());

        auto.expected_uplift_bps = 0;
        assert!(auto.validate_effort_admission("pro", &[]).is_err());
        auto.expected_uplift_bps = minimum_team_uplift_bps("pro");
        assert!(auto.validate_effort_admission("pro", &[]).is_ok());

        auto.expected_uplift_bps = 3_500;
        let evidence = MatchedCollaborationEvidence {
            task_class: auto.task_class.clone(),
            effort: "auto".to_string(),
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
        let error = auto
            .validate_effort_admission("auto", std::slice::from_ref(&evidence))
            .unwrap_err();
        assert!(error.contains("matched direct-anchor evidence"));

        let mut unrelated = evidence;
        unrelated.routing_signature = "different-task-shape".to_string();
        assert!(auto.validate_effort_admission("auto", &[unrelated]).is_ok());
    }
}
