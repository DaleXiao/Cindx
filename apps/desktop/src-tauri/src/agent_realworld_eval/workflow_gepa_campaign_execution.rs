use super::execution::{execute_case, CaseExecutionInput};
use super::workflow_gepa_campaign_contract::{
    CampaignSplit, ProductPairReceipt, ProductRunReceipt, CAMPAIGN_PROJECT_ID,
};
use super::{materialize_case, ExecutionCell, RawRun, RealworldCase, Treatment};
use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use crate::prompt_learning_runtime::{
    prompt_evaluation_parent_budget, prompt_workspace_content_sha256,
};
use agent_runtime::AgentRunControl;
use orchestrator::{sha256_hex, FrozenPromptProfileSnapshot};
use std::ffi::OsString;
use std::path::Path;

pub(super) struct EvaluationDataEnvironment {
    prior_requested_root: Option<OsString>,
    prior_active_root: Option<OsString>,
}

struct EvaluationProfileEnvironment {
    prior_path: Option<OsString>,
    prior_sha256: Option<OsString>,
}

impl EvaluationDataEnvironment {
    pub(super) fn install(root: &Path) -> Self {
        let prior_requested_root = std::env::var_os("CINDX_AGENT_REALWORLD_DATA_DIR");
        let prior_active_root = std::env::var_os("CINDX_DATA_DIR");
        std::env::set_var("CINDX_AGENT_REALWORLD_DATA_DIR", root.join("data"));
        Self {
            prior_requested_root,
            prior_active_root,
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
    case: &RealworldCase,
    split: CampaignSplit,
    replicate: u32,
    pair_index: usize,
    execution_index: &mut usize,
    suite_sha256: &str,
    snapshot_path: &Path,
    snapshot_artifact_sha256: &str,
    snapshot: &FrozenPromptProfileSnapshot,
) -> Result<ProductPairReceipt, String> {
    let seed_root = suite_root.join(format!(
        "{}-{}-r{replicate}-seed",
        split.label(),
        case.id
    ));
    let candidate_root = suite_root.join(format!(
        "{}-{}-r{replicate}-candidate",
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
    let candidate_first = pair_index % 2 == 1;
    let order = if candidate_first {
        "candidate_then_seed"
    } else {
        "seed_then_candidate"
    };
    let seed_scope = project_scope(split, case, replicate, "seed");
    let candidate_scope = project_scope(split, case, replicate, "candidate");
    let run_seed = |index: usize, position: usize| {
        execute_campaign_case(
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
        )
    };
    let run_candidate = |index: usize, position: usize| {
        let _profile_environment =
            EvaluationProfileEnvironment::install(snapshot_path, snapshot_artifact_sha256);
        execute_campaign_case(
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
        )
    };
    let (seed, candidate) = if candidate_first {
        let candidate = run_candidate(*execution_index, 1);
        *execution_index = execution_index.saturating_add(1);
        let seed = run_seed(*execution_index, 2);
        *execution_index = execution_index.saturating_add(1);
        (seed, candidate)
    } else {
        let seed = run_seed(*execution_index, 1);
        *execution_index = execution_index.saturating_add(1);
        let candidate = run_candidate(*execution_index, 2);
        *execution_index = execution_index.saturating_add(1);
        (seed, candidate)
    };
    let seed_receipt = ProductRunReceipt::from_run(&seed, split, replicate)?;
    let candidate_receipt = ProductRunReceipt::from_run(&candidate, split, replicate)?;
    let evaluation_id = format!(
        "product-pair-{}",
        &sha256_hex(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                suite_sha256,
                case.id,
                split.label(),
                replicate,
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
        },
    )
}

pub(super) fn campaign_workspace_sha256(root: &Path) -> Result<String, String> {
    let control = AgentRunControl::with_budget(prompt_evaluation_parent_budget());
    prompt_workspace_content_sha256(root, &control)
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
}
