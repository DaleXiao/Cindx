use super::direct_finalizer_campaign_support::{require_clean_source, required_external_path};
use super::setup::{activate_evaluation_data_root, build_evaluation_app};
use super::workflow_gepa_campaign_contract::{
    aggregate_pairs, test_gate, validate_grounded_control, validation_gate, CampaignSplit,
    PairAggregateReceipt, ProductPairReceipt, ProductRunReceipt, CAMPAIGN_PROJECT_ID,
    TEST_REPLICATES,
};
use super::workflow_gepa_campaign_evidence::training_reflection_packets;
use super::workflow_gepa_campaign_execution::{
    execute_campaign_case, execute_product_pair, project_scope, EvaluationDataEnvironment,
};
use super::workflow_gepa_campaign_suite::{
    cases_for_split, training_dataset_sha256, validate_campaign_suite,
};
use super::{materialize_case, RealworldSuite, Treatment};
use crate::app_state::AppState;
use crate::collaboration_execution::collaboration_candidate_models;
use crate::configuration_models::{ProviderConfig, SidecarConfig};
use crate::configuration_persistence::load_provider_config;
use crate::phase16_task_id;
use crate::prompt_learning_runtime::prompt_evaluation_parent_budget;
use crate::prompt_mutation_runtime::run_background_prompt_mutation_stage;
use agent_core::Metadata;
use agent_runtime::AgentRunControl;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, AgentEvaluationReflectionPacket, AgentPolicy,
    ConductorPromptGenome, FrozenPromptProfileSnapshot,
};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::Manager;

const REPORT_SCHEMA: &str = "cindx.workflow-gepa-product-evidence.v4";

#[derive(Debug, Serialize)]
struct CandidateReceipt {
    profile_id: String,
    profile_sha256: String,
    route_profile_sha256: String,
    parent_profile_id: String,
    generation: u32,
    mutation_response_sha256: String,
    mutation_repaired: bool,
    snapshot_artifact_sha256: String,
}

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
    train_runs: Vec<ProductRunReceipt>,
    reflection_evidence_sha256: String,
    candidate: CandidateReceipt,
    validation_pairs: Vec<ProductPairReceipt>,
    validation: PairAggregateReceipt,
    validation_gate_passed: bool,
    test_pairs: Vec<ProductPairReceipt>,
    test: Option<PairAggregateReceipt>,
    test_gate_passed: bool,
    grounded_direct_control: Option<ProductRunReceipt>,
    grounded_direct_control_passed: bool,
    final_test_was_untouched_during_learning: bool,
    candidate_selected_without_test_evidence: bool,
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
        .unwrap_or_else(|| repo_root.join("benchmarks/agent/workflow-gepa-v4.json"));
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

    let temp = tempfile::Builder::new()
        .prefix("cindx-workflow-gepa-v4-")
        .tempdir()
        .map_err(|error| format!("failed to create campaign workspace: {error}"))?;
    let suite_root = temp.path();
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

    let mut train_raw = Vec::new();
    let mut train_runs = Vec::new();
    for case in cases_for_split(&suite, CampaignSplit::Train)? {
        eprintln!("[workflow-gepa-v4] train seed: {}", case.id);
        let run_execution_index = execution_index;
        let root = suite_root.join(format!("train-{}", case.id));
        materialize_case(&root, case)?;
        let project_scope = project_scope(CampaignSplit::Train, case, 1, "seed");
        let run = execute_campaign_case(
            &app,
            &state,
            &runtime_provider,
            &evaluation_database,
            &root,
            case,
            run_execution_index,
            1,
            1,
            &suite_sha256,
            None,
            Treatment::Pro,
            &project_scope,
        );
        execution_index = execution_index.saturating_add(1);
        train_runs.push(ProductRunReceipt::from_run(
            &run,
            CampaignSplit::Train,
            1,
        )?);
        train_raw.push((run_execution_index, run));
    }

    let reflection_packets = training_reflection_packets(&suite, &seed_profile, &train_raw)?;
    let reflection_evidence_sha256 = sha256_hex(
        &serde_json::to_vec(&reflection_packets)
            .map_err(|error| format!("failed to encode reflection evidence: {error}"))?,
    );
    let run_context = mutation_run_context();
    let (candidate_genome, mutation_response_sha256, mutation_repaired) = generate_candidate(
        &state,
        &provider,
        &run_context,
        &seed_profile,
        &reflection_packets,
    )?;
    let seed_route_sha256 = seed_profile.route_decision_profile_sha256(policy.label())?;
    let candidate_route_sha256 = candidate_genome.route_decision_profile_sha256(policy.label())?;
    if candidate_route_sha256 == seed_route_sha256 {
        return Err("GEPA mutation did not change the route-decision phenotype".to_string());
    }
    let snapshot = FrozenPromptProfileSnapshot::new_gepa(
        policy.label(),
        candidate_genome.clone(),
        seed_profile.id.clone(),
        training_dataset_sha256.clone(),
        reflection_evidence_sha256.clone(),
    )?;
    let snapshot_bytes = serde_json::to_vec_pretty(&snapshot)
        .map_err(|error| format!("failed to encode Workflow GEPA snapshot: {error}"))?;
    tools::write_private_file_atomically(&snapshot_path, &snapshot_bytes)
        .map_err(|error| format!("failed to write Workflow GEPA snapshot: {error}"))?;
    let snapshot_artifact_sha256 = snapshot.artifact_sha256()?;
    let candidate = CandidateReceipt {
        profile_id: candidate_genome.id.clone(),
        profile_sha256: prompt_genome_sha256(&candidate_genome)?,
        route_profile_sha256: candidate_route_sha256,
        parent_profile_id: seed_profile.id.clone(),
        generation: candidate_genome.generation,
        mutation_response_sha256,
        mutation_repaired,
        snapshot_artifact_sha256: snapshot_artifact_sha256.clone(),
    };

    let mut validation_pairs = Vec::new();
    for (pair_index, case) in cases_for_split(&suite, CampaignSplit::Validation)?
        .into_iter()
        .enumerate()
    {
        eprintln!("[workflow-gepa-v4] validation pair: {}", case.id);
        validation_pairs.push(execute_product_pair(
            &app,
            &state,
            &runtime_provider,
            &evaluation_database,
            suite_root,
            case,
            CampaignSplit::Validation,
            1,
            pair_index,
            &mut execution_index,
            &suite_sha256,
            &snapshot_path,
            &snapshot_artifact_sha256,
            &snapshot,
        )?);
    }
    let validation = aggregate_pairs(&validation_pairs, &candidate_genome)?;
    let validation_result = validation_gate(&validation);
    if let Err(error) = validation_result {
        let receipt = WorkflowGepaCampaignReceipt {
            schema: REPORT_SCHEMA,
            source_commit,
            suite_id: suite.id,
            suite_sha256,
            training_dataset_sha256,
            provider_id: provider.provider_id,
            provider_endpoint_sha256: sha256_hex(provider.base_url.as_bytes()),
            configured_model_sha256: configured_models
                .iter()
                .map(|model| sha256_hex(model.as_bytes()))
                .collect(),
            train_runs,
            reflection_evidence_sha256,
            candidate,
            validation_pairs,
            validation,
            validation_gate_passed: false,
            test_pairs: Vec::new(),
            test: None,
            test_gate_passed: false,
            grounded_direct_control: None,
            grounded_direct_control_passed: false,
            final_test_was_untouched_during_learning: true,
            candidate_selected_without_test_evidence: true,
            production_promotion_claimed: false,
            status: "valid_no_go_validation".to_string(),
        };
        write_report(&report_path, &receipt)?;
        return Err(error);
    }

    let grounded_case = suite
        .cases
        .iter()
        .find(|case| case.id == "coding-calculate-total")
        .ok_or_else(|| "campaign Grounded Direct control case is missing".to_string())?;
    let grounded_root = suite_root.join("grounded-direct-control");
    materialize_case(&grounded_root, grounded_case)?;
    let grounded_scope = format!("{CAMPAIGN_PROJECT_ID}-grounded-control");
    let grounded_raw = execute_campaign_case(
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
    );
    execution_index = execution_index.saturating_add(1);
    let grounded_receipt = ProductRunReceipt::from_run(
        &grounded_raw,
        CampaignSplit::Train,
        1,
    )?;
    let grounded_result = validate_grounded_control(&grounded_raw);

    let mut test_pairs = Vec::new();
    if grounded_result.is_ok() {
        let mut pair_index = 0usize;
        for replicate in 1..=TEST_REPLICATES {
            for case in cases_for_split(&suite, CampaignSplit::Test)? {
                eprintln!(
                    "[workflow-gepa-v4] untouched test pair replicate={replicate}: {}",
                    case.id
                );
                test_pairs.push(execute_product_pair(
                    &app,
                    &state,
                    &runtime_provider,
                    &evaluation_database,
                    suite_root,
                    case,
                    CampaignSplit::Test,
                    replicate,
                    pair_index,
                    &mut execution_index,
                    &suite_sha256,
                    &snapshot_path,
                    &snapshot_artifact_sha256,
                    &snapshot,
                )?);
                pair_index = pair_index.saturating_add(1);
            }
        }
    }
    let test = if test_pairs.is_empty() {
        None
    } else {
        Some(aggregate_pairs(&test_pairs, &candidate_genome)?)
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
    let receipt = WorkflowGepaCampaignReceipt {
        schema: REPORT_SCHEMA,
        source_commit,
        suite_id: suite.id,
        suite_sha256,
        training_dataset_sha256,
        provider_id: provider.provider_id,
        provider_endpoint_sha256: sha256_hex(provider.base_url.as_bytes()),
        configured_model_sha256: configured_models
            .iter()
            .map(|model| sha256_hex(model.as_bytes()))
            .collect(),
        train_runs,
        reflection_evidence_sha256,
        candidate,
        validation_pairs,
        validation,
        validation_gate_passed: true,
        test_pairs,
        test,
        test_gate_passed: test_result.is_ok(),
        grounded_direct_control: Some(grounded_receipt),
        grounded_direct_control_passed: grounded_result.is_ok(),
        final_test_was_untouched_during_learning: true,
        candidate_selected_without_test_evidence: true,
        production_promotion_claimed: false,
        status: status.to_string(),
    };
    write_report(&report_path, &receipt)?;
    if let Err(error) = grounded_result {
        return Err(error);
    }
    if let Err(error) = test_result {
        return Err(error);
    }
    eprintln!(
        "[workflow-gepa-v4] passed report={} snapshot={}",
        report_path.display(),
        snapshot_path.display()
    );
    Ok(())
}

fn generate_candidate(
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    run_context: &Metadata,
    parent: &ConductorPromptGenome,
    packets: &[AgentEvaluationReflectionPacket],
) -> Result<(ConductorPromptGenome, String, bool), String> {
    let control = Arc::new(AgentRunControl::with_budget(
        prompt_evaluation_parent_budget(),
    ));
    let prompt = parent.reflective_mutation_prompt(packets)?;
    let response = run_background_prompt_mutation_stage(
        state,
        provider,
        &phase16_task_id(),
        run_context,
        "workflow-gepa-v4-mutation",
        "workflow_gepa_v4_mutation",
        prompt,
        &control,
    )?;
    let initial_sha256 = sha256_hex(response.as_bytes());
    let initial_id = format!("learned-pro-v4-{}", &initial_sha256[..16]);
    match parent.learned_reflective_mutation_from_response(&response, initial_id, packets) {
        Ok(candidate) => Ok((candidate, initial_sha256, false)),
        Err(initial_error) => {
            let repaired = run_background_prompt_mutation_stage(
                state,
                provider,
                &phase16_task_id(),
                run_context,
                "workflow-gepa-v4-mutation-repair",
                "workflow_gepa_v4_mutation_repair",
                parent.mutation_repair_prompt(&response, &initial_error),
                &control,
            )?;
            let repaired_sha256 = sha256_hex(repaired.as_bytes());
            let repaired_id = format!("learned-pro-v4-{}", &repaired_sha256[..16]);
            let candidate = parent.learned_reflective_mutation_from_response(
                &repaired,
                repaired_id,
                packets,
            )?;
            Ok((candidate, repaired_sha256, true))
        }
    }
}

fn mutation_run_context() -> Metadata {
    [
        ("project_id".to_string(), CAMPAIGN_PROJECT_ID.to_string()),
        (
            "session_id".to_string(),
            "session-workflow-gepa-v4".to_string(),
        ),
        ("effort".to_string(), AgentPolicy::Pro.label().to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect()
}

fn write_report(path: &Path, receipt: &WorkflowGepaCampaignReceipt) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("failed to encode Workflow GEPA report: {error}"))?;
    tools::write_private_file_atomically(path, &encoded)
        .map_err(|error| format!("failed to write Workflow GEPA report: {error}"))
}
