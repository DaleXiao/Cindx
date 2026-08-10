use super::execution::{execute_case, CaseExecutionInput};
use super::outcome_shadow::{project_shadow_outcome_pair, ShadowOutcomePairV1};
use super::workflow_gepa_campaign_contract::{
    CampaignSplit, ProductPairReceipt, ProductRunReceipt, CAMPAIGN_PROJECT_ID,
};
use super::workflow_gepa_campaign_journal::CampaignJournal;
use super::{materialize_case, ExecutionCell, RawRun, RealworldCase, RealworldSuite, Treatment};
use crate::agent_execution_constraint::{AgentExecutionConstraint, MatchedRoutePlanAnchor};
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

pub(super) struct MatchedRoutePairRun {
    pub(super) pair: ProductPairReceipt,
    pub(super) direct: RawRun,
    pub(super) workflow: RawRun,
    #[allow(dead_code)]
    pub(super) shadow_outcome_pair: Option<ShadowOutcomePairV1>,
    #[allow(dead_code)]
    pub(super) shadow_outcome_error: Option<String>,
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
    let seed_scope = project_scope(split, case, replicate, &format!("{comparison_scope}-seed"));
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
    execute_journaled_campaign_case_with_constraint(
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
        split,
        action_label,
        None,
        None,
        journal,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_journaled_campaign_case_with_constraint(
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
    execution_constraint: Option<AgentExecutionConstraint>,
    matched_route_plan_anchor: Option<&MatchedRoutePlanAnchor>,
    journal: &mut CampaignJournal,
) -> Result<(RawRun, ProductRunReceipt), String> {
    journal.begin_product(action_label)?;
    let run = execute_campaign_case_with_constraint(
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
        execution_constraint,
        matched_route_plan_anchor,
    );
    let receipt = ProductRunReceipt::from_run(&run, split, replicate)?;
    journal.complete_product(&receipt)?;
    Ok((run, receipt))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_matched_route_pair(
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
    journal: &mut CampaignJournal,
) -> Result<MatchedRoutePairRun, String> {
    let direct_root = suite_root.join(format!(
        "{}-{}-r{replicate}-forced-direct",
        split.label(),
        case.id
    ));
    let workflow_root = suite_root.join(format!(
        "{}-{}-r{replicate}-forced-workflow",
        split.label(),
        case.id
    ));
    materialize_case(&direct_root, case)?;
    materialize_case(&workflow_root, case)?;
    let direct_prestate = campaign_workspace_sha256(&direct_root)?;
    let workflow_prestate = campaign_workspace_sha256(&workflow_root)?;
    if direct_prestate != workflow_prestate {
        return Err(format!(
            "matched route pair {} did not start from identical workspaces",
            case.id
        ));
    }
    let workflow_first = candidate_executes_first(pair_index, replicate);
    let direct_scope = project_scope(split, case, replicate, "forced-direct");
    let workflow_scope = project_scope(split, case, replicate, "forced-workflow");
    let run_arm = |root: &Path,
                   constraint: AgentExecutionConstraint,
                   scope: &str,
                   index: usize,
                   position: usize,
                   plan_anchor: Option<&MatchedRoutePlanAnchor>,
                   journal: &mut CampaignJournal| {
        execute_journaled_campaign_case_with_constraint(
            app,
            state,
            provider,
            evaluation_database,
            root,
            case,
            index,
            position,
            replicate,
            suite_sha256,
            None,
            Treatment::Pro,
            scope,
            split,
            scope,
            Some(constraint),
            plan_anchor,
            journal,
        )
    };
    let (direct, direct_receipt, workflow, workflow_receipt, execution_order) = if workflow_first {
        let (workflow, workflow_receipt) = run_arm(
            &workflow_root,
            AgentExecutionConstraint::MatchedWorkflow,
            &workflow_scope,
            *execution_index,
            1,
            None,
            journal,
        )?;
        let plan_anchor = matched_route_plan_anchor(&workflow)?;
        *execution_index = execution_index.saturating_add(1);
        let (direct, direct_receipt) = run_arm(
            &direct_root,
            AgentExecutionConstraint::MatchedDirect,
            &direct_scope,
            *execution_index,
            2,
            Some(&plan_anchor),
            journal,
        )?;
        *execution_index = execution_index.saturating_add(1);
        (
            direct,
            direct_receipt,
            workflow,
            workflow_receipt,
            "forced_workflow_then_forced_direct",
        )
    } else {
        let (direct, direct_receipt) = run_arm(
            &direct_root,
            AgentExecutionConstraint::MatchedDirect,
            &direct_scope,
            *execution_index,
            1,
            None,
            journal,
        )?;
        let plan_anchor = matched_route_plan_anchor(&direct)?;
        *execution_index = execution_index.saturating_add(1);
        let (workflow, workflow_receipt) = run_arm(
            &workflow_root,
            AgentExecutionConstraint::MatchedWorkflow,
            &workflow_scope,
            *execution_index,
            2,
            Some(&plan_anchor),
            journal,
        )?;
        *execution_index = execution_index.saturating_add(1);
        (
            direct,
            direct_receipt,
            workflow,
            workflow_receipt,
            "forced_direct_then_forced_workflow",
        )
    };
    validate_matched_route_receipts(case, &direct_receipt, &workflow_receipt)?;
    let (shadow_outcome_pair, shadow_outcome_error) =
        project_shadow_outcome_pair(&direct, &workflow);
    let evaluation_id = format!(
        "route-treatment-pair-{}",
        &sha256_hex(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                suite_sha256,
                case.id,
                split.label(),
                replicate,
                execution_order,
            )
            .as_bytes()
        )[..20]
    );
    let pair = ProductPairReceipt::new(
        evaluation_id,
        execution_order.to_string(),
        direct_prestate,
        direct_receipt,
        workflow_receipt,
    )?;
    Ok(MatchedRoutePairRun {
        pair,
        direct,
        workflow,
        shadow_outcome_pair,
        shadow_outcome_error,
    })
}

fn validate_matched_route_receipts(
    case: &RealworldCase,
    direct: &ProductRunReceipt,
    workflow: &ProductRunReceipt,
) -> Result<(), String> {
    if direct.execution_mode != "direct" || workflow.execution_mode != "workflow" {
        return Err(format!(
            "matched route pair {} did not exercise Direct and Workflow",
            case.id
        ));
    }
    if direct.execution_constraint.as_deref() != Some("matched_direct")
        || workflow.execution_constraint.as_deref() != Some("matched_workflow")
    {
        return Err(format!(
            "matched route pair {} is missing its treatment constraints",
            case.id
        ));
    }
    if direct.profile_sha256.is_none()
        || direct.profile_sha256 != workflow.profile_sha256
        || direct.route_profile_sha256 != workflow.route_profile_sha256
    {
        return Err(format!(
            "matched route pair {} changed the prompt or route profile between arms",
            case.id
        ));
    }
    if direct.conductor_candidate_sha256.is_none()
        || direct.conductor_candidate_sha256 != workflow.conductor_candidate_sha256
        || direct.workflow_proposal_sha256.is_none()
        || direct.workflow_proposal_sha256 != workflow.workflow_proposal_sha256
    {
        return Err(format!(
            "matched route pair {} did not share one conductor candidate and workflow proposal",
            case.id
        ));
    }
    if direct.execution_plan_authority.as_deref() != Some("runtime_constraint")
        || workflow.execution_plan_authority.as_deref() != Some("runtime_constraint")
    {
        return Err(format!(
            "matched route pair {} is missing runtime treatment authority receipts",
            case.id
        ));
    }
    let direct_exposure = direct.treatment_exposure.as_ref().ok_or_else(|| {
        format!(
            "matched route pair {} is missing Direct treatment exposure",
            case.id
        )
    })?;
    let workflow_exposure = workflow.treatment_exposure.as_ref().ok_or_else(|| {
        format!(
            "matched route pair {} is missing Workflow treatment exposure",
            case.id
        )
    })?;
    if direct_exposure.logical_model_calls != direct.model_calls
        || workflow_exposure.logical_model_calls != workflow.model_calls
    {
        return Err(format!(
            "matched route pair {} changed the model-call counting boundary",
            case.id
        ));
    }
    if direct_exposure.non_owner_permission_gated_calls > 0
        || workflow_exposure.non_owner_permission_gated_calls > 0
    {
        return Err(format!(
            "matched route pair {} exposed permission-gated authority outside Owner",
            case.id
        ));
    }
    if direct_exposure.workflow_planned
        || direct_exposure.workflow_completed
        || direct_exposure.worker_model_calls > 0
        || direct_exposure.successful_specialist_model_calls > 0
        || direct_exposure.successful_independent_verifier_model_calls > 0
        || direct_exposure.direct_anchor_competition_calls > 0
    {
        return Err(format!(
            "matched route pair {} exposed workflow workers in Direct",
            case.id
        ));
    }
    if !workflow_exposure.workflow_planned
        || !workflow_exposure.workflow_completed
        || workflow_exposure.successful_workflow_specialist_model_calls != 1
        || workflow_exposure
            .successful_workflow_specialist_models
            .len()
            != 1
        || workflow.workflow_verifier_steps > 1
        || workflow_exposure.successful_workflow_verifier_model_calls
            != workflow.workflow_verifier_steps
        || workflow_exposure.successful_workflow_verifier_models.len()
            != workflow.workflow_verifier_steps
        || (workflow.workflow_verifier_steps == 1
            && !workflow_exposure
                .successful_workflow_specialist_models
                .is_disjoint(&workflow_exposure.successful_workflow_verifier_models))
        || workflow_exposure.successful_specialist_model_calls
            != workflow_exposure.successful_workflow_specialist_model_calls
        || workflow_exposure.successful_independent_verifier_model_calls
            != workflow_exposure.successful_workflow_verifier_model_calls
        || workflow_exposure.direct_anchor_competition_calls > 0
    {
        return Err(format!(
            "matched route pair {} did not expose the completed Workflow treatment",
            case.id
        ));
    }
    Ok(())
}

fn matched_route_plan_anchor(run: &RawRun) -> Result<MatchedRoutePlanAnchor, String> {
    run.strategy_receipt
        .as_ref()
        .and_then(|receipt| receipt.matched_route_plan_anchor.clone())
        .ok_or_else(|| {
            format!(
                "matched route pair {} did not retain its private shared plan anchor",
                run.case_id
            )
        })
}

fn candidate_executes_first(pair_index: usize, replicate: u32) -> bool {
    (pair_index + replicate.saturating_sub(1) as usize) % 2 == 1
}

#[allow(clippy::too_many_arguments)]
fn execute_campaign_case_with_constraint(
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
    execution_constraint: Option<AgentExecutionConstraint>,
    matched_route_plan_anchor: Option<&MatchedRoutePlanAnchor>,
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
            execution_constraint,
            matched_route_plan_anchor,
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
    budget.max_total_tokens =
        80_u64.saturating_mul(agent_runtime::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT);
    budget.terminal_physical_model_attempt_reserve = 12;
    budget.terminal_token_reserve =
        12_u64.saturating_mul(agent_runtime::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT);
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
    use super::super::receipts::TreatmentExposureReceipt;
    use super::*;
    use std::{collections::BTreeSet, fs};

    fn route_receipt(execution_mode: &str) -> ProductRunReceipt {
        let workflow = execution_mode == "workflow";
        let treatment_exposure = if workflow {
            TreatmentExposureReceipt {
                logical_model_calls: 2,
                worker_model_calls: 2,
                successful_specialist_model_calls: 1,
                successful_independent_verifier_model_calls: 1,
                successful_workflow_specialist_model_calls: 1,
                successful_workflow_verifier_model_calls: 1,
                successful_workflow_specialist_models: BTreeSet::from([
                    "specialist-model".to_string()
                ]),
                successful_workflow_verifier_models: BTreeSet::from(["verifier-model".to_string()]),
                workflow_planned: true,
                workflow_completed: true,
                ..TreatmentExposureReceipt::default()
            }
        } else {
            TreatmentExposureReceipt {
                logical_model_calls: 1,
                successful_owner_model_calls: 1,
                ..TreatmentExposureReceipt::default()
            }
        };
        ProductRunReceipt {
            case_id: "case".to_string(),
            category: "coding".to_string(),
            split: CampaignSplit::Train,
            replicate: 1,
            treatment: "pro".to_string(),
            execution_mode: execution_mode.to_string(),
            execution_constraint: Some(if workflow {
                "matched_workflow".to_string()
            } else {
                "matched_direct".to_string()
            }),
            completed: true,
            terminal_status: "completed".to_string(),
            behavior_checks_passed: 1,
            behavior_checks_total: 1,
            behavior_score: 1.0,
            quality_passed: true,
            safety_violations: 0,
            latency_ms: 1,
            model_calls: treatment_exposure.logical_model_calls,
            total_tokens: 1,
            output_sha256: "a".repeat(64),
            conductor_candidate_sha256: Some("0".repeat(64)),
            workflow_proposal_sha256: Some("1".repeat(64)),
            profile_id: Some("profile".to_string()),
            profile_sha256: Some("b".repeat(64)),
            route_profile_sha256: Some("c".repeat(64)),
            execution_plan_sha256: Some("d".repeat(64)),
            execution_plan_semantic_sha256: Some("e".repeat(64)),
            execution_plan_authority: Some("runtime_constraint".to_string()),
            workflow_execution_profile_sha256: workflow.then(|| "f".repeat(64)),
            route_profile_semantics_exercised: true,
            workflow_profile_exercised: workflow,
            workflow_verifier_steps: usize::from(workflow),
            treatment_exposure: Some(treatment_exposure),
        }
    }

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
    fn agent_execution_graph_contract_preserves_shared_anchor_and_balanced_order() {
        let case = RealworldCase {
            id: "case".to_string(),
            category: "coding".to_string(),
            objective: "objective".to_string(),
            campaign_split: Some("train".to_string()),
            expected_execution_mode: None,
            seed_memory_prompt: None,
            index_workspace: false,
            files: Vec::new(),
            permission_policy: super::super::PermissionPolicy::AllowOnce,
            verification: Default::default(),
            memory_effect: None,
        };
        let direct = route_receipt("direct");
        let workflow = route_receipt("workflow");
        validate_matched_route_receipts(&case, &direct, &workflow).unwrap();
        let mut workflow_without_verifier = workflow.clone();
        workflow_without_verifier.workflow_verifier_steps = 0;
        let exposure = workflow_without_verifier
            .treatment_exposure
            .as_mut()
            .unwrap();
        exposure.successful_independent_verifier_model_calls = 0;
        exposure.successful_workflow_verifier_model_calls = 0;
        exposure.successful_workflow_verifier_models.clear();
        validate_matched_route_receipts(&case, &direct, &workflow_without_verifier).unwrap();

        let mut correlated_verifier = workflow.clone();
        correlated_verifier
            .treatment_exposure
            .as_mut()
            .unwrap()
            .successful_workflow_verifier_models = BTreeSet::from(["specialist-model".to_string()]);
        assert!(validate_matched_route_receipts(&case, &direct, &correlated_verifier).is_err());
        for pair_index in 0..4 {
            assert_ne!(
                candidate_executes_first(pair_index, 1),
                candidate_executes_first(pair_index + 4, 2),
            );
        }

        let mut drifted = workflow.clone();
        drifted.route_profile_sha256 = Some("9".repeat(64));
        assert!(validate_matched_route_receipts(&case, &direct, &drifted).is_err());
        let mut different_candidate = workflow.clone();
        different_candidate.conductor_candidate_sha256 = Some("8".repeat(64));
        assert!(validate_matched_route_receipts(&case, &direct, &different_candidate).is_err());
        let mut different_proposal = workflow.clone();
        different_proposal.workflow_proposal_sha256 = Some("7".repeat(64));
        assert!(validate_matched_route_receipts(&case, &direct, &different_proposal).is_err());
        assert!(validate_matched_route_receipts(&case, &direct, &direct).is_err());
    }

    #[test]
    fn agent_execution_graph_contract_rejects_label_only_workflow() {
        let case = RealworldCase {
            id: "case".to_string(),
            category: "coding".to_string(),
            objective: "objective".to_string(),
            campaign_split: Some("train".to_string()),
            expected_execution_mode: None,
            seed_memory_prompt: None,
            index_workspace: false,
            files: Vec::new(),
            permission_policy: super::super::PermissionPolicy::AllowOnce,
            verification: Default::default(),
            memory_effect: None,
        };
        let direct = route_receipt("direct");
        let mut workflow = route_receipt("workflow");
        let exposure = workflow.treatment_exposure.as_mut().unwrap();
        exposure.workflow_planned = false;
        exposure.workflow_completed = false;
        exposure.successful_workflow_specialist_model_calls = 0;
        exposure.successful_workflow_verifier_model_calls = 0;

        assert!(validate_matched_route_receipts(&case, &direct, &workflow).is_err());

        let mut extra_verifier = route_receipt("workflow");
        let exposure = extra_verifier.treatment_exposure.as_mut().unwrap();
        exposure.successful_independent_verifier_model_calls = 2;
        exposure.successful_workflow_verifier_model_calls = 2;
        assert!(validate_matched_route_receipts(&case, &direct, &extra_verifier).is_err());
    }

    #[test]
    fn agent_execution_graph_contract_rejects_direct_worker_exposure() {
        let case = RealworldCase {
            id: "case".to_string(),
            category: "coding".to_string(),
            objective: "objective".to_string(),
            campaign_split: Some("train".to_string()),
            expected_execution_mode: None,
            seed_memory_prompt: None,
            index_workspace: false,
            files: Vec::new(),
            permission_policy: super::super::PermissionPolicy::AllowOnce,
            verification: Default::default(),
            memory_effect: None,
        };
        let mut direct = route_receipt("direct");
        direct
            .treatment_exposure
            .as_mut()
            .unwrap()
            .successful_specialist_model_calls = 1;
        let workflow = route_receipt("workflow");

        assert!(validate_matched_route_receipts(&case, &direct, &workflow).is_err());

        let mut attempted_worker = route_receipt("direct");
        attempted_worker
            .treatment_exposure
            .as_mut()
            .unwrap()
            .worker_model_calls = 1;
        assert!(validate_matched_route_receipts(&case, &attempted_worker, &workflow).is_err());

        let mut unsafe_direct = route_receipt("direct");
        unsafe_direct
            .treatment_exposure
            .as_mut()
            .unwrap()
            .non_owner_permission_gated_calls = 1;
        assert!(validate_matched_route_receipts(&case, &unsafe_direct, &workflow).is_err());

        let mut recounted_workflow = workflow;
        recounted_workflow
            .treatment_exposure
            .as_mut()
            .unwrap()
            .logical_model_calls += 1;
        assert!(validate_matched_route_receipts(
            &case,
            &route_receipt("direct"),
            &recounted_workflow
        )
        .is_err());
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
