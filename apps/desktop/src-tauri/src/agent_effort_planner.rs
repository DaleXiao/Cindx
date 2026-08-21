use agent_core::Metadata;
use orchestrator::{
    ConductorExecutionContract, ConductorFallbackPolicy, ConductorStopPolicy,
    MemoryRecallPolicy, OrchestrationPolicy, TaskClass, MAX_RUN_DECISION_QUERY_CHARS,
};

/// The minimal, effort-tier run plan that replaces the orchestrator/conductor
/// planning surface. It carries only what the loop and preparation actually need:
/// the effort label, the tier-selected primary model, fixed single-model
/// scheduling facts, and the deterministic knowledge decision. No conductor,
/// routing, or collaboration concepts.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// loop and contract read, so the new planner can later replace the orchestrator
/// writes key-for-key.
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
}
