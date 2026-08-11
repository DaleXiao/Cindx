use super::*;

#[test]
fn auto_transfer_followup_uses_the_current_pro_contract() {
    let config = ProviderConfig {
        planner_model: "planner".to_string(),
        executor_model: "executor".to_string(),
        reviewer_model: "reviewer".to_string(),
        ..ProviderConfig::default()
    };
    let model = build_prompt_evolution_read_model(&[], 0, 0);

    let (effort, policy, worker_models, agent_budget, profile) =
        prompt_auto_transfer_request(&config, &model).unwrap();

    assert_eq!(effort, "pro");
    assert_eq!(policy, "best_of_n");
    assert_eq!(agent_budget, AgentPolicy::Pro.max_parallelism());
    assert_eq!(worker_models, vec!["planner", "executor"]);
    assert_eq!(profile.id, ConductorPromptGenome::seed_for_effort("pro").id);
    assert!(profile.require_final_synthesis);
}
