use super::execution::{execute_case, CaseExecutionInput};
use super::workflow_gepa_campaign_contract::{
    CampaignSplit, ProductPairReceipt, ProductRunReceipt, CAMPAIGN_PROJECT_ID,
};
use super::workflow_gepa_campaign_journal::CampaignJournal;
use super::{
    materialize_case, ExecutionCell, RawRun, RealworldCase, RealworldSuite, Treatment,
};
use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use crate::prompt_learning_runtime::{
    prompt_evaluation_parent_budget, prompt_workspace_content_sha256,
};
use agent_runtime::AgentRunControl;
use agent_runtime::RunBudget;
use orchestrator::{sha256_hex, FrozenPromptProfileSnapshot};
use std::ffi::OsString;
use std::path::Path;

pub(super) struct EvaluationDataEnvironment {
    prior_requested_root: Option<OsString>,
    prior_active_root: Option<OsString>,
    prior_background_memory_setting: Option<OsString>,
}

struct EvaluationProfileEnvironment {
    prior_path: Option<OsString>,
    prior_sha256: Option<OsString>,
}

impl EvaluationDataEnvironment {
    pub(super) fn install(root: &Path) -> Self {
        let prior_requested_root = std::env::var_os("CINDX_AGENT_REALWORLD_DATA_DIR");
        let prior_active_root = std::env::var_os("CINDX_DATA_DIR");
        let prior_background_memory_setting =
            std::env::var_os("CINDX_AGENT_REALWORLD_DISABLE_BACKGROUND_MEMORY");
        std::env::set_var("CINDX_AGENT_REALWORLD_DATA_DIR", root.join("data"));
        std::env::set_var("CINDX_AGENT_REALWORLD_DISABLE_BACKGROUND_MEMORY", "1");
        Self {
            prior_requested_root,
            prior_active_root,
            prior_background_memory_setting,
        }
    }
}

impl Drop for EvaluationDataEnvironment {
    fn drop(&mut self) {
        restore_environment(
            "CINDX_AGENT_REALWORLD_DATA_DIR",
            self.prior_requested_root.take(),
        );
        restore_environment("CINDX_DATA_DIR", self.prior_active_root.take());
        restore_environment(
            "CINDX_AGENT_REALWORLD_DISABLE_BACKGROUND_MEMORY",
            self.prior_background_memory_setting.take(),
        );
    }
}

impl EvaluationProfileEnvironment {
    fn install(path: &Path, artifact_sha256: &str) -> Self {
        let prior_path = std::env::var_os("CINDX_AGENT_REALWORLD_PROFILE_PATH");
        let prior_sha256 = std::env::var_os("CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256");
        std::env::set_var("CINDX_AGENT_REALWORLD_PROFILE_PATH", path);
        std::env::set_var(
            "CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256",
            artifact_sha256,
        );
        Self {
            prior_path,
            prior_sha256,
        }
    }
}

impl Drop for EvaluationProfileEnvironment {
    fn drop(&mut self) {
        restore_environment("CINDX_AGENT_REALWORLD_PROFILE_PATH", self.prior_path.take());
        restore_environment(
            "CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256",
            self.prior_sha256.take(),
        );
    }
}

fn restore_environment(key: &str, value: Option<OsString>) {
    if let Some(value) = value {
        std::env::set_var(key, value);
    } else {
        std::env::remove_var(key);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_product_pair(
    app: &tauri::App<tauri::Wry>,
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    evaluation_database: &Path,
    suite_root: &Path,
    comparison_scope: &str,
    case: &RealworldCase,
    split: CampaignSplit,
    replicate: u32,
    pair_index: usize,
    execution_index: &mut usize,
    suite_sha256: &str,
    snapshot_path: &Path,
    snapshot_artifact_sha256: &str,
    snapshot: &FrozenPromptProfileSnapshot,
    journal: &mut CampaignJournal,
) -> Result<ProductPairReceipt, String> {
    if comparison_scope.is_empty()
        || !comparison_scope
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("matched product pair has an invalid comparison scope".to_string());
    }
    let seed_root = suite_root.join(format!(
        "{}-{}-r{replicate}-{comparison_scope}-seed",
        split.label(),
        case.id
    ));
    let candidate_root = suite_root.join(format!(
        "{}-{}-r{replicate}-{comparison_scope}-candidate",
        split.label(),
        case.id
    ));
    materialize_case(&seed_root, case)?;
    materialize_case(&candidate_root, case)?;
    let seed_prestate = campaign_workspace_sha256(&seed_root)?;
    let candidate_prestate = campaign_workspace_sha256(&candidate_root)?;
    if seed_prestate != candidate_prestate {
        return Err(format!(
            "matched product pair {} did not start from identical workspaces",
            case.id
        ));
    }
    let candidate_first = candidate_executes_first(pair_index, replicate);
    let order = if candidate_first {
        "candidate_then_seed"
    } else {
        "seed_then_candidate"
    };
    let seed_scope = project_scope(
        split,
        case,
        replicate,
        &format!("{comparison_scope}-seed"),
    );
    let candidate_scope = project_scope(
        split,
        case,
        replicate,
        &format!("{comparison_scope}-candidate"),
    );
    let run_seed = |index: usize,
                    position: usize,
                    journal: &mut CampaignJournal|
     -> Result<(RawRun, ProductRunReceipt), String> {
        execute_journaled_campaign_case(
            app,
            state,
            provider,
            evaluation_database,
            &seed_root,
            case,
            index,
            position,
            replicate,
            suite_sha256,
            None,
            Treatment::Pro,
            &seed_scope,
            split,
            &format!("{}-seed", seed_scope),
            journal,
        )
    };
    let run_candidate = |index: usize,
                         position: usize,
                         journal: &mut CampaignJournal|
     -> Result<(RawRun, ProductRunReceipt), String> {
        let _profile_environment =
            EvaluationProfileEnvironment::install(snapshot_path, snapshot_artifact_sha256);
        execute_journaled_campaign_case(
            app,
            state,
            provider,
            evaluation_database,
            &candidate_root,
            case,
            index,
            position,
            replicate,
            suite_sha256,
            Some(snapshot),
            Treatment::Pro,
            &candidate_scope,
            split,
            &format!("{}-candidate", candidate_scope),
            journal,
        )
    };
    let (seed_receipt, candidate_receipt) = if candidate_first {
        let (_, candidate) = run_candidate(*execution_index, 1, journal)?;
        *execution_index = execution_index.saturating_add(1);
        let (_, seed) = run_seed(*execution_index, 2, journal)?;
        *execution_index = execution_index.saturating_add(1);
        (seed, candidate)
    } else {
        let (_, seed) = run_seed(*execution_index, 1, journal)?;
        *execution_index = execution_index.saturating_add(1);
        let (_, candidate) = run_candidate(*execution_index, 2, journal)?;
        *execution_index = execution_index.saturating_add(1);
        (seed, candidate)
    };
    let evaluation_id = format!(
        "product-pair-{}",
        &sha256_hex(
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}",
                suite_sha256,
                case.id,
                split.label(),
                replicate,
                comparison_scope,
                order
            )
            .as_bytes()
        )[..20]
    );
    ProductPairReceipt::new(
        evaluation_id,
        order.to_string(),
        seed_prestate,
        seed_receipt,
        candidate_receipt,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_journaled_campaign_case(
    app: &tauri::App<tauri::Wry>,
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    evaluation_database: &Path,
    root: &Path,
    case: &RealworldCase,
    execution_index: usize,
    treatment_position: usize,
    replicate: u32,
    plan_sha256: &str,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
    treatment: Treatment,
    project_scope: &str,
    split: CampaignSplit,
    action_label: &str,
    journal: &mut CampaignJournal,
) -> Result<(RawRun, ProductRunReceipt), String> {
    journal.begin_product(action_label)?;
    let run = execute_campaign_case(
        app,
        state,
        provider,
        evaluation_database,
        root,
        case,
        execution_index,
        treatment_position,
        replicate,
        plan_sha256,
        frozen_profile,
        treatment,
        project_scope,
    );
    let receipt = ProductRunReceipt::from_run(&run, split, replicate)?;
    journal.complete_product(&receipt)?;
    Ok((run, receipt))
}

fn candidate_executes_first(pair_index: usize, replicate: u32) -> bool {
    (pair_index + replicate.saturating_sub(1) as usize) % 2 == 1
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_campaign_case(
    app: &tauri::App<tauri::Wry>,
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    evaluation_database: &Path,
    root: &Path,
    case: &RealworldCase,
    execution_index: usize,
    treatment_position: usize,
    replicate: u32,
    plan_sha256: &str,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
    treatment: Treatment,
    project_scope: &str,
) -> RawRun {
    let execution = ExecutionCell {
        execution_index,
        treatment_position,
        plan_sha256: plan_sha256.to_string(),
    };
    execute_case(
        app,
        state,
        provider,
        evaluation_database,
        CaseExecutionInput {
            case,
            treatment,
            replicate,
            root,
            execution: &execution,
            frozen_profile,
            project_scope: Some(project_scope),
            run_budget: Some(workflow_gepa_product_budget()),
        },
    )
}

pub(super) fn workflow_gepa_product_budget() -> RunBudget {
    let mut budget = RunBudget::for_effort("fast");
    budget.max_duration = std::time::Duration::from_secs(10 * 60);
    budget.initial_model_calls = 12;
    budget.max_model_calls = 20;
    budget.model_calls_per_extension = 4;
    budget.initial_tool_calls = 24;
    budget.max_tool_calls = 48;
    budget.tool_calls_per_extension = 8;
    budget.initial_agent_turns = 12;
    budget.max_agent_turns = 20;
    budget.agent_turns_per_extension = 4;
    budget.max_repair_attempts = 3;
    budget.terminal_model_call_reserve = 3;
    budget.terminal_time_reserve = std::time::Duration::from_secs(3 * 60);
    budget.max_physical_model_attempts = 80;
    budget.max_total_tokens = 80_u64.saturating_mul(
        agent_runtime::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT,
    );
    budget.terminal_physical_model_attempt_reserve = 12;
    budget.terminal_token_reserve = 12_u64.saturating_mul(
        agent_runtime::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT,
    );
    budget
}

pub(super) fn campaign_workspace_sha256(root: &Path) -> Result<String, String> {
    let control = AgentRunControl::with_budget(prompt_evaluation_parent_budget());
    prompt_workspace_content_sha256(root, &control)
}

pub(super) fn preflight_matched_workspaces(
    suite_root: &Path,
    suite: &RealworldSuite,
) -> Result<(), String> {
    let preflight_root = suite_root.join("matched-workspace-preflight");
    for case in &suite.cases {
        let seed_root = preflight_root.join(format!("{}-seed", case.id));
        let candidate_root = preflight_root.join(format!("{}-candidate", case.id));
        materialize_case(&seed_root, case)?;
        materialize_case(&candidate_root, case)?;
        if campaign_workspace_sha256(&seed_root)? != campaign_workspace_sha256(&candidate_root)? {
            return Err(format!(
                "matched product pair {} did not materialize identical workspace contents",
                case.id
            ));
        }
    }
    Ok(())
}

pub(super) fn project_scope(
    split: CampaignSplit,
    case: &RealworldCase,
    replicate: u32,
    arm: &str,
) -> String {
    format!(
        "{CAMPAIGN_PROJECT_ID}-{}-{}-r{replicate}-{arm}",
        split.label(),
        case.id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn repeated_test_pairs_reverse_arm_order_for_each_case() {
        for first_replicate_index in 0..4 {
            let second_replicate_index = first_replicate_index + 4;
            assert_ne!(
                candidate_executes_first(first_replicate_index, 1),
                candidate_executes_first(second_replicate_index, 2),
            );
        }
    }

    #[test]
    fn campaign_workspace_fingerprint_ignores_root_path_but_detects_content_changes() {
        let seed = tempfile::tempdir().unwrap();
        let candidate = tempfile::tempdir().unwrap();
        fs::write(seed.path().join("fixture.txt"), "same content\n").unwrap();
        fs::write(candidate.path().join("fixture.txt"), "same content\n").unwrap();

        assert_eq!(
            campaign_workspace_sha256(seed.path()).unwrap(),
            campaign_workspace_sha256(candidate.path()).unwrap()
        );

        fs::write(candidate.path().join("fixture.txt"), "changed content\n").unwrap();
        assert_ne!(
            campaign_workspace_sha256(seed.path()).unwrap(),
            campaign_workspace_sha256(candidate.path()).unwrap()
        );
    }

    #[test]
    fn campaign_preflights_every_matched_case_before_provider_work() {
        let suite: RealworldSuite = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/workflow-gepa-v7.json"
        )))
        .unwrap();
        let root = tempfile::tempdir().unwrap();

        preflight_matched_workspaces(root.path(), &suite).unwrap();
    }

    #[test]
    fn campaign_product_budget_is_bounded_below_shipping_pro() {
        let campaign = workflow_gepa_product_budget();
        let shipping_pro = RunBudget::for_effort("pro");

        assert_eq!(campaign.max_model_calls, 20);
        assert_eq!(campaign.max_tool_calls, 48);
        assert_eq!(campaign.max_physical_model_attempts, 80);
        assert!(campaign.max_duration < shipping_pro.max_duration);
        assert!(campaign.max_model_calls < shipping_pro.max_model_calls);
        assert!(campaign.max_tool_calls < shipping_pro.max_tool_calls);
    }
}
