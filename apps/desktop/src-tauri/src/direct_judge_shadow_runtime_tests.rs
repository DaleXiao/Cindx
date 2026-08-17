use super::*;
use crate::direct_judge_shadow_runtime::{
    admit_direct_judge_shadow_fitness, load_direct_judge_shadow_signals,
    project_direct_judge_shadow_signal, record_direct_judge_shadow_fitness_to,
    DIRECT_JUDGE_SHADOW_JOURNAL_CAPACITY,
};
use agent_core::TaskId;

fn shadow_runtime() -> agent_runtime::AgentLoopState {
    agent_runtime::start_agent_loop(
        TaskId("direct-judge-shadow-test".to_string()),
        "answer carefully",
        agent_runtime::AgentRuntimeConfig::default(),
    )
}

fn shadow_runtime_with_verified_mutation() -> agent_runtime::AgentLoopState {
    let mut state = shadow_runtime();
    state.task_contract.merge_workspace_verification_policy(
        agent_runtime::WorkspaceVerificationPolicy::RequiredAfterMutation,
    );
    agent_runtime::record_tool_outcome_with_risk(
        &mut state,
        "file.write",
        r#"{"path":"report.md"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::WritesWorkspace),
    );
    agent_runtime::record_tool_outcome_with_risk(
        &mut state,
        "file.read",
        r#"{"path":"report.md"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ReadOnly),
    );
    state
}

fn shadow_journal_path() -> PathBuf {
    std::env::temp_dir()
        .join(unique_id("direct-judge-shadow"))
        .join("journal.jsonl")
}

#[test]
fn shadow_fitness_projects_verified_pass_to_full_reward() {
    let state = shadow_runtime_with_verified_mutation();
    let signal =
        project_direct_judge_shadow_signal(&state, "direct_judge_passed", "postcondition_verified")
            .expect("projection");
    assert_eq!(signal.reward_bps, Some(10_000));
    assert_eq!(
        signal.mutation_verification,
        agent_application::DirectJudgeMutationVerificationV1::VerifiedAfterLastMutation
    );
    assert!(signal.workspace_verification_required);
}

#[test]
fn shadow_fitness_penalizes_unverified_mutation_under_required_policy() {
    let mut state = shadow_runtime_with_verified_mutation();
    agent_runtime::record_tool_outcome_with_risk(
        &mut state,
        "file.write",
        r#"{"path":"second.md"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::WritesWorkspace),
    );
    let signal =
        project_direct_judge_shadow_signal(&state, "direct_judge_passed", "evidence_visible")
            .expect("projection");
    assert_eq!(signal.reward_bps, Some(5_000));
    assert_eq!(
        signal.mutation_verification,
        agent_application::DirectJudgeMutationVerificationV1::UnverifiedAfterMutation
    );
}

#[test]
fn shadow_fitness_censors_unjudged_completions_but_records_them() {
    let state = shadow_runtime();
    let path = shadow_journal_path();
    let signal = record_direct_judge_shadow_fitness_to(
        &path,
        &state,
        "direct_judge_not_eligible",
        "self_contained",
    )
    .expect("censored recording");
    assert!(signal.censored());
    let signals = load_direct_judge_shadow_signals(&path).expect("journal loads");
    assert_eq!(signals.len(), 1);
    let summary = agent_application::summarize_direct_judge_fitness(&signals).expect("summary");
    assert_eq!(summary.censored_runs, 1);
    assert_eq!(summary.scored_runs, 0);
    assert_eq!(summary.average_reward_bps, None);
    assert!(!summary.promotion_eligible);
}

#[test]
fn shadow_fitness_fails_closed_on_unknown_disposition() {
    let state = shadow_runtime();
    let path = shadow_journal_path();
    let error = record_direct_judge_shadow_fitness_to(
        &path,
        &state,
        "direct_judge_bogus",
        "self_contained",
    )
    .expect_err("unknown disposition must fail closed");
    assert!(error.contains("unknown"));
    assert!(!path.exists());
}

#[test]
fn shadow_fitness_journal_bounds_growth_and_reloads_for_summary() {
    let path = shadow_journal_path();
    for index in 0..(DIRECT_JUDGE_SHADOW_JOURNAL_CAPACITY + 5) {
        let mut state = shadow_runtime_with_verified_mutation();
        state.task_id = TaskId(format!("direct-judge-shadow-{index}"));
        record_direct_judge_shadow_fitness_to(
            &path,
            &state,
            "direct_judge_passed",
            "postcondition_verified",
        )
        .expect("append");
    }
    let signals = load_direct_judge_shadow_signals(&path).expect("journal loads");
    assert_eq!(signals.len(), DIRECT_JUDGE_SHADOW_JOURNAL_CAPACITY);
    let summary = agent_application::summarize_direct_judge_fitness(&signals).expect("summary");
    assert_eq!(
        summary.window,
        agent_application::DIRECT_JUDGE_FITNESS_WINDOW
    );
    assert_eq!(
        summary.scored_runs,
        agent_application::DIRECT_JUDGE_FITNESS_WINDOW
    );
    assert_eq!(summary.average_reward_bps, Some(10_000));
    assert!(!summary.promotion_eligible);
}

fn admit_window(path: &std::path::Path) -> Vec<agent_application::DirectJudgeFitnessSignalV1> {
    for index in 0..2 {
        let mut state = shadow_runtime_with_verified_mutation();
        state.task_id = TaskId(format!("admit-pass-{index}"));
        record_direct_judge_shadow_fitness_to(
            path,
            &state,
            "direct_judge_passed",
            "postcondition_verified",
        )
        .expect("append pass");
    }
    let mut state = shadow_runtime_with_verified_mutation();
    state.task_id = TaskId("admit-censored".to_string());
    record_direct_judge_shadow_fitness_to(
        path,
        &state,
        "direct_judge_not_applicable",
        "self_contained",
    )
    .expect("append censored");
    load_direct_judge_shadow_signals(path).expect("journal loads")
}

#[test]
fn shadow_fitness_admission_admits_only_a_bound_approving_window() {
    let path = shadow_journal_path();
    let signals = admit_window(&path);
    let digest = agent_application::direct_judge_fitness_window_digest(&signals).expect("digest");
    let receipt = agent_application::DirectJudgeReviewReceiptV1::new(
        "a".repeat(64),
        digest,
        signals.len(),
        true,
    )
    .expect("receipt");
    let (admission, fitness) =
        admit_direct_judge_shadow_fitness(&path, &receipt.to_json().expect("receipt json"))
            .expect("admits");
    assert_eq!(admission.window_size, 3);
    assert_eq!(admission.scored_runs, 2);
    assert_eq!(admission.censored_runs, 1);
    assert_eq!(admission.average_reward_bps, 10_000);
    assert!(!admission.production_eligible);
    assert!(!admission.promotion_eligible);
    assert_eq!(fitness.runs, 2);
    assert_eq!(fitness.average_reward, 1.0);
    assert_eq!(fitness.success_rate, 1.0);
    assert_eq!(fitness.paired_runs, 0);
    assert_eq!(fitness.execution_runs, 0);
    assert_eq!(fitness.safety_violations, 0);
}

#[test]
fn shadow_fitness_admission_fails_closed_without_approval_or_binding() {
    let path = shadow_journal_path();
    let signals = admit_window(&path);
    let digest = agent_application::direct_judge_fitness_window_digest(&signals).expect("digest");

    let rejecting = agent_application::DirectJudgeReviewReceiptV1::new(
        "a".repeat(64),
        digest.clone(),
        signals.len(),
        false,
    )
    .expect("receipt");
    let error = admit_direct_judge_shadow_fitness(&path, &rejecting.to_json().expect("json"))
        .expect_err("rejecting receipt must not admit");
    assert!(error.contains("does not admit"));

    let subset_digest =
        agent_application::direct_judge_fitness_window_digest(&signals[..2]).expect("digest");
    let foreign =
        agent_application::DirectJudgeReviewReceiptV1::new("a".repeat(64), subset_digest, 2, true)
            .expect("receipt");
    let error = admit_direct_judge_shadow_fitness(&path, &foreign.to_json().expect("json"))
        .expect_err("foreign window must not admit");
    assert!(error.contains("does not bind"));

    assert!(admit_direct_judge_shadow_fitness(&path, "not json").is_err());
}

use crate::direct_judge_shadow_runtime::direct_judge_shadow_journal_path_for;
use crate::prompt_evolution_admission_runtime::{
    admitted_direct_judge_fitness_state, load_prompt_evolution_admission_config_from,
    prompt_evolution_admission_config_path_for,
};

fn admission_root() -> PathBuf {
    std::env::temp_dir().join(unique_id("prompt-evolution-admission"))
}

fn config_json(enabled: bool) -> String {
    format!(
        r#"{{"admittedDirectJudgeFitness":{{"enabled":{enabled}}}}}"#,
        enabled = enabled
    )
}

fn seed_admitted_window(root: &Path) {
    let journal = direct_judge_shadow_journal_path_for(root);
    for index in 0..2 {
        let mut state = agent_runtime::start_agent_loop(
            TaskId(format!("admission-runtime-{index}")),
            "answer carefully",
            agent_runtime::AgentRuntimeConfig::default(),
        );
        state.task_contract.merge_workspace_verification_policy(
            agent_runtime::WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        agent_runtime::record_tool_outcome_with_risk(
            &mut state,
            "file.write",
            r#"{"path":"report.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        agent_runtime::record_tool_outcome_with_risk(
            &mut state,
            "file.read",
            r#"{"path":"report.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        record_direct_judge_shadow_fitness_to(
            &journal,
            &state,
            "direct_judge_passed",
            "postcondition_verified",
        )
        .expect("append");
    }
    let signals = load_direct_judge_shadow_signals(&journal).expect("journal loads");
    let digest = agent_application::direct_judge_fitness_window_digest(&signals).expect("digest");
    let receipt = agent_application::DirectJudgeReviewReceiptV1::new(
        "a".repeat(64),
        digest,
        signals.len(),
        true,
    )
    .expect("receipt");
    let receipt_path = root.join("prompt-evolution/direct-judge-review-receipt.json");
    fs::create_dir_all(receipt_path.parent().expect("parent")).expect("create receipt dir");
    fs::write(&receipt_path, receipt.to_json().expect("receipt json")).expect("write receipt");
}

#[test]
fn admission_runtime_defaults_off_without_config() {
    let root = admission_root();
    assert!(admitted_direct_judge_fitness_state(&root).is_none());
}

#[test]
fn admission_runtime_stays_off_until_config_enables_it() {
    let root = admission_root();
    seed_admitted_window(&root);
    assert!(admitted_direct_judge_fitness_state(&root).is_none());

    let config_path = prompt_evolution_admission_config_path_for(&root);
    fs::write(&config_path, config_json(false)).expect("write config");
    assert!(admitted_direct_judge_fitness_state(&root).is_none());

    fs::write(&config_path, config_json(true)).expect("write config");
    let config = load_prompt_evolution_admission_config_from(&config_path);
    assert!(config.admitted_direct_judge_fitness.enabled);
}

#[test]
fn admission_runtime_admits_an_approving_bound_window() {
    let root = admission_root();
    seed_admitted_window(&root);
    fs::write(
        prompt_evolution_admission_config_path_for(&root),
        config_json(true),
    )
    .expect("write config");
    let state = admitted_direct_judge_fitness_state(&root).expect("admits");
    assert_eq!(state.window_size, 2);
    assert_eq!(state.scored_runs, 2);
    assert_eq!(state.average_reward_bps, 10_000);
    assert_eq!(state.fitness.runs, 2);
    assert_eq!(state.fitness.average_reward, 1.0);
    assert_eq!(state.fitness.paired_runs, 0);
    assert_eq!(state.fitness.execution_runs, 0);
    assert_eq!(state.reviewer_identity_sha256, "a".repeat(64));
    assert!(!state.admission_sha256.is_empty());
}

#[test]
fn admission_runtime_fails_closed_on_missing_or_tampered_inputs() {
    let root = admission_root();
    seed_admitted_window(&root);
    fs::write(
        prompt_evolution_admission_config_path_for(&root),
        config_json(true),
    )
    .expect("write config");
    assert!(admitted_direct_judge_fitness_state(&root).is_some());

    let receipt_path = root.join("prompt-evolution/direct-judge-review-receipt.json");
    let mut tampered: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&receipt_path).expect("read")).expect("value");
    tampered["approve"] = serde_json::json!(false);
    fs::write(
        &receipt_path,
        serde_json::to_string(&tampered).expect("encode"),
    )
    .expect("write tampered receipt");
    assert!(admitted_direct_judge_fitness_state(&root).is_none());

    let journal = direct_judge_shadow_journal_path_for(&root);
    fs::remove_file(&journal).expect("remove journal");
    fs::write(&receipt_path, "not json").expect("write junk");
    assert!(admitted_direct_judge_fitness_state(&root).is_none());
}

#[test]
fn admission_runtime_config_loader_defaults_on_corrupt_json() {
    let root = admission_root();
    let config_path = prompt_evolution_admission_config_path_for(&root);
    fs::create_dir_all(root.clone()).expect("create root");
    fs::write(&config_path, "{ not json").expect("write corrupt config");
    let config = load_prompt_evolution_admission_config_from(&config_path);
    assert!(!config.admitted_direct_judge_fitness.enabled);
    assert!(admitted_direct_judge_fitness_state(&root).is_none());
}
