use super::direct_finalizer_campaign_support::{require_clean_source, required_external_path};
use super::setup::{activate_evaluation_data_root, build_evaluation_app};
use super::workflow_gepa_campaign_contract::{
    aggregate_pairs, test_gate, validate_grounded_control, validation_gate, CampaignSplit,
    PairAggregateReceipt, ProductPairReceipt, ProductRunReceipt, CAMPAIGN_PROJECT_ID,
    TEST_REPLICATES,
};
use super::workflow_gepa_campaign_evidence::route_treatment_reflection_packets;
use super::workflow_gepa_campaign_execution::{
    execute_journaled_campaign_case, execute_matched_route_pair, execute_product_pair,
    preflight_matched_workspaces, EvaluationDataEnvironment,
};
use super::workflow_gepa_campaign_journal::{
    CampaignBudgetReceipt, CampaignJournal, CampaignUsageReceipt,
};
use super::workflow_gepa_campaign_suite::{
    cases_for_split, training_dataset_sha256, validate_campaign_suite,
};
use super::workflow_gepa_candidate_search::{
    build_training_receipt, generate_candidate_population, select_training_candidate,
    CandidateIdentityReceipt, CandidateTrainingReceipt,
};
use super::{materialize_case, RealworldSuite, Treatment};
use crate::app_state::AppState;
use crate::collaboration_execution::collaboration_candidate_models;
use crate::configuration_models::SidecarConfig;
use crate::configuration_persistence::load_provider_config;
use agent_core::Metadata;
use orchestrator::{
    assess_prompt_learning_readiness, sha256_hex, AgentPolicy, ConductorPromptGenome,
    PromptLearningLayer, PromptLearningReadinessReceipt,
};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::Manager;

const REPORT_SCHEMA: &str = "cindx.workflow-gepa-product-evidence.v9";

#[derive(Debug, Serialize)]
struct WorkflowGepaCampaignReceipt {
    schema: &'static str,
    source_commit: String,
    suite_id: String,
    suite_sha256: String,
    training_dataset_sha256: String,
    provider_id: String,
    provider_endpoint_sha256: String,
    configured_model_sha256: Vec<String>,
    campaign_budget: CampaignBudgetReceipt,
    campaign_usage: CampaignUsageReceipt,
    train_runs: Vec<ProductRunReceipt>,
    route_treatment_pairs: Vec<ProductPairReceipt>,
    route_treatment_signal: bool,
    learning_readiness: PromptLearningReadinessReceipt,
    reflection_evidence_sha256: String,
    candidate_population: Vec<CandidateTrainingReceipt>,
    selected_candidate: Option<CandidateIdentityReceipt>,
    validation_pairs: Vec<ProductPairReceipt>,
    validation: Option<PairAggregateReceipt>,
    validation_gate_passed: bool,
    test_pairs: Vec<ProductPairReceipt>,
    test: Option<PairAggregateReceipt>,
    test_gate_passed: bool,
    grounded_direct_control: Option<ProductRunReceipt>,
    grounded_direct_control_passed: bool,
    final_test_was_untouched_during_learning: bool,
    candidate_selected_from_train_only: bool,
    candidate_snapshot_published: bool,
    production_promotion_claimed: bool,
    status: String,
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
        .unwrap_or_else(|| repo_root.join("benchmarks/agent/workflow-gepa-v7.json"));
    let suite_bytes = fs::read(&suite_path)
        .map_err(|error| format!("failed to read {}: {error}", suite_path.display()))?;
    let suite: RealworldSuite = serde_json::from_slice(&suite_bytes)
        .map_err(|error| format!("invalid Workflow GEPA suite JSON: {error}"))?;
    validate_campaign_suite(&suite)?;
    let suite_sha256 = sha256_hex(&suite_bytes);
    let training_dataset_sha256 = training_dataset_sha256(&suite)?;
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
    let journal_path = required_external_path(
        "CINDX_WORKFLOW_GEPA_JOURNAL",
        &repo_root,
        "workflow GEPA journal",
    )?;
    if report_path == snapshot_path || report_path == journal_path || snapshot_path == journal_path
    {
        return Err(
            "workflow GEPA report, snapshot, and journal paths must be distinct".to_string(),
        );
    }
    if report_path.exists() || snapshot_path.exists() || journal_path.exists() {
        return Err(
            "workflow GEPA report, snapshot, and journal paths must be new for each authorized run"
                .to_string(),
        );
    }

    let provider = load_provider_config();
    if !provider.is_ready() {
        return Err("configured provider is required; campaign will not synthesize results".into());
    }
    let policy = AgentPolicy::Pro;
    let configured_models = collaboration_candidate_models(&provider, policy.max_parallelism());
    if configured_models.len() < 2 {
        return Err(
            "Workflow GEPA campaign requires at least two distinct configured models".into(),
        );
    }
    let provider_endpoint_sha256 = sha256_hex(provider.base_url.as_bytes());
    let configured_model_sha256 = configured_models
        .iter()
        .map(|model| sha256_hex(model.as_bytes()))
        .collect::<Vec<_>>();
    let mut journal = CampaignJournal::create(
        &journal_path,
        source_commit.clone(),
        suite_sha256.clone(),
        provider_endpoint_sha256.clone(),
        configured_model_sha256.clone(),
    )?;

    let temp = tempfile::Builder::new()
        .prefix("cindx-workflow-gepa-v9-route-")
        .tempdir()
        .map_err(|error| format!("failed to create campaign workspace: {error}"))?;
    let suite_root = temp.path();
    preflight_matched_workspaces(suite_root, &suite)?;
    let _evaluation_data_environment = EvaluationDataEnvironment::install(suite_root);
    let evaluation_database = activate_evaluation_data_root(suite_root)?;
    let sidecars = SidecarConfig::default();
    super::apply_sidecar_env(&sidecars);
    let mut runtime_provider = provider.clone();
    runtime_provider.prompt_evolution_enabled = false;
    let app = build_evaluation_app(
        runtime_provider.clone(),
        sidecars,
        &suite_root.join("workspace"),
        &evaluation_database,
    )?;
    let state = app.state::<AppState>();
    let seed_profile = ConductorPromptGenome::seed_for_effort(policy.label());
    let mut execution_index = 1usize;

    let mut train_route_pairs = Vec::new();
    let mut train_runs = Vec::new();
    for (pair_index, case) in cases_for_split(&suite, CampaignSplit::Train)?
        .into_iter()
        .enumerate()
    {
        eprintln!("[workflow-gepa-v9] matched route train pair: {}", case.id);
        let pair = execute_matched_route_pair(
            &app,
            &state,
            &runtime_provider,
            &evaluation_database,
            suite_root,
            case,
            CampaignSplit::Train,
            1,
            pair_index,
            &mut execution_index,
            &suite_sha256,
            &mut journal,
        )?;
        train_runs.push(pair.pair.seed.clone());
        train_runs.push(pair.pair.candidate.clone());
        train_route_pairs.push((pair_index + 1, pair));
    }

    let reflection_packets =
        route_treatment_reflection_packets(&suite, &seed_profile, &train_route_pairs)?;
    let route_treatment_pairs = train_route_pairs
        .iter()
        .map(|(_, pair)| pair.pair.clone())
        .collect::<Vec<_>>();
    let route_treatment_signal = route_treatment_pairs
        .iter()
        .any(|pair| pair.outcome == "candidate_win");
    let reflection_evidence_sha256 = sha256_hex(
        &serde_json::to_vec(&reflection_packets)
            .map_err(|error| format!("failed to encode reflection evidence: {error}"))?,
    );
    let diagnostic_plans = train_runs
        .iter()
        .map(ProductRunReceipt::execution_diagnostic)
        .collect::<Result<Vec<_>, String>>()?;
    let seed_route_profile_sha256 = seed_profile.route_decision_profile_sha256(policy.label())?;
    let learning_readiness = assess_prompt_learning_readiness(
        PromptLearningLayer::RouteDecision,
        Some(&seed_route_profile_sha256),
        None,
        &diagnostic_plans,
    )?;
    if !learning_readiness.eligible {
        let status =
            if learning_readiness.reason == "learning_layer_is_dormant_for_all_frozen_plans" {
                "valid_no_go_dormant_learning_layer"
            } else {
                "invalid_learning_diagnostic_identity"
            };
        let readiness_reason = learning_readiness.reason.clone();
        let receipt = WorkflowGepaCampaignReceipt {
            schema: REPORT_SCHEMA,
            source_commit,
            suite_id: suite.id,
            suite_sha256,
            training_dataset_sha256,
            provider_id: provider.provider_id,
            provider_endpoint_sha256,
            configured_model_sha256,
            campaign_budget: journal.budget(),
            campaign_usage: journal.usage(),
            train_runs,
            route_treatment_pairs,
            route_treatment_signal,
            learning_readiness,
            reflection_evidence_sha256,
            candidate_population: Vec::new(),
            selected_candidate: None,
            validation_pairs: Vec::new(),
            validation: None,
            validation_gate_passed: false,
            test_pairs: Vec::new(),
            test: None,
            test_gate_passed: false,
            grounded_direct_control: None,
            grounded_direct_control_passed: false,
            final_test_was_untouched_during_learning: true,
            candidate_selected_from_train_only: false,
            candidate_snapshot_published: false,
            production_promotion_claimed: false,
            status: status.to_string(),
        };
        let report_sha256 = write_report(&report_path, &receipt)?;
        journal.finish(&receipt.status, report_sha256)?;
        return Err(format!(
            "workflow GEPA stopped before mutation: {readiness_reason}"
        ));
    }
    if !route_treatment_signal {
        let receipt = WorkflowGepaCampaignReceipt {
            schema: REPORT_SCHEMA,
            source_commit,
            suite_id: suite.id,
            suite_sha256,
            training_dataset_sha256,
            provider_id: provider.provider_id,
            provider_endpoint_sha256,
            configured_model_sha256,
            campaign_budget: journal.budget(),
            campaign_usage: journal.usage(),
            train_runs,
            route_treatment_pairs,
            route_treatment_signal,
            learning_readiness,
            reflection_evidence_sha256,
            candidate_population: Vec::new(),
            selected_candidate: None,
            validation_pairs: Vec::new(),
            validation: None,
            validation_gate_passed: false,
            test_pairs: Vec::new(),
            test: None,
            test_gate_passed: false,
            grounded_direct_control: None,
            grounded_direct_control_passed: false,
            final_test_was_untouched_during_learning: true,
            candidate_selected_from_train_only: false,
            candidate_snapshot_published: false,
            production_promotion_claimed: false,
            status: "valid_no_go_no_positive_workflow_treatment".to_string(),
        };
        let report_sha256 = write_report(&report_path, &receipt)?;
        journal.finish(&receipt.status, report_sha256)?;
        return Err(
            "matched forced Workflow produced no verified training quality win over forced Direct"
                .to_string(),
        );
    }
    let run_context = mutation_run_context();
    journal.begin_mutation_search()?;
    let generated_population = generate_candidate_population(
        &state,
        &provider,
        &run_context,
        &seed_profile,
        &reflection_packets,
        &training_dataset_sha256,
        &reflection_evidence_sha256,
    )?;
    journal.complete_mutation_search(
        generated_population.model_calls,
        generated_population.physical_attempts,
        generated_population.total_tokens,
    )?;
    let candidates = generated_population.candidates;
    let candidate_snapshot_root = suite_root.join("candidate-snapshots");
    fs::create_dir_all(&candidate_snapshot_root).map_err(|error| {
        format!(
            "failed to create candidate snapshot root {}: {error}",
            candidate_snapshot_root.display()
        )
    })?;
    let train_cases = cases_for_split(&suite, CampaignSplit::Train)?;
    let mut candidate_population = Vec::with_capacity(candidates.len());
    for (candidate_index, generated) in candidates.iter().enumerate() {
        let internal_snapshot_path = candidate_snapshot_root.join(format!(
            "candidate-{}-{}.json",
            candidate_index + 1,
            &generated.identity.profile_sha256[..16]
        ));
        let snapshot_bytes = serde_json::to_vec_pretty(&generated.snapshot)
            .map_err(|error| format!("failed to encode train candidate snapshot: {error}"))?;
        tools::write_private_file_atomically(&internal_snapshot_path, &snapshot_bytes)
            .map_err(|error| format!("failed to write train candidate snapshot: {error}"))?;
        let mut train_pairs = Vec::with_capacity(train_cases.len());
        for (case_index, case) in train_cases.iter().enumerate() {
            eprintln!(
                "[workflow-gepa-v9] train candidate={}: {}",
                candidate_index + 1,
                case.id
            );
            train_pairs.push(execute_product_pair(
                &app,
                &state,
                &runtime_provider,
                &evaluation_database,
                suite_root,
                &format!("candidate-{}", candidate_index + 1),
                case,
                CampaignSplit::Train,
                1,
                candidate_index * train_cases.len() + case_index,
                &mut execution_index,
                &suite_sha256,
                &internal_snapshot_path,
                &generated.identity.snapshot_artifact_sha256,
                &generated.snapshot,
                &mut journal,
            )?);
        }
        candidate_population.push(build_training_receipt(generated, train_pairs)?);
    }
    let selected_profile_id =
        match select_training_candidate(&candidates, &mut candidate_population) {
            Ok(profile_id) => profile_id,
            Err(error) => {
                let receipt = WorkflowGepaCampaignReceipt {
                    schema: REPORT_SCHEMA,
                    source_commit,
                    suite_id: suite.id,
                    suite_sha256,
                    training_dataset_sha256,
                    provider_id: provider.provider_id,
                    provider_endpoint_sha256: provider_endpoint_sha256.clone(),
                    configured_model_sha256: configured_model_sha256.clone(),
                    campaign_budget: journal.budget(),
                    campaign_usage: journal.usage(),
                    train_runs,
                    route_treatment_pairs,
                    route_treatment_signal,
                    learning_readiness,
                    reflection_evidence_sha256,
                    candidate_population,
                    selected_candidate: None,
                    validation_pairs: Vec::new(),
                    validation: None,
                    validation_gate_passed: false,
                    test_pairs: Vec::new(),
                    test: None,
                    test_gate_passed: false,
                    grounded_direct_control: None,
                    grounded_direct_control_passed: false,
                    final_test_was_untouched_during_learning: true,
                    candidate_selected_from_train_only: false,
                    candidate_snapshot_published: false,
                    production_promotion_claimed: false,
                    status: "valid_no_go_training".to_string(),
                };
                let report_sha256 = write_report(&report_path, &receipt)?;
                journal.finish(&receipt.status, report_sha256)?;
                return Err(error);
            }
        };
    let selected = candidates
        .iter()
        .find(|candidate| candidate.genome.id == selected_profile_id)
        .ok_or_else(|| "selected train candidate is missing from the population".to_string())?;
    let candidate_genome = &selected.genome;
    let snapshot = &selected.snapshot;
    let selected_candidate = selected.identity.clone();
    let snapshot_bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|error| format!("failed to encode selected Workflow GEPA snapshot: {error}"))?;
    let selected_snapshot_path = candidate_snapshot_root.join(format!(
        "selected-{}.json",
        &selected_candidate.snapshot_artifact_sha256[..16]
    ));
    tools::write_private_file_atomically(&selected_snapshot_path, &snapshot_bytes).map_err(
        |error| format!("failed to write internal selected Workflow GEPA snapshot: {error}"),
    )?;
    let snapshot_artifact_sha256 = selected_candidate.snapshot_artifact_sha256.clone();

    let mut validation_pairs = Vec::new();
    for (pair_index, case) in cases_for_split(&suite, CampaignSplit::Validation)?
        .into_iter()
        .enumerate()
    {
        eprintln!("[workflow-gepa-v9] validation pair: {}", case.id);
        validation_pairs.push(execute_product_pair(
            &app,
            &state,
            &runtime_provider,
            &evaluation_database,
            suite_root,
            "selected",
            case,
            CampaignSplit::Validation,
            1,
            pair_index,
            &mut execution_index,
            &suite_sha256,
            &selected_snapshot_path,
            &snapshot_artifact_sha256,
            snapshot,
            &mut journal,
        )?);
    }
    let validation = aggregate_pairs(&validation_pairs, candidate_genome)?;
    let validation_result = validation_gate(&validation);
    if let Err(error) = validation_result {
        let receipt = WorkflowGepaCampaignReceipt {
            schema: REPORT_SCHEMA,
            source_commit,
            suite_id: suite.id,
            suite_sha256,
            training_dataset_sha256,
            provider_id: provider.provider_id,
            provider_endpoint_sha256: provider_endpoint_sha256.clone(),
            configured_model_sha256: configured_model_sha256.clone(),
            campaign_budget: journal.budget(),
            campaign_usage: journal.usage(),
            train_runs,
            route_treatment_pairs,
            route_treatment_signal,
            learning_readiness,
            reflection_evidence_sha256,
            candidate_population,
            selected_candidate: Some(selected_candidate),
            validation_pairs,
            validation: Some(validation),
            validation_gate_passed: false,
            test_pairs: Vec::new(),
            test: None,
            test_gate_passed: false,
            grounded_direct_control: None,
            grounded_direct_control_passed: false,
            final_test_was_untouched_during_learning: true,
            candidate_selected_from_train_only: true,
            candidate_snapshot_published: false,
            production_promotion_claimed: false,
            status: "valid_no_go_validation".to_string(),
        };
        let report_sha256 = write_report(&report_path, &receipt)?;
        journal.finish(&receipt.status, report_sha256)?;
        return Err(error);
    }

    let grounded_case = suite
        .cases
        .iter()
        .find(|case| case.id == "coding-reconcile-records")
        .ok_or_else(|| "campaign Grounded Direct control case is missing".to_string())?;
    let grounded_root = suite_root.join("grounded-direct-control");
    materialize_case(&grounded_root, grounded_case)?;
    let grounded_scope = format!("{CAMPAIGN_PROJECT_ID}-grounded-control");
    let (grounded_raw, grounded_receipt) = execute_journaled_campaign_case(
        &app,
        &state,
        &runtime_provider,
        &evaluation_database,
        &grounded_root,
        grounded_case,
        execution_index,
        1,
        1,
        &suite_sha256,
        None,
        Treatment::GroundedDirect,
        &grounded_scope,
        CampaignSplit::Train,
        &format!("{grounded_scope}-grounded-control"),
        &mut journal,
    )?;
    execution_index = execution_index.saturating_add(1);
    let grounded_result = validate_grounded_control(&grounded_raw);

    let mut test_pairs = Vec::new();
    if grounded_result.is_ok() {
        let mut pair_index = 0usize;
        for replicate in 1..=TEST_REPLICATES {
            for case in cases_for_split(&suite, CampaignSplit::Test)? {
                eprintln!(
                    "[workflow-gepa-v9] untouched test pair replicate={replicate}: {}",
                    case.id
                );
                test_pairs.push(execute_product_pair(
                    &app,
                    &state,
                    &runtime_provider,
                    &evaluation_database,
                    suite_root,
                    "selected",
                    case,
                    CampaignSplit::Test,
                    replicate,
                    pair_index,
                    &mut execution_index,
                    &suite_sha256,
                    &selected_snapshot_path,
                    &snapshot_artifact_sha256,
                    snapshot,
                    &mut journal,
                )?);
                pair_index = pair_index.saturating_add(1);
            }
        }
    }
    let test = if test_pairs.is_empty() {
        None
    } else {
        Some(aggregate_pairs(&test_pairs, candidate_genome)?)
    };
    let test_result = test
        .as_ref()
        .map(test_gate)
        .unwrap_or_else(|| Err("untouched product test was not run".to_string()));
    let passed = grounded_result.is_ok() && test_result.is_ok();
    let status = if passed {
        "valid_candidate_product_uplift"
    } else if grounded_result.is_err() {
        "valid_no_go_grounded_control"
    } else {
        "valid_no_go_product_test"
    };
    if passed {
        tools::write_private_file_atomically(&snapshot_path, &snapshot_bytes)
            .map_err(|error| format!("failed to publish passed Workflow GEPA snapshot: {error}"))?;
    }
    let receipt = WorkflowGepaCampaignReceipt {
        schema: REPORT_SCHEMA,
        source_commit,
        suite_id: suite.id,
        suite_sha256,
        training_dataset_sha256,
        provider_id: provider.provider_id,
        provider_endpoint_sha256,
        configured_model_sha256,
        campaign_budget: journal.budget(),
        campaign_usage: journal.usage(),
        train_runs,
        route_treatment_pairs,
        route_treatment_signal,
        learning_readiness,
        reflection_evidence_sha256,
        candidate_population,
        selected_candidate: Some(selected_candidate),
        validation_pairs,
        validation: Some(validation),
        validation_gate_passed: true,
        test_pairs,
        test,
        test_gate_passed: test_result.is_ok(),
        grounded_direct_control: Some(grounded_receipt),
        grounded_direct_control_passed: grounded_result.is_ok(),
        final_test_was_untouched_during_learning: true,
        candidate_selected_from_train_only: true,
        candidate_snapshot_published: passed,
        production_promotion_claimed: false,
        status: status.to_string(),
    };
    let report_sha256 = write_report(&report_path, &receipt)?;
    journal.finish(&receipt.status, report_sha256)?;
    grounded_result?;
    test_result?;
    eprintln!(
        "[workflow-gepa-v9] passed report={} snapshot={}",
        report_path.display(),
        snapshot_path.display()
    );
    Ok(())
}

fn mutation_run_context() -> Metadata {
    [
        ("project_id".to_string(), CAMPAIGN_PROJECT_ID.to_string()),
        (
            "session_id".to_string(),
            "session-workflow-gepa-v9".to_string(),
        ),
        ("effort".to_string(), AgentPolicy::Pro.label().to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect()
}

fn write_report(path: &Path, receipt: &WorkflowGepaCampaignReceipt) -> Result<String, String> {
    let encoded = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("failed to encode Workflow GEPA report: {error}"))?;
    tools::write_private_file_atomically(path, &encoded)
        .map_err(|error| format!("failed to write Workflow GEPA report: {error}"))?;
    Ok(sha256_hex(&encoded))
}
