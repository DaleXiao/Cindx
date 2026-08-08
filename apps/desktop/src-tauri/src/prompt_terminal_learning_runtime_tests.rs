use super::*;

fn assigned_terminal_context(effort: &str) -> Metadata {
    let mut context = [
        ("agent_run_id".to_string(), "run-terminal".to_string()),
        (
            "logical_agent_run_id".to_string(),
            "logical-terminal".to_string(),
        ),
        ("agent_effort".to_string(), effort.to_string()),
        (
            "collaboration_policy".to_string(),
            "auto_router".to_string(),
        ),
        ("project_id".to_string(), "project-terminal".to_string()),
        (
            "project_root".to_string(),
            "/tmp/project-terminal".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let selection = crate::prompt_profile_serving::seed_prompt_profile_selection(
        effort,
        crate::prompt_profile_serving::PromptProfileFallback::NoDeployment,
        &context,
    );
    let (receipt, receipt_sha256) = selection.receipt_json_and_sha256().unwrap();
    context.insert("prompt_profile".to_string(), selection.genome.id.clone());
    context.insert(
        "prompt_genome".to_string(),
        serde_json::to_string(&selection.genome).unwrap(),
    );
    context.insert(
        "prompt_profile_source".to_string(),
        selection.source_label(),
    );
    context.insert("prompt_profile_assignment_receipt".to_string(), receipt);
    context.insert(
        "prompt_profile_assignment_sha256".to_string(),
        receipt_sha256,
    );
    context
}

fn ready_evolution_config() -> ProviderConfig {
    ProviderConfig {
        api_key: "test-key".to_string(),
        prompt_evolution_enabled: true,
        planner_model: "planner".to_string(),
        executor_model: "executor".to_string(),
        reviewer_model: "reviewer".to_string(),
        ..ProviderConfig::default()
    }
}

#[test]
fn trusted_auto_terminal_bootstraps_prompt_evaluation() {
    let context = assigned_terminal_context("auto");
    let schedule = terminal_prompt_evaluation_schedule(&ready_evolution_config(), &context)
        .unwrap()
        .expect("trusted Auto terminal should schedule learning");

    assert_eq!(schedule.effort, "auto");
    assert_eq!(schedule.policy, "auto_router");
    assert_eq!(schedule.agent_budget, AgentPolicy::Auto.max_parallelism());
    assert_eq!(schedule.worker_models, vec!["planner", "executor"]);
    assert_eq!(
        schedule.current_profile.id,
        ConductorPromptGenome::seed_for_effort("auto").id
    );
}

#[test]
fn terminal_learning_stays_disabled_for_fast_or_unready_configuration() {
    let config = ready_evolution_config();
    assert!(
        terminal_prompt_evaluation_schedule(&config, &assigned_terminal_context("fast"))
            .unwrap()
            .is_none()
    );

    let mut disabled = config.clone();
    disabled.prompt_evolution_enabled = false;
    assert!(
        terminal_prompt_evaluation_schedule(&disabled, &assigned_terminal_context("auto"))
            .unwrap()
            .is_none()
    );

    let mut unready = config;
    unready.api_key.clear();
    assert!(
        terminal_prompt_evaluation_schedule(&unready, &assigned_terminal_context("auto"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn terminal_learning_rejects_tampered_profile_assignment() {
    let mut context = assigned_terminal_context("pro");
    context.insert("prompt_profile".to_string(), "tampered".to_string());
    let error = terminal_prompt_evaluation_schedule(&ready_evolution_config(), &context)
        .expect_err("tampered assignment must fail closed");
    assert!(error.contains("assignment identity"));
}
