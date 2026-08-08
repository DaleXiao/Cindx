use super::direct_finalizer_campaign_support::{require_clean_source, required_external_path};
use super::execution::{execute_case, CaseExecutionInput};
use super::setup::{activate_evaluation_data_root, build_evaluation_app};
use super::{materialize_case, ExecutionCell, RawRun, RealworldCase, RealworldSuite, Treatment};
use crate::app_state::AppState;
use crate::collaboration_execution::collaboration_candidate_models;
use crate::configuration_models::{ProviderConfig, SidecarConfig};
use crate::configuration_persistence::load_provider_config;
use crate::phase16_task_id;
use crate::prompt_evidence_runtime::{
    prompt_direct_profile_evidence_counts, prompt_learning_dataset, prompt_offline_dataset_digest,
};
use crate::prompt_evolution_read_model::{
    load_prompt_evolution_read_model, prompt_evolution_read_model_for_scope,
};
use crate::prompt_evolution_store_runtime::with_prompt_evolution_store;
use crate::prompt_learning_runtime::prompt_evaluation_parent_budget;
use crate::prompt_pairwise_runtime::run_background_prompt_pairwise_evaluation;
use agent_core::Metadata;
use agent_runtime::AgentRunControl;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, AgentPolicy, ConductorPromptGenome,
    FrozenPromptProfileSnapshot, PromptEvaluationSplit, PromptEvolutionObservation,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::Manager;

const CAMPAIGN_SCHEMA: &str = "cindx.workflow-gepa-campaign.v2";
const CAMPAIGN_SUITE_ID: &str = "cindx-workflow-gepa-v2";
const CAMPAIGN_PROJECT_ID: &str = "project-workflow-gepa-v2";
const MAX_PAIRWISE_ACTIONS: usize = 24;
const MIN_TRAIN_EVIDENCE: usize = 6;
const MIN_HOLDOUT_EVIDENCE: usize = 4;
const MIN_HOLDOUT_TASK_CLASSES: usize = 2;
const MIN_HOLDOUT_RELATIVE_REWARD: f64 = 0.02;

#[derive(Debug, Serialize)]
struct SeedRunReceipt {
    case_id: String,
    completed: bool,
    quality_passed: bool,
    external_effect_passed: Option<bool>,
    safety_violations: usize,
    latency_ms: u64,
    total_tokens: u64,
    output_sha256: String,
}

#[derive(Debug, Serialize)]
struct CandidateEvidenceReceipt {
    profile_id: String,
    profile_sha256: String,
    route_profile_sha256: String,
    train_runs: usize,
    holdout_runs: usize,
    unique_holdout_cases: usize,
    holdout_task_classes: usize,
    holdout_relative_reward: f64,
    holdout_candidate_quality: f64,
    holdout_seed_quality: f64,
    holdout_latency_ratio: f64,
    holdout_token_ratio: f64,
    holdout_safety_violations: u64,
    holdout_failure_regressions: usize,
}

#[derive(Debug, Serialize)]
struct RouteExerciseReceipt {
    case_id: String,
    seed_quality_passed: bool,
    candidate_quality_passed: bool,
    seed_execution_mode: Option<String>,
    candidate_execution_mode: Option<String>,
    candidate_profile_id: Option<String>,
    candidate_profile_source: Option<String>,
    candidate_route_profile_semantics_exercised: bool,
    candidate_workflow_profile_exercised: bool,
}

#[derive(Debug, Serialize)]
struct WorkflowGepaCampaignReceipt {
    schema: &'static str,
    source_commit: String,
    suite_id: String,
    suite_sha256: String,
    provider_id: String,
    provider_endpoint_sha256: String,
    configured_models: Vec<String>,
    seed_runs: Vec<SeedRunReceipt>,
    learning_dataset_sha256: String,
    workspace_prestate_sha256: String,
    workspace_mutated_sha256: String,
    workspace_restored_sha256: String,
    learning_dataset_cases: usize,
    learning_train_cases: usize,
    learning_holdout_cases: usize,
    learning_task_classes: usize,
    pairwise_actions: usize,
    candidate: CandidateEvidenceReceipt,
    route_exercise: RouteExerciseReceipt,
    grounded_direct_control: SeedRunReceipt,
    production_promotion_claimed: bool,
    status: String,
}

struct EvaluationProfileEnvironment {
    prior_path: Option<OsString>,
    prior_sha256: Option<OsString>,
}

struct EvaluationDataEnvironment {
    prior_requested_root: Option<OsString>,
    prior_active_root: Option<OsString>,
}

impl EvaluationDataEnvironment {
    fn install(root: &Path) -> Self {
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

pub(super) fn run() -> Result<(), String> {
    super::install_eval_crypto_provider();
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))?;
    let source_commit = require_clean_source(&repo_root)?;
    let suite_path = std::env::var_os("CINDX_WORKFLOW_GEPA_SUITE")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("benchmarks/agent/workflow-gepa-v2.json"));
    let suite_bytes = fs::read(&suite_path)
        .map_err(|error| format!("failed to read {}: {error}", suite_path.display()))?;
    let suite: RealworldSuite = serde_json::from_slice(&suite_bytes)
        .map_err(|error| format!("invalid Workflow GEPA suite JSON: {error}"))?;
    validate_campaign_suite(&suite)?;
    let suite_sha256 = sha256_hex(&suite_bytes);
    let report_path = required_external_path(
        "CINDX_WORKFLOW_GEPA_REPORT",
        &repo_root,
        "workflow GEPA report",
    )?;
    let snapshot_path = required_external_path(
        "CINDX_WORKFLOW_GEPA_SNAPSHOT",
        &repo_root,
        "workflow GEPA snapshot",
    )?;

    let provider = load_provider_config();
    if !provider.is_ready() {
        return Err("configured provider is required; campaign will not synthesize results".into());
    }
    let policy = AgentPolicy::Pro;
    let agent_budget = policy.max_parallelism();
    let worker_models = collaboration_candidate_models(&provider, agent_budget);
    if worker_models.len() < 2 {
        return Err(
            "Workflow GEPA campaign requires at least two distinct configured models".into(),
        );
    }

    let temp = tempfile::Builder::new()
        .prefix("cindx-workflow-gepa-")
        .tempdir()
        .map_err(|error| format!("failed to create campaign workspace: {error}"))?;
    let suite_root = temp.path();
    let _evaluation_data_environment = EvaluationDataEnvironment::install(suite_root);
    let evaluation_database = activate_evaluation_data_root(suite_root)?;
    let workspace_root = suite_root.join("workspace");
    reset_suite_workspace(&workspace_root, &suite)?;
    let workspace_prestate_sha256 = campaign_workspace_sha256(&workspace_root)?;
    let sidecars = SidecarConfig::default();
    super::apply_sidecar_env(&sidecars);
    let mut seed_runtime_provider = provider.clone();
    seed_runtime_provider.prompt_evolution_enabled = false;
    let app = build_evaluation_app(
        seed_runtime_provider.clone(),
        sidecars,
        &workspace_root,
        &evaluation_database,
    )?;
    let state = app.state::<AppState>();
    let seed_profile = ConductorPromptGenome::seed_for_effort(policy.label());
    let mut seed_runs = Vec::new();
    for (index, case) in suite.cases.iter().enumerate() {
        eprintln!(
            "[workflow-gepa] seed task {}/{}: {}",
            index + 1,
            suite.cases.len(),
            case.id
        );
        let run = execute_campaign_case(
            &app,
            &state,
            &seed_runtime_provider,
            &evaluation_database,
            &workspace_root,
            case,
            index + 1,
            &suite_sha256,
            None,
            Treatment::Pro,
        );
        seed_runs.push(seed_run_receipt(&run));
        validate_seed_run(&run)?;
    }
    let workspace_mutated_sha256 = campaign_workspace_sha256(&workspace_root)?;
    if workspace_mutated_sha256 == workspace_prestate_sha256 {
        return Err("seed tasks did not produce an observable workspace transition".to_string());
    }
    reset_suite_workspace(&workspace_root, &suite)?;
    let workspace_restored_sha256 = campaign_workspace_sha256(&workspace_root)?;
    if workspace_restored_sha256 != workspace_prestate_sha256 {
        return Err(
            "campaign workspace could not be restored to its pre-treatment state".to_string(),
        );
    }

    let (dataset, run_context) = learning_dataset_and_context(&state)?;
    let dataset_sha256 = prompt_offline_dataset_digest(&dataset);
    let train_cases = dataset
        .iter()
        .filter(|case| case.split == PromptEvaluationSplit::Train)
        .count();
    let holdout_cases = dataset.len().saturating_sub(train_cases);
    let task_classes = dataset
        .iter()
        .map(|case| case.task_class.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    if dataset.len() < suite.cases.len() || train_cases < 4 || holdout_cases < 4 || task_classes < 2
    {
        return Err(format!(
            "real task evidence is insufficient: cases={}, train={}, holdout={}, classes={}",
            dataset.len(),
            train_cases,
            holdout_cases,
            task_classes
        ));
    }

    let mut pairwise_actions = 0usize;
    let candidate = loop {
        let control = Arc::new(AgentRunControl::with_budget(
            prompt_evaluation_parent_budget(),
        ));
        let outcome = run_background_prompt_pairwise_evaluation(
            &state,
            &provider,
            &phase16_task_id(),
            &run_context,
            policy.label(),
            policy.requested_policy().label(),
            &worker_models,
            agent_budget,
            &seed_profile,
            None,
            &control,
        )?;
        if let Some(error) = outcome.deployment_recovery_error {
            return Err(format!("prompt deployment recovery failed: {error}"));
        }
        if outcome.progressed {
            pairwise_actions = pairwise_actions.saturating_add(1);
        }
        let model = scoped_prompt_model(&state)?;
        if let Some(candidate) = strongest_learned_candidate(&model, &seed_profile)? {
            if candidate.train_runs >= MIN_TRAIN_EVIDENCE
                && candidate.holdout_runs >= MIN_HOLDOUT_EVIDENCE
            {
                break candidate;
            }
        }
        if !outcome.progressed || pairwise_actions >= MAX_PAIRWISE_ACTIONS {
            return Err(format!(
                "Workflow GEPA did not produce a sufficiently evaluated learned profile in {pairwise_actions} actions"
            ));
        }
    };
    validate_candidate_gate(&candidate)?;

    let model = scoped_prompt_model(&state)?;
    let candidate_genome = model
        .genomes
        .iter()
        .find(|record| record.effort == policy.label() && record.genome.id == candidate.profile_id)
        .map(|record| record.genome.clone())
        .ok_or_else(|| {
            "selected learned profile is absent from the canonical read model".to_string()
        })?;
    let evidence = candidate_observations(&model, &candidate.profile_id, &seed_profile.id);
    let mut ordered_evidence = evidence.into_iter().cloned().collect::<Vec<_>>();
    ordered_evidence.sort_by_key(PromptEvolutionObservation::evidence_identity);
    let evidence_sha256 = sha256_hex(
        &serde_json::to_vec(&ordered_evidence)
            .map_err(|error| format!("failed to encode paired evidence: {error}"))?,
    );
    let snapshot = FrozenPromptProfileSnapshot::new_gepa(
        policy.label(),
        candidate_genome,
        seed_profile.id.clone(),
        dataset_sha256.clone(),
        evidence_sha256,
    )?;
    let snapshot_bytes = serde_json::to_vec_pretty(&snapshot)
        .map_err(|error| format!("failed to encode Workflow GEPA snapshot: {error}"))?;
    tools::write_private_file_atomically(&snapshot_path, &snapshot_bytes)
        .map_err(|error| format!("failed to write Workflow GEPA snapshot: {error}"))?;

    let route_case = suite
        .cases
        .iter()
        .find(|case| case.id == "route-collaboration-proof")
        .ok_or_else(|| "campaign route exercise case is missing".to_string())?;
    let route_seed_root = suite_root.join("route-seed");
    let route_candidate_root = suite_root.join("route-candidate");
    materialize_case(&route_seed_root, route_case)?;
    materialize_case(&route_candidate_root, route_case)?;
    let route_seed = execute_campaign_case(
        &app,
        &state,
        &seed_runtime_provider,
        &evaluation_database,
        &route_seed_root,
        route_case,
        suite.cases.len() + 1,
        &suite_sha256,
        None,
        Treatment::Pro,
    );
    validate_seed_run(&route_seed)?;
    let artifact_sha256 = snapshot.artifact_sha256()?;
    let route_candidate = {
        let _profile_environment =
            EvaluationProfileEnvironment::install(&snapshot_path, &artifact_sha256);
        execute_campaign_case(
            &app,
            &state,
            &seed_runtime_provider,
            &evaluation_database,
            &route_candidate_root,
            route_case,
            suite.cases.len() + 2,
            &suite_sha256,
            Some(&snapshot),
            Treatment::Pro,
        )
    };
    validate_seed_run(&route_candidate)?;
    let route_exercise = route_exercise_receipt(route_case, &route_seed, &route_candidate);
    if !route_exercise.candidate_route_profile_semantics_exercised
        || !route_exercise.candidate_workflow_profile_exercised
        || route_exercise.candidate_profile_id.as_deref() != Some(candidate.profile_id.as_str())
    {
        return Err("learned profile did not exercise both route semantics and Workflow in the full product path".to_string());
    }

    let grounded_case = suite
        .cases
        .iter()
        .find(|case| case.id == "coding-calculate-total")
        .ok_or_else(|| "campaign Grounded Direct control case is missing".to_string())?;
    let grounded_root = suite_root.join("grounded-direct-control");
    materialize_case(&grounded_root, grounded_case)?;
    let grounded_direct_control = execute_campaign_case(
        &app,
        &state,
        &seed_runtime_provider,
        &evaluation_database,
        &grounded_root,
        grounded_case,
        suite.cases.len() + 3,
        &suite_sha256,
        None,
        Treatment::GroundedDirect,
    );
    validate_seed_run(&grounded_direct_control)?;

    let receipt = WorkflowGepaCampaignReceipt {
        schema: CAMPAIGN_SCHEMA,
        source_commit,
        suite_id: suite.id,
        suite_sha256,
        provider_id: provider.provider_id,
        provider_endpoint_sha256: sha256_hex(provider.base_url.as_bytes()),
        configured_models: worker_models,
        seed_runs,
        learning_dataset_sha256: dataset_sha256,
        workspace_prestate_sha256,
        workspace_mutated_sha256,
        workspace_restored_sha256,
        learning_dataset_cases: dataset.len(),
        learning_train_cases: train_cases,
        learning_holdout_cases: holdout_cases,
        learning_task_classes: task_classes,
        pairwise_actions,
        candidate,
        route_exercise,
        grounded_direct_control: seed_run_receipt(&grounded_direct_control),
        production_promotion_claimed: false,
        status: "exploratory_matched_gate_passed".to_string(),
    };
    let encoded = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| format!("failed to encode Workflow GEPA report: {error}"))?;
    tools::write_private_file_atomically(&report_path, &encoded)
        .map_err(|error| format!("failed to write Workflow GEPA report: {error}"))?;
    eprintln!(
        "[workflow-gepa] passed actions={} report={} snapshot={}",
        pairwise_actions,
        report_path.display(),
        snapshot_path.display()
    );
    Ok(())
}

fn reset_suite_workspace(root: &Path, suite: &RealworldSuite) -> Result<(), String> {
    if root.exists() {
        fs::remove_dir_all(root)
            .map_err(|error| format!("failed to reset {}: {error}", root.display()))?;
    }
    fs::create_dir_all(root)
        .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
    for case in &suite.cases {
        materialize_case(root, case)?;
    }
    Ok(())
}

fn campaign_workspace_sha256(root: &Path) -> Result<String, String> {
    let control = AgentRunControl::with_budget(prompt_evaluation_parent_budget());
    crate::prompt_learning_runtime::prompt_workspace_revision_sha256(root, &control)
}

#[allow(clippy::too_many_arguments)]
fn execute_campaign_case(
    app: &tauri::App<tauri::Wry>,
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    evaluation_database: &Path,
    root: &Path,
    case: &RealworldCase,
    execution_index: usize,
    plan_sha256: &str,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
    treatment: Treatment,
) -> RawRun {
    let execution = ExecutionCell {
        execution_index,
        treatment_position: 1,
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
            replicate: 1,
            root,
            execution: &execution,
            frozen_profile,
            project_scope: Some(CAMPAIGN_PROJECT_ID),
        },
    )
}

fn seed_run_receipt(run: &RawRun) -> SeedRunReceipt {
    SeedRunReceipt {
        case_id: run.case_id.clone(),
        completed: run.completed,
        quality_passed: run.verification.quality_passed,
        external_effect_passed: run.verification.external_effect_passed,
        safety_violations: run.verification.safety_violations,
        latency_ms: run.metrics.latency_ms,
        total_tokens: run.metrics.total_tokens,
        output_sha256: run.output_sha256.clone(),
    }
}

fn validate_seed_run(run: &RawRun) -> Result<(), String> {
    if !run.completed
        || !run.verification.quality_passed
        || run.verification.external_effect_passed != Some(true)
        || run.verification.safety_violations > 0
        || run.evidence_error.is_some()
    {
        return Err(format!(
            "real product task {} failed its completion/evidence gate: status={}, failures={:?}, evidence={:?}, successful_tools={:?}, tool_statuses={:?}",
            run.case_id,
            run.terminal_status,
            run.verification.failures,
            run.evidence_error,
            run.tools_used,
            run.tool_receipts
                .iter()
                .map(|receipt| format!("{}:{:?}", receipt.tool, receipt.status))
                .collect::<Vec<_>>()
        ));
    }
    Ok(())
}

fn learning_dataset_and_context(
    state: &tauri::State<'_, AppState>,
) -> Result<
    (
        Vec<crate::collaboration_models::PromptOfflineCase>,
        Metadata,
    ),
    String,
> {
    let events = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?
        .list_by_task_and_metadata(&phase16_task_id(), "project_id", CAMPAIGN_PROJECT_ID)
        .map_err(|error| error.to_string())?;
    let dataset = prompt_learning_dataset(&events, CAMPAIGN_PROJECT_ID, None);
    let run_context = events
        .iter()
        .rev()
        .find(|event| event.summary == "Agent task completed")
        .map(|event| event.metadata.clone())
        .ok_or_else(|| "campaign has no completed run context".to_string())?;
    Ok((dataset, run_context))
}

fn scoped_prompt_model(
    state: &tauri::State<'_, AppState>,
) -> Result<crate::view_models::PromptEvolutionReadModel, String> {
    with_prompt_evolution_store(state, |store| {
        let model = load_prompt_evolution_read_model(store).map_err(|error| error.to_string())?;
        Ok(prompt_evolution_read_model_for_scope(
            &model,
            CAMPAIGN_PROJECT_ID,
        ))
    })
}

fn candidate_observations<'a>(
    model: &'a crate::view_models::PromptEvolutionReadModel,
    candidate_id: &str,
    seed_id: &str,
) -> Vec<&'a PromptEvolutionObservation> {
    model
        .observations
        .iter()
        .filter(|(effort, observation)| {
            effort == AgentPolicy::Pro.label()
                && observation.profile_id == candidate_id
                && observation.opponent_profile_id.as_deref() == Some(seed_id)
                && observation.is_strict_matched_evidence()
        })
        .map(|(_, observation)| observation)
        .collect()
}

fn strongest_learned_candidate(
    model: &crate::view_models::PromptEvolutionReadModel,
    seed: &ConductorPromptGenome,
) -> Result<Option<CandidateEvidenceReceipt>, String> {
    let seed_route = seed.route_decision_profile_sha256(AgentPolicy::Pro.label())?;
    let mut candidates = model
        .genomes
        .iter()
        .filter(|record| record.effort == AgentPolicy::Pro.label())
        .filter(|record| record.genome.id.starts_with("learned-"))
        .map(|record| record.genome.clone())
        .filter(|candidate| {
            candidate
                .route_decision_profile_sha256(AgentPolicy::Pro.label())
                .is_ok_and(|sha256| sha256 != seed_route)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    let mut receipts = candidates
        .iter()
        .map(|candidate| candidate_evidence_receipt(model, candidate, seed))
        .collect::<Result<Vec<_>, _>>()?;
    receipts.sort_by(|left, right| {
        right
            .holdout_runs
            .cmp(&left.holdout_runs)
            .then_with(|| right.train_runs.cmp(&left.train_runs))
            .then_with(|| {
                right
                    .holdout_relative_reward
                    .total_cmp(&left.holdout_relative_reward)
            })
            .then_with(|| left.profile_id.cmp(&right.profile_id))
    });
    Ok(receipts.into_iter().next())
}

fn candidate_evidence_receipt(
    model: &crate::view_models::PromptEvolutionReadModel,
    candidate: &ConductorPromptGenome,
    seed: &ConductorPromptGenome,
) -> Result<CandidateEvidenceReceipt, String> {
    let observations = candidate_observations(model, &candidate.id, &seed.id);
    let (train_runs, holdout_runs) = prompt_direct_profile_evidence_counts(
        &model
            .observations
            .iter()
            .filter(|(effort, _)| effort == AgentPolicy::Pro.label())
            .map(|(_, observation)| observation.clone())
            .collect::<Vec<_>>(),
        &candidate.id,
        &seed.id,
    );
    let holdout = observations
        .iter()
        .copied()
        .filter(|observation| observation.split == PromptEvaluationSplit::Holdout)
        .collect::<Vec<_>>();
    let seed_by_evaluation = model
        .observations
        .iter()
        .filter(|(effort, observation)| {
            effort == AgentPolicy::Pro.label()
                && observation.profile_id == seed.id
                && observation.opponent_profile_id.as_deref() == Some(candidate.id.as_str())
                && observation.is_strict_matched_evidence()
        })
        .map(|(_, observation)| (observation.evaluation_id.as_str(), observation))
        .collect::<std::collections::BTreeMap<_, _>>();
    let matched = holdout
        .iter()
        .filter_map(|candidate| {
            seed_by_evaluation
                .get(candidate.evaluation_id.as_str())
                .map(|seed| (*candidate, *seed))
        })
        .collect::<Vec<_>>();
    let denominator = matched.len().max(1) as f64;
    let candidate_quality = matched
        .iter()
        .map(|(candidate, _)| candidate.quality_score)
        .sum::<f64>()
        / denominator;
    let seed_quality = matched
        .iter()
        .map(|(_, seed)| seed.quality_score)
        .sum::<f64>()
        / denominator;
    let candidate_latency = matched
        .iter()
        .map(|(candidate, _)| candidate.latency_ms as f64)
        .sum::<f64>();
    let seed_latency = matched
        .iter()
        .map(|(_, seed)| seed.latency_ms as f64)
        .sum::<f64>();
    let candidate_tokens = matched
        .iter()
        .map(|(candidate, _)| candidate.total_tokens as f64)
        .sum::<f64>();
    let seed_tokens = matched
        .iter()
        .map(|(_, seed)| seed.total_tokens as f64)
        .sum::<f64>();
    Ok(CandidateEvidenceReceipt {
        profile_id: candidate.id.clone(),
        profile_sha256: prompt_genome_sha256(candidate)?,
        route_profile_sha256: candidate.route_decision_profile_sha256(AgentPolicy::Pro.label())?,
        train_runs,
        holdout_runs,
        unique_holdout_cases: holdout
            .iter()
            .map(|observation| observation.case_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        holdout_task_classes: holdout
            .iter()
            .map(|observation| observation.task_class.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        holdout_relative_reward: holdout
            .iter()
            .map(|observation| observation.relative_reward.unwrap_or_default())
            .sum::<f64>()
            / holdout.len().max(1) as f64,
        holdout_candidate_quality: candidate_quality,
        holdout_seed_quality: seed_quality,
        holdout_latency_ratio: ratio(candidate_latency, seed_latency),
        holdout_token_ratio: ratio(candidate_tokens, seed_tokens),
        holdout_safety_violations: holdout
            .iter()
            .map(|observation| observation.safety_violations)
            .sum(),
        holdout_failure_regressions: matched
            .iter()
            .filter(|(candidate, seed)| !candidate.succeeded && seed.succeeded)
            .count(),
    })
}

fn ratio(candidate: f64, baseline: f64) -> f64 {
    if baseline <= f64::EPSILON {
        if candidate <= f64::EPSILON {
            1.0
        } else {
            f64::INFINITY
        }
    } else {
        candidate / baseline
    }
}

fn validate_candidate_gate(candidate: &CandidateEvidenceReceipt) -> Result<(), String> {
    let seed_route = ConductorPromptGenome::seed_for_effort(AgentPolicy::Pro.label())
        .route_decision_profile_sha256(AgentPolicy::Pro.label())?;
    let passed = candidate.train_runs >= MIN_TRAIN_EVIDENCE
        && candidate.holdout_runs >= MIN_HOLDOUT_EVIDENCE
        && candidate.unique_holdout_cases >= MIN_HOLDOUT_EVIDENCE
        && candidate.holdout_task_classes >= MIN_HOLDOUT_TASK_CLASSES
        && candidate.holdout_relative_reward >= MIN_HOLDOUT_RELATIVE_REWARD
        && candidate.holdout_candidate_quality >= candidate.holdout_seed_quality
        && candidate.holdout_latency_ratio <= 1.05
        && candidate.holdout_token_ratio <= 1.02
        && candidate.holdout_safety_violations == 0
        && candidate.holdout_failure_regressions == 0
        && candidate.route_profile_sha256 != seed_route;
    if !passed {
        return Err(format!(
            "learned candidate failed the bounded matched gate: {}",
            serde_json::to_string(candidate)
                .unwrap_or_else(|_| "candidate receipt unavailable".to_string())
        ));
    }
    Ok(())
}

fn route_exercise_receipt(
    case: &RealworldCase,
    seed: &RawRun,
    candidate: &RawRun,
) -> RouteExerciseReceipt {
    RouteExerciseReceipt {
        case_id: case.id.clone(),
        seed_quality_passed: seed.verification.quality_passed,
        candidate_quality_passed: candidate.verification.quality_passed,
        seed_execution_mode: seed
            .strategy_receipt
            .as_ref()
            .map(|receipt| receipt.execution_mode.clone()),
        candidate_execution_mode: candidate
            .strategy_receipt
            .as_ref()
            .map(|receipt| receipt.execution_mode.clone()),
        candidate_profile_id: candidate
            .strategy_receipt
            .as_ref()
            .map(|receipt| receipt.profile_id.clone()),
        candidate_profile_source: candidate
            .strategy_receipt
            .as_ref()
            .map(|receipt| receipt.profile_source.clone()),
        candidate_route_profile_semantics_exercised: candidate
            .strategy_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.route_profile_semantics_exercised),
        candidate_workflow_profile_exercised: candidate
            .strategy_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.workflow_profile_exercised),
    }
}

fn validate_campaign_suite(suite: &RealworldSuite) -> Result<(), String> {
    if suite.schema != CAMPAIGN_SCHEMA
        || suite.id != CAMPAIGN_SUITE_ID
        || suite.version != 2
        || suite.cases.len() != 8
    {
        return Err("Workflow GEPA suite identity or case count is invalid".to_string());
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for case in &suite.cases {
        if case.id.trim().is_empty()
            || !ids.insert(case.id.as_str())
            || case.objective.trim().is_empty()
            || case.files.is_empty()
            || case.objective.contains("{{BROWSER_URL}}")
            || case.seed_memory_prompt.is_some()
            || case.index_workspace
        {
            return Err(format!("Workflow GEPA case {} is invalid", case.id));
        }
        for fixture in &case.files {
            super::validate_relative_path(&fixture.path)?;
            if !paths.insert(fixture.path.as_str()) {
                return Err(format!("duplicate campaign fixture path {}", fixture.path));
            }
        }
    }
    if !ids.contains("route-collaboration-proof") {
        return Err("Workflow GEPA suite lacks its route exercise case".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_suite_has_balanced_classes_and_immutable_splits() {
        let suite: RealworldSuite = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/workflow-gepa-v2.json"
        )))
        .unwrap();
        validate_campaign_suite(&suite).unwrap();
        assert!(suite.cases.iter().all(|case| {
            case.verification.required_tools_all.len() == 1
                && case.verification.required_tools_all[0] == "shell.run"
                && !case.verification.required_tools_any.is_empty()
        }));

        let planned = suite
            .cases
            .iter()
            .map(|case| {
                let objective_sha256 = sha256_hex(case.objective.as_bytes());
                let id = format!("runtime-{}-{}", case.category, &objective_sha256[..16]);
                let split_sha256 = sha256_hex(id.as_bytes());
                let bucket = u8::from_str_radix(&split_sha256[..2], 16).unwrap();
                (
                    case.category.as_str(),
                    if bucket.is_multiple_of(5) {
                        PromptEvaluationSplit::Holdout
                    } else {
                        PromptEvaluationSplit::Train
                    },
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            planned
                .iter()
                .filter(|(_, split)| *split == PromptEvaluationSplit::Train)
                .count(),
            4
        );
        assert_eq!(
            planned
                .iter()
                .filter(|(_, split)| *split == PromptEvaluationSplit::Holdout)
                .count(),
            4
        );
        for category in ["coding", "research"] {
            assert!(planned.iter().any(|(observed, split)| {
                *observed == category && *split == PromptEvaluationSplit::Train
            }));
            assert!(planned.iter().any(|(observed, split)| {
                *observed == category && *split == PromptEvaluationSplit::Holdout
            }));
        }
    }

    #[test]
    fn campaign_workspace_reset_removes_treatment_outputs() {
        let suite: RealworldSuite = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/workflow-gepa-v2.json"
        )))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        reset_suite_workspace(&root, &suite).unwrap();
        let baseline = campaign_workspace_sha256(&root).unwrap();

        fs::write(root.join("wg1/calc.mjs"), "treated\n").unwrap();
        fs::write(root.join("wg5/out.json"), "{}\n").unwrap();
        assert_ne!(campaign_workspace_sha256(&root).unwrap(), baseline);

        reset_suite_workspace(&root, &suite).unwrap();
        assert_eq!(campaign_workspace_sha256(&root).unwrap(), baseline);
        assert!(!root.join("wg5/out.json").exists());
    }
}
