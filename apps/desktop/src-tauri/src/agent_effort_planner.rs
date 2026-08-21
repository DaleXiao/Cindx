use agent_core::{
    sha256_hex, MemoryRecallPlan, MemoryRecallPolicy, Metadata, TaskClass,
    WorkspaceRetrievalChannel, WorkspaceRetrievalPlan,
};
use orchestrator::{
    AgentEffectAuthority, AgentRouteRequirements, AgentRunDecision, ConductorExecutionContract,
    ConductorFallbackPolicy, ConductorStopPolicy, OrchestrationPolicy,
    MAX_RUN_DECISION_QUERY_CHARS,
};

const WORKSPACE_RETRIEVAL_MAX_RESULTS: usize = 8;

/// The minimal, effort-tier run plan that replaces the orchestrator/conductor
/// planning surface. It carries only what the loop and preparation actually need:
/// the effort label, the tier-selected primary model, fixed single-model
/// scheduling facts, and the deterministic knowledge decision. No conductor,
/// routing, or collaboration concepts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct EffortRunPlan {
    pub(crate) effort_label: String,
    pub(crate) primary_model: String,
    pub(crate) collaboration_policy: String,
    pub(crate) task_class: String,
    pub(crate) tool_requirement: String,
    pub(crate) vision_required: bool,
    pub(crate) knowledge: KnowledgeDecision,
}

/// The deterministic knowledge (durable memory + workspace retrieval) decision the
/// effort planner produces for a run, replacing the conductor's memory/retrieval
/// fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct KnowledgeDecision {
    pub(crate) memory_policy: MemoryRecallPolicy,
    pub(crate) memory_query: String,
    pub(crate) retrieve_workspace: bool,
}

impl KnowledgeDecision {
    pub(crate) fn none() -> Self {
        Self {
            memory_policy: MemoryRecallPolicy::None,
            memory_query: String::new(),
            retrieve_workspace: false,
        }
    }

    pub(crate) fn memory_enabled(&self) -> bool {
        self.memory_policy != MemoryRecallPolicy::None
    }

    /// The workspace retrieval plan implied by this decision: all four retrieval
    /// channels over the bounded query when workspace retrieval is enabled with a
    /// focused query; otherwise no retrieval.
    pub(crate) fn workspace_plan(&self) -> Option<WorkspaceRetrievalPlan> {
        if !self.retrieve_workspace || self.memory_query.trim().is_empty() {
            return None;
        }
        Some(WorkspaceRetrievalPlan {
            query: self.memory_query.clone(),
            channels: [
                WorkspaceRetrievalChannel::Semantic,
                WorkspaceRetrievalChannel::FileSearch,
                WorkspaceRetrievalChannel::GraphDirect,
                WorkspaceRetrievalChannel::GraphWalk,
            ]
            .into_iter()
            .collect(),
            max_results: WORKSPACE_RETRIEVAL_MAX_RESULTS,
        })
    }
}

/// Build the effort-tier plan. `primary_model` is the tier-selected actor model
/// (see `effort_tier_model`); the planner itself never consults a conductor.
pub(crate) fn plan_effort_run(
    effort_label: &str,
    primary_model: String,
    prompt: &str,
) -> EffortRunPlan {
    let effort_label = normalize_effort_label(effort_label);
    let knowledge = knowledge_decision_for_effort(&effort_label, prompt);
    EffortRunPlan {
        effort_label,
        primary_model,
        collaboration_policy: "single".to_string(),
        task_class: "general".to_string(),
        tool_requirement: "none".to_string(),
        vision_required: false,
        knowledge,
    }
}

/// Effort-tier knowledge defaults: Fast answers directly without durable memory or
/// workspace retrieval, while Auto/Pro recall relevant durable memory keyed on the
/// bounded run prompt and retrieve workspace context.
pub(crate) fn knowledge_decision_for_effort(
    effort_label: &str,
    prompt: &str,
) -> KnowledgeDecision {
    if normalize_effort_label(effort_label) == "fast" {
        KnowledgeDecision::none()
    } else {
        KnowledgeDecision {
            memory_policy: MemoryRecallPolicy::Relevant,
            memory_query: bounded_memory_query(prompt),
            retrieve_workspace: true,
        }
    }
}

impl EffortRunPlan {
    /// Stable identity of the plan: the SHA-256 of its canonical JSON. It plays the
    /// role the execution-plan semantic digest played under the orchestrator, so the
    /// strategy receipt and lifecycle bindings keep a plan hash to bind to.
    pub(crate) fn plan_digest(&self) -> Result<String, String> {
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("effort plan serialization failed: {error}"))
    }

    /// Applies the preparation-computed route requirements onto the deterministic
    /// plan: the tool requirement is lifted to the runtime minimum, the prompt
    /// effect authority is enforced fail-closed, and an active image input forces
    /// vision. Mirrors the former direct-route application/validation.
    pub(crate) fn apply_route_requirements(
        &mut self,
        requirements: AgentRouteRequirements,
    ) -> Result<(), String> {
        fn requirement_rank(value: &str) -> u8 {
            match value {
                "effects" => 2,
                "read_only" => 1,
                _ => 0,
            }
        }
        let minimum = requirements.minimum_tool_requirement.label();
        if requirement_rank(&self.tool_requirement) < requirement_rank(minimum) {
            self.tool_requirement = minimum.to_string();
        }
        if requirements.effect_authority == AgentEffectAuthority::Forbidden
            && requirement_rank(&self.tool_requirement) == 2
        {
            return Err("effort plan exceeds the prompt effect authority".to_string());
        }
        if requirements.effect_authority == AgentEffectAuthority::Required
            && requirement_rank(&self.tool_requirement) < 2
        {
            return Err("effort plan omitted required effect authority".to_string());
        }
        if requirements.image_input_required {
            self.vision_required = true;
        }
        Ok(())
    }

    /// Compatibility projection of the effort plan onto the legacy run-decision
    /// shape so readers that still parse `run_decision` (semantic-memory curation,
    /// permission restore, evaluation receipts) observe the effort-tier facts.
    pub(crate) fn run_decision(&self) -> AgentRunDecision {
        let mut decision = AgentRunDecision::direct(self.primary_model.clone());
        decision.tool_requirement = match self.tool_requirement.as_str() {
            "effects" => orchestrator::AgentToolRequirement::Effects,
            "read_only" => orchestrator::AgentToolRequirement::ReadOnly,
            _ => orchestrator::AgentToolRequirement::None,
        };
        decision.vision_required = self.vision_required;
        decision.memory = MemoryRecallPlan {
            policy: self.knowledge.memory_policy,
            query: self.knowledge.memory_query.clone(),
        };
        decision.retrieval = self
            .knowledge
            .workspace_plan()
            .unwrap_or_else(WorkspaceRetrievalPlan::none);
        decision
    }

    /// The policy the run requested by effort tier: Fast runs single, Auto/Pro
    /// historically requested the router and now resolve to the same single lane.
    pub(crate) fn requested_policy_label(&self) -> &'static str {
        if self.effort_label == "fast" {
            "single"
        } else {
            "auto_router"
        }
    }
}

/// The minimal single-model execution contract for an effort tier. It keeps the
/// loop's `conductor_contract` reader working without a conductor: one actor, no
/// branches, and post-mutation verification required exactly on the verified-answer
/// tiers (Auto/Pro).
pub(crate) fn effort_execution_contract(effort_label: &str) -> ConductorExecutionContract {
    let effort = normalize_effort_label(effort_label);
    let terminal_model_call_reserve = match effort.as_str() {
        "fast" => 1,
        "pro" => 3,
        _ => 2,
    };
    ConductorExecutionContract {
        task_class: TaskClass::General,
        effort,
        policy: OrchestrationPolicy::Single,
        expected_uplift_bps: 0,
        confidence_bps: 0,
        max_parallelism: 1,
        max_workflow_steps: 1,
        min_successful_branches: 1,
        verification_required: normalize_effort_label(effort_label) != "fast",
        terminal_model_call_reserve,
        stop_policy: ConductorStopPolicy::FirstVerified,
        fallback_policy: ConductorFallbackPolicy::SinglePath,
        min_team_uplift_bps: 0,
        min_distinct_contributions: 0,
        requires_synthesis: false,
    }
}

/// Write the plan's scheduling facts into the run context using the same keys the
/// loop and contract read, replacing the orchestrator writes key-for-key.
pub(crate) fn apply_effort_plan_keys(
    plan: &EffortRunPlan,
    run_context: &mut Metadata,
) -> Result<(), String> {
    run_context.insert("agent_model".to_string(), plan.primary_model.clone());
    run_context.insert("agent_effort".to_string(), plan.effort_label.clone());
    run_context.insert(
        "collaboration_policy".to_string(),
        plan.collaboration_policy.clone(),
    );
    run_context.insert("task_class".to_string(), plan.task_class.clone());
    run_context.insert(
        "tool_requirement".to_string(),
        plan.tool_requirement.clone(),
    );
    run_context.insert(
        "vision_required".to_string(),
        plan.vision_required.to_string(),
    );
    run_context.insert(
        "conductor_contract".to_string(),
        effort_execution_contract(&plan.effort_label).to_json()?,
    );
    run_context.insert(
        "requested_policy".to_string(),
        plan.requested_policy_label().to_string(),
    );
    run_context.insert(
        "run_decision".to_string(),
        serde_json::to_string(&plan.run_decision())
            .map_err(|error| format!("run decision serialization failed: {error}"))?,
    );
    run_context.insert(
        "execution_plan_semantic_sha256".to_string(),
        plan.plan_digest()?,
    );
    Ok(())
}

fn bounded_memory_query(prompt: &str) -> String {
    prompt.chars().take(MAX_RUN_DECISION_QUERY_CHARS).collect()
}

fn normalize_effort_label(effort_label: &str) -> String {
    match effort_label.trim().to_ascii_lowercase().as_str() {
        "fast" => "fast".to_string(),
        "pro" => "pro".to_string(),
        _ => "auto".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_plan_is_single_model_and_general_task() {
        let plan = plan_effort_run("auto", "qwen3.7-plus".to_string(), "fix the login bug");
        assert_eq!(plan.effort_label, "auto");
        assert_eq!(plan.primary_model, "qwen3.7-plus");
        assert_eq!(plan.collaboration_policy, "single");
        assert_eq!(plan.task_class, "general");
        assert_eq!(plan.tool_requirement, "none");
        assert!(!plan.vision_required);
    }

    #[test]
    fn apply_writes_the_keys_the_loop_reads() {
        let plan = plan_effort_run("pro", "qwen3.7-max".to_string(), "deep mission");
        let mut run_context = Metadata::new();
        apply_effort_plan_keys(&plan, &mut run_context).expect("plan keys should apply");

        assert_eq!(
            run_context.get("agent_model").map(String::as_str),
            Some("qwen3.7-max")
        );
        assert_eq!(run_context.get("agent_effort").map(String::as_str), Some("pro"));
        assert_eq!(
            run_context.get("collaboration_policy").map(String::as_str),
            Some("single")
        );
        assert_eq!(run_context.get("task_class").map(String::as_str), Some("general"));
        assert_eq!(
            run_context.get("tool_requirement").map(String::as_str),
            Some("none")
        );
        assert_eq!(
            run_context.get("vision_required").map(String::as_str),
            Some("false")
        );
        let contract = run_context
            .get("conductor_contract")
            .expect("conductor_contract should be written");
        let parsed = ConductorExecutionContract::from_json(contract)
            .expect("the loop reader must parse the effort contract");
        assert_eq!(parsed.policy, OrchestrationPolicy::Single);
        assert_eq!(parsed.max_parallelism, 1);
        assert!(parsed.verification_required);
    }

    #[test]
    fn fast_knowledge_decision_skips_memory_and_retrieval() {
        let decision = knowledge_decision_for_effort("fast", "quick question");
        assert_eq!(decision, KnowledgeDecision::none());
        assert!(!decision.memory_enabled());
        assert!(!decision.retrieve_workspace);
    }

    #[test]
    fn auto_and_pro_recall_relevant_memory_and_retrieve_workspace() {
        for effort in ["auto", "pro"] {
            let decision = knowledge_decision_for_effort(effort, "Why does login expire early?");
            assert_eq!(decision.memory_policy, MemoryRecallPolicy::Relevant);
            assert_eq!(decision.memory_query, "Why does login expire early?");
            assert!(decision.retrieve_workspace);
        }
    }

    #[test]
    fn memory_query_stays_inside_the_recall_budget() {
        let long_prompt = "x".repeat(MAX_RUN_DECISION_QUERY_CHARS + 500);
        let decision = knowledge_decision_for_effort("auto", &long_prompt);
        assert_eq!(decision.memory_query.chars().count(), MAX_RUN_DECISION_QUERY_CHARS);
    }

    #[test]
    fn effort_contract_verification_follows_the_tier() {
        let fast = effort_execution_contract("fast");
        assert!(!fast.verification_required);
        assert_eq!(fast.terminal_model_call_reserve, 1);

        let auto = effort_execution_contract("auto");
        assert!(auto.verification_required);
        assert_eq!(auto.terminal_model_call_reserve, 2);

        let pro = effort_execution_contract("PRO");
        assert!(pro.verification_required);
        assert_eq!(pro.effort, "pro");
        assert_eq!(pro.terminal_model_call_reserve, 3);
    }

    #[test]
    fn workspace_plan_enables_all_channels_over_the_bounded_query() {
        let knowledge = knowledge_decision_for_effort("auto", "find the session bug");
        let plan = knowledge
            .workspace_plan()
            .expect("auto retrieves workspace context");
        assert_eq!(plan.query, "find the session bug");
        assert_eq!(plan.max_results, WORKSPACE_RETRIEVAL_MAX_RESULTS);
        assert_eq!(plan.channels.len(), 4);
        assert!(plan.enabled());
    }

    #[test]
    fn workspace_plan_stays_none_without_retrieval_or_query() {
        assert_eq!(KnowledgeDecision::none().workspace_plan(), None);
        let blank_query = KnowledgeDecision {
            memory_policy: MemoryRecallPolicy::Relevant,
            memory_query: "   ".to_string(),
            retrieve_workspace: true,
        };
        assert_eq!(blank_query.workspace_plan(), None);
    }

    #[test]
    fn route_requirements_lift_tool_requirement_and_force_vision() {
        let mut plan = plan_effort_run("auto", "model".to_string(), "prompt");
        assert_eq!(plan.tool_requirement, "none");
        assert!(!plan.vision_required);

        plan.apply_route_requirements(AgentRouteRequirements {
            minimum_tool_requirement: orchestrator::AgentToolRequirement::Effects,
            effect_authority: AgentEffectAuthority::Required,
            image_input_required: true,
        })
        .expect("requirements apply");

        assert_eq!(plan.tool_requirement, "effects");
        assert!(plan.vision_required);
    }

    #[test]
    fn forbidden_effect_authority_rejects_an_effects_plan() {
        let mut plan = plan_effort_run("auto", "model".to_string(), "prompt");
        plan.tool_requirement = "effects".to_string();
        let error = plan
            .apply_route_requirements(AgentRouteRequirements {
                minimum_tool_requirement: orchestrator::AgentToolRequirement::None,
                effect_authority: AgentEffectAuthority::Forbidden,
                image_input_required: false,
            })
            .expect_err("forbidden authority must reject effects");
        assert!(error.contains("effect authority"));
    }

    #[test]
    fn required_effect_authority_rejects_a_non_effects_plan() {
        let mut plan = plan_effort_run("auto", "model".to_string(), "prompt");
        let error = plan
            .apply_route_requirements(AgentRouteRequirements {
                minimum_tool_requirement: orchestrator::AgentToolRequirement::None,
                effect_authority: AgentEffectAuthority::Required,
                image_input_required: false,
            })
            .expect_err("required authority must demand effects");
        assert!(error.contains("effect authority"));
    }

    #[test]
    fn run_decision_projects_the_effort_knowledge_facts() {
        let plan = plan_effort_run("pro", "qwen3.7-max".to_string(), "deep mission");
        let decision = plan.run_decision();
        assert_eq!(decision.primary_model, "qwen3.7-max");
        assert_eq!(decision.memory.policy, MemoryRecallPolicy::Relevant);
        assert_eq!(decision.memory.query, "deep mission");
        assert!(decision.retrieval.enabled());

        let fast = plan_effort_run("fast", "qwen3.7-flash".to_string(), "quick answer");
        let fast_decision = fast.run_decision();
        assert_eq!(fast_decision.memory.policy, MemoryRecallPolicy::None);
        assert!(!fast_decision.retrieval.enabled());
    }

    #[test]
    fn plan_digest_is_a_stable_sha256() {
        let plan = plan_effort_run("auto", "model".to_string(), "prompt");
        let digest = plan.plan_digest().expect("plan digest computes");
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(digest, plan.plan_digest().expect("digest is deterministic"));
    }

    #[test]
    fn requested_policy_follows_the_effort_tier() {
        assert_eq!(
            plan_effort_run("fast", "m".to_string(), "p").requested_policy_label(),
            "single"
        );
        assert_eq!(
            plan_effort_run("auto", "m".to_string(), "p").requested_policy_label(),
            "auto_router"
        );
        assert_eq!(
            plan_effort_run("pro", "m".to_string(), "p").requested_policy_label(),
            "auto_router"
        );
    }
}
