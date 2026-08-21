use agent_core::Metadata;

/// The minimal, effort-tier run plan that replaces the orchestrator/conductor
/// planning surface. It carries only what the loop and preparation actually need:
/// the effort label, the tier-selected primary model, and fixed single-model
/// scheduling facts. No conductor, routing, or collaboration concepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffortRunPlan {
    pub(crate) effort_label: String,
    pub(crate) primary_model: String,
    pub(crate) collaboration_policy: String,
    pub(crate) task_class: String,
}

/// Build the effort-tier plan. `primary_model` is the tier-selected actor model
/// (see `effort_tier_model`); the planner itself never consults a conductor.
pub(crate) fn plan_effort_run(effort_label: &str, primary_model: String) -> EffortRunPlan {
    EffortRunPlan {
        effort_label: effort_label.to_string(),
        primary_model,
        collaboration_policy: "single".to_string(),
        task_class: "general".to_string(),
    }
}

/// Write the plan's scheduling facts into the run context using the same keys the
/// loop and contract read, so the new planner can later replace the orchestrator
/// writes key-for-key.
pub(crate) fn apply_effort_plan_keys(plan: &EffortRunPlan, run_context: &mut Metadata) {
    run_context.insert("agent_model".to_string(), plan.primary_model.clone());
    run_context.insert("agent_effort".to_string(), plan.effort_label.clone());
    run_context.insert(
        "collaboration_policy".to_string(),
        plan.collaboration_policy.clone(),
    );
    run_context.insert("task_class".to_string(), plan.task_class.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_plan_is_single_model_and_general_task() {
        let plan = plan_effort_run("auto", "qwen3.7-plus".to_string());
        assert_eq!(plan.effort_label, "auto");
        assert_eq!(plan.primary_model, "qwen3.7-plus");
        assert_eq!(plan.collaboration_policy, "single");
        assert_eq!(plan.task_class, "general");
    }

    #[test]
    fn apply_writes_the_keys_the_loop_reads() {
        let plan = plan_effort_run("pro", "qwen3.7-max".to_string());
        let mut run_context = Metadata::new();
        apply_effort_plan_keys(&plan, &mut run_context);

        assert_eq!(run_context.get("agent_model").map(String::as_str), Some("qwen3.7-max"));
        assert_eq!(run_context.get("agent_effort").map(String::as_str), Some("pro"));
        assert_eq!(
            run_context.get("collaboration_policy").map(String::as_str),
            Some("single")
        );
        assert_eq!(run_context.get("task_class").map(String::as_str), Some("general"));
    }
}
