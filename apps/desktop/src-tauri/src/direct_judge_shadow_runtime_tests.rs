use super::*;
use crate::direct_judge_shadow_runtime::{
    load_direct_judge_shadow_signals, project_direct_judge_shadow_signal,
    record_direct_judge_shadow_fitness_to, DIRECT_JUDGE_SHADOW_JOURNAL_CAPACITY,
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

