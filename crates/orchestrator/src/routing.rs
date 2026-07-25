use super::*;

mod workflow_topology_learning;

use workflow_topology_learning::ADAPTIVE_WORKFLOW_PRIOR_MIN_QUALITY;
pub use workflow_topology_learning::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
                "fix",
                "modify",
                "edit",
                "refactor",
                "implement",
                "debug",
                "compile",
                "test",
                "run",
                "修复",
                "修改",
                "重构",
                "实现",
                "调试",
                "编译",
                "测试",
                "运行",
                "执行",
                "定位",
                "排查",
            ],
        );
        let workspace_reference = contains_any(
            prompt,
            &[
                "workspace",
                "repo",
                "repository",
                "project",
                "codebase",
                "this file",
                "these files",
                "工作区",
                "仓库",
                "项目",
                "代码库",
                "这个文件",
                "这些文件",
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
                "security",
                "legal",
                "medical",
                "financial",
                "production",
                "migration",
                "critical",
                "high-stakes",
                "安全",
                "法律",
                "医疗",
                "财务",
                "生产",
                "迁移",
                "高风险",
                "关键",
            ],
        );
        let deep_analysis = contains_any(
            prompt,
            &[
                "compare",
                "tradeoff",
                "trade-off",
                "architecture",
                "strategy",
                "root cause",
                "investigate",
                "comprehensive",
                "alternatives",
                "方案",
                "比较",
                "对比",
                "权衡",
                "架构",
                "策略",
                "根因",
                "深入",
                "全面",
                "多条路径",
            ],
        );
        let parallelizable = !capability_question
            && contains_any(
                prompt,
                &[
                    "compare",
                    "alternatives",
                    "independent",
                    "multiple options",
                    "second opinion",
                    "cross-check",
                    "parallel",
                    "sources",
                    "citations",
                    "比较",
                    "对比",
                    "多个方案",
                    "independent analysis",
                    "investigate",
                    "root cause",
                    "独立分析",
                    "交叉验证",
                    "并行",
                    "多条路径",
                    "来源",
                    "引用",
                    "根因",
                    "排查",
                ],
            );
        let multi_phase = !capability_question
            && contains_any(
                prompt,
                &[
                    " and then ",
                    " then ",
                    " after that ",
                    "并且",
                    "然后",
                    "之后",
                    "再运行",
                    "再检查",
                    "同时",
                    "and run tests",
                    "fix and test",
                    "implement and test",
                    "修改并",
                    "修复并",
                    "实现并",
                    "排查并",
                    "并运行",
                    "并测试",
                    "并检查",
                    "并验证",
                ],
            );
        let latency_sensitive = !capability_question
            && contains_any(
                prompt,
                &[
                    "quick",
                    "quickly",
                    "fast",
                    "brief",
                    "one sentence",
                    "简单回答",
                    "快速",
                    "尽快",
                    "一句话",
                    "简短",
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
                        "screenshot",
                        "screen",
                        "visible",
                        "ui",
                        "截图",
                        "屏幕",
                        "界面",
                        "可见",
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
            (1u8 + u8::from(needs_tools)
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
            complexity_score: if capability_question {
                0
            } else {
                complexity_score
            },
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingTelemetry {
    pub task_class: TaskClass,
    pub context_signature: String,
    pub selected_policy: OrchestrationPolicy,
    pub selected_model: String,
    pub latency_ms: u64,
    pub outcome: RoutingOutcome,
    #[serde(default)]
    pub quality_score: Option<f32>,
    #[serde(default)]
    pub verification_passed: Option<bool>,
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
    pub successes: usize,
    pub success_rate: f32,
    pub success_confidence: f64,
    pub average_quality_score: Option<f32>,
    pub verification_rate: Option<f32>,
    pub average_latency_ms: u64,
    pub average_cost_proxy: u64,
}

const LEARNED_ROUTER_MIN_EXAMPLES: usize = 4;
pub(crate) const LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE: f64 = 0.50;

impl LearnedRoute {
    pub fn evidence_ready(&self) -> bool {
        self.examples >= LEARNED_ROUTER_MIN_EXAMPLES
            && self.success_confidence >= LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityRubricScore {
    pub correctness: u8,
    pub evidence: u8,
    pub completion: u8,
    pub safety: u8,
}

impl QualityRubricScore {
    pub fn validate(&self) -> Result<(), String> {
        if [
            self.correctness,
            self.evidence,
            self.completion,
            self.safety,
        ]
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

fn is_self_contained_direct(context: &RoutingContext) -> bool {
    matches!(context.task_class, TaskClass::General | TaskClass::Coding)
        && !context.needs_tools
        && !context.needs_retrieval
        && !context.needs_vision
        && !context.verification_required
        && !context.high_stakes
        && !context.parallelizable
        && context.estimated_steps <= 2
        && context.complexity_score <= 1
}

fn requires_ultra(context: &RoutingContext) -> bool {
    let contract =
        ConductorExecutionContract::from_routing(context, "auto", OrchestrationPolicy::AutoRouter);
    context.needs_multi_model
        || (!context.latency_sensitive
            && !matches!(context.task_class, TaskClass::Browser | TaskClass::Computer)
            && ((context.high_stakes && context.complexity_score >= 3)
                || (context.parallelizable
                    && context.complexity_score >= 4
                    && context.estimated_steps >= 4))
            && contract.should_auto_collaborate())
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
            TaskClass::Coding if is_self_contained_direct(context) => (
                OrchestrationPolicy::Single,
                "self-contained coding question does not need tools or workspace context",
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
            TaskClass::General if is_self_contained_direct(context) => (
                OrchestrationPolicy::Single,
                "self-contained general prompt can run directly",
            ),
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
        metadata.insert(
            "task_class".to_string(),
            context.task_class.label().to_string(),
        );
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
                .max_by(|(_, left), (_, right)| left.compare_preference(right))
            {
                if let Some(policy) = parse_policy(&policy_label) {
                    routes.insert(
                        context_signature,
                        LearnedRoute {
                            task_class: accumulator
                                .task_class
                                .clone()
                                .unwrap_or(TaskClass::General),
                            policy,
                            model,
                            examples: accumulator.examples,
                            successes: accumulator.successes,
                            success_rate: accumulator.success_rate(),
                            success_confidence: accumulator.success_confidence(),
                            average_quality_score: accumulator.average_quality_score(),
                            verification_rate: accumulator.verification_rate(),
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
        let Some(route) = self.routes.get(&context.learning_signature()) else {
            return baseline;
        };
        if !route.evidence_ready()
            || !learned_policy_allowed(context, &route.policy)
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
            "historical executions provide sufficient evidence for this policy and model in the exact context",
        );
        decision.model = route.model.clone();
        decision.explanation = format!(
            "class={} learned_policy={} learned_model={} examples={} success_rate={:.2} confidence={:.3} quality={} verification={}",
            context.task_class.label(),
            route.policy.label(),
            route.model,
            route.examples,
            route.success_rate,
            route.success_confidence,
            route
                .average_quality_score
                .map(|score| format!("{score:.3}"))
                .unwrap_or_else(|| "unrated".to_string()),
            route
                .verification_rate
                .map(|rate| format!("{rate:.3}"))
                .unwrap_or_else(|| "unrated".to_string())
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
        decision.metadata.insert(
            "learned_success_confidence".to_string(),
            format!("{:.3}", route.success_confidence),
        );
        if let Some(score) = route.average_quality_score {
            decision
                .metadata
                .insert("learned_quality_score".to_string(), format!("{score:.3}"));
        }
        if let Some(rate) = route.verification_rate {
            decision.metadata.insert(
                "learned_verification_rate".to_string(),
                format!("{rate:.3}"),
            );
        }
        decision
    }

    pub fn learned_route(&self, task_class: &TaskClass) -> Option<&LearnedRoute> {
        self.routes
            .values()
            .filter(|route| &route.task_class == task_class)
            .max_by_key(|route| route.examples)
    }

    pub fn learned_route_for_context(&self, context: &RoutingContext) -> Option<&LearnedRoute> {
        self.routes.get(&context.learning_signature())
    }
}

#[derive(Debug, Clone, Default)]
struct RouteAccumulator {
    task_class: Option<TaskClass>,
    examples: usize,
    successes: usize,
    quality_total: f32,
    quality_examples: usize,
    verified: usize,
    verification_examples: usize,
    latency_ms: u64,
    cost_proxy: u64,
}

impl RouteAccumulator {
    fn record(&mut self, telemetry: &RoutingTelemetry) {
        self.task_class = Some(telemetry.task_class.clone());
        self.examples += 1;
        if telemetry.outcome.is_success()
            && telemetry
                .quality_score
                .is_none_or(|score| score >= ADAPTIVE_WORKFLOW_PRIOR_MIN_QUALITY)
            && telemetry.verification_passed != Some(false)
        {
            self.successes += 1;
        }
        if let Some(score) = telemetry.quality_score {
            self.quality_total += score.clamp(0.0, 1.0);
            self.quality_examples += 1;
        }
        if let Some(verified) = telemetry.verification_passed {
            self.verified += usize::from(verified);
            self.verification_examples += 1;
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

    fn success_confidence(&self) -> f64 {
        wilson_lower_bound(self.successes, self.examples)
    }

    fn average_quality_score(&self) -> Option<f32> {
        (self.quality_examples > 0).then(|| self.quality_total / self.quality_examples as f32)
    }

    fn verification_rate(&self) -> Option<f32> {
        (self.verification_examples > 0)
            .then(|| self.verified as f32 / self.verification_examples as f32)
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

    fn compare_preference(&self, other: &Self) -> std::cmp::Ordering {
        self.success_confidence()
            .total_cmp(&other.success_confidence())
            .then_with(|| {
                self.average_quality_score()
                    .unwrap_or(self.success_rate())
                    .total_cmp(
                        &other
                            .average_quality_score()
                            .unwrap_or(other.success_rate()),
                    )
            })
            .then_with(|| {
                self.verification_rate()
                    .unwrap_or(0.0)
                    .total_cmp(&other.verification_rate().unwrap_or(0.0))
            })
            .then_with(|| self.success_rate().total_cmp(&other.success_rate()))
            .then_with(|| self.examples.cmp(&other.examples))
            .then_with(|| other.average_latency_ms().cmp(&self.average_latency_ms()))
            .then_with(|| other.average_cost_proxy().cmp(&self.average_cost_proxy()))
    }
}

fn wilson_lower_bound(successes: usize, trials: usize) -> f64 {
    if trials == 0 {
        return 0.0;
    }
    const Z: f64 = 1.959_963_984_540_054;
    let n = trials as f64;
    let rate = successes.min(trials) as f64 / n;
    let z2 = Z * Z;
    let denominator = 1.0 + z2 / n;
    let center = rate + z2 / (2.0 * n);
    let margin = Z * ((rate * (1.0 - rate) + z2 / (4.0 * n)) / n).sqrt();
    ((center - margin) / denominator).clamp(0.0, 1.0)
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
            "fix",
            "modify",
            "edit",
            "refactor",
            "debug",
            "run",
            "test",
            "workspace",
            "repo",
            "project",
            "修复",
            "修改",
            "重构",
            "调试",
            "运行",
            "测试",
            "工作区",
            "仓库",
            "项目",
            "这个文件",
            "这些文件",
        ],
    ) {
        return false;
    }
    let compact = normalized
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let compact = compact.trim_end_matches(['?', '？', '。', '!', '！']);
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
        normalized.trim_end_matches(['?', '!', '.']),
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
            "computer",
            "desktop",
            "screenshot",
            "screen",
            "click",
            "keyboard",
            "电脑",
            "桌面",
            "截图",
            "屏幕",
            "点击",
            "键盘",
        ],
    ) {
        TaskClass::Computer
    } else if contains_any(
        prompt,
        &[
            "browser",
            "webpage",
            "website",
            "scroll",
            "form",
            "浏览器",
            "网页",
            "网站",
            "滚动",
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
            "code",
            "codebase",
            "rust",
            "typescript",
            "file",
            "files",
            "test",
            "tests",
            "compile",
            "build",
            "bug",
            "implement",
            "modify",
            "edit",
            "refactor",
            "debug",
            "function",
            "parser",
            "代码",
            "编程",
            "文件",
            "测试",
            "编译",
            "错误",
            "修复",
            "修改",
            "调试",
            "重构",
            "实现",
        ],
    ) {
        TaskClass::Coding
    } else if contains_any(
        prompt,
        &[
            "research",
            "compare",
            "investigate",
            "latest",
            "study",
            "collaboration",
            "multi-model",
            "multiple models",
            "orchestration",
            "orchestrator",
            "fugu",
            "reproduce",
            "replicate",
            "研究",
            "调研",
            "比较",
            "对比",
            "分析",
            "调查",
            "最新",
            "评估",
            "对标",
            "协同",
            "协作",
            "多模型",
            "多个模型",
            "复现",
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

pub fn evaluate_routing_telemetry(telemetry: &[RoutingTelemetry]) -> OperationalEvaluationReport {
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
    let total_latency = telemetry.iter().map(|entry| entry.latency_ms).sum::<u64>();
    let total_cost = telemetry.iter().map(|entry| entry.cost_proxy).sum::<u64>();
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
        *model_counts
            .entry(entry.selected_model.clone())
            .or_default() += 1;
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
    if needle
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        normalized
            .split(|character: char| !character.is_alphanumeric())
            .any(|token| token == needle)
    } else {
        normalized.contains(&needle)
    }
}
