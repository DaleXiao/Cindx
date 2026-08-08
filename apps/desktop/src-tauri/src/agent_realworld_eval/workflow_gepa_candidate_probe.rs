use super::direct_finalizer_campaign_support::{
    require_clean_source, required_external_path,
};
use super::setup::{activate_evaluation_data_root, build_evaluation_app};
use super::workflow_gepa_campaign_execution::EvaluationDataEnvironment;
use super::workflow_gepa_candidate_search::{
    generate_candidate_population, CandidateIdentityReceipt, GeneratedCandidate,
    TARGET_CANDIDATE_POPULATION,
};
use crate::app_state::AppState;
use crate::configuration_models::SidecarConfig;
use crate::configuration_persistence::load_provider_config;
use agent_core::Metadata;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, AgentEvaluationReflectionPacket, AgentPolicy,
    ConductorPromptGenome, FrozenPromptProfileSnapshot,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

const INPUT_SCHEMA: &str = "cindx.workflow-gepa-candidate-input.v1";
const OUTPUT_SCHEMA: &str = "cindx.workflow-gepa-candidate-probe.v1";
const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REFLECTION_PACKETS: usize = 64;

#[derive(Debug, Deserialize)]
struct CandidateProbeInput {
    schema: String,
    packets: Vec<AgentEvaluationReflectionPacket>,
}

#[derive(Debug, Serialize)]
struct CandidateProbeArtifact {
    identity: CandidateIdentityReceipt,
    genome: ConductorPromptGenome,
    snapshot: FrozenPromptProfileSnapshot,
}

#[derive(Debug, Serialize)]
struct CandidateProbeReceipt {
    schema: &'static str,
    status: &'static str,
    source_commit: String,
    input_sha256: String,
    reflection_evidence_sha256: String,
    reflection_packet_count: usize,
    provider_id: String,
    provider_endpoint_sha256: String,
    parent_profile_id: String,
    parent_profile_sha256: String,
    model_calls: usize,
    physical_attempts: u64,
    total_tokens: u64,
    candidates: Vec<CandidateProbeArtifact>,
    production_profile_changed: bool,
    capability_claimed: bool,
}

pub(super) fn run() -> Result<(), String> {
    super::install_eval_crypto_provider();
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))?;
    let source_commit = require_clean_source(&repo_root)?;
    let input_path = required_external_path(
        "CINDX_WORKFLOW_GEPA_REFLECTIONS",
        &repo_root,
        "workflow GEPA reflection input",
    )?;
    let output_path = required_external_path(
        "CINDX_WORKFLOW_GEPA_CANDIDATES",
        &repo_root,
        "workflow GEPA candidate output",
    )?;
    if input_path == output_path {
        return Err("reflection input and candidate output paths must be distinct".to_string());
    }
    if output_path.exists() {
        return Err("candidate output path must be new for each probe".to_string());
    }
    let input_metadata = fs::metadata(&input_path).map_err(|error| {
        format!(
            "failed to inspect reflection input {}: {error}",
            input_path.display()
        )
    })?;
    if input_metadata.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "reflection input exceeds the {} byte limit",
            MAX_INPUT_BYTES
        ));
    }
    let input_bytes = fs::read(&input_path).map_err(|error| {
        format!(
            "failed to read reflection input {}: {error}",
            input_path.display()
        )
    })?;
    let input: CandidateProbeInput = serde_json::from_slice(&input_bytes)
        .map_err(|error| format!("invalid candidate-probe input JSON: {error}"))?;
    validate_input(&input)?;

    let provider = load_provider_config();
    if !provider.is_ready() {
        return Err("configured provider is required; probe will not synthesize results".into());
    }
    let redaction_secrets = vec![provider.api_key.clone()];
    if input.packets.iter().any(|packet| {
        !crate::prompt_evaluation_feedback::prompt_reflection_packet_is_safe(
            packet,
            &redaction_secrets,
        )
    }) {
        return Err("reflection input contains a configured or residual secret".to_string());
    }

    let input_sha256 = sha256_hex(&input_bytes);
    let reflection_evidence_sha256 = sha256_hex(
        &serde_json::to_vec(&input.packets)
            .map_err(|error| format!("failed to encode reflection evidence: {error}"))?,
    );
    let parent = ConductorPromptGenome::seed_for_effort(AgentPolicy::Pro.label());
    let parent_profile_sha256 = prompt_genome_sha256(&parent)?;

    let temp = tempfile::Builder::new()
        .prefix("cindx-workflow-gepa-candidate-probe-")
        .tempdir()
        .map_err(|error| format!("failed to create candidate-probe workspace: {error}"))?;
    let probe_root = temp.path();
    let _evaluation_data_environment = EvaluationDataEnvironment::install(probe_root);
    let evaluation_database = activate_evaluation_data_root(probe_root)?;
    let sidecars = SidecarConfig::default();
    super::apply_sidecar_env(&sidecars);
    let mut runtime_provider = provider.clone();
    runtime_provider.prompt_evolution_enabled = false;
    let app = build_evaluation_app(
        runtime_provider,
        sidecars,
        &probe_root.join("workspace"),
        &evaluation_database,
    )?;
    let state = app.state::<AppState>();
    let generated = generate_candidate_population(
        &state,
        &provider,
        &mutation_run_context(),
        &parent,
        &input.packets,
        &input_sha256,
        &reflection_evidence_sha256,
    )?;
    validate_generated_population(&generated.candidates, &parent)?;

    let receipt = CandidateProbeReceipt {
        schema: OUTPUT_SCHEMA,
        status: "DIAGNOSTIC_CANDIDATE_GENERATION_ONLY",
        source_commit,
        input_sha256,
        reflection_evidence_sha256,
        reflection_packet_count: input.packets.len(),
        provider_id: provider.provider_id.clone(),
        provider_endpoint_sha256: sha256_hex(provider.base_url.as_bytes()),
        parent_profile_id: parent.id.clone(),
        parent_profile_sha256,
        model_calls: generated.model_calls,
        physical_attempts: generated.physical_attempts,
        total_tokens: generated.total_tokens,
        candidates: generated
            .candidates
            .into_iter()
            .map(|candidate| CandidateProbeArtifact {
                identity: candidate.identity,
                genome: candidate.genome,
                snapshot: candidate.snapshot,
            })
            .collect(),
        production_profile_changed: false,
        capability_claimed: false,
    };
    let encoded = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| format!("failed to encode candidate-probe receipt: {error}"))?;
    tools::write_private_file_atomically(&output_path, &encoded)
        .map_err(|error| format!("failed to write candidate-probe receipt: {error}"))?;
    eprintln!(
        "[workflow-gepa-candidate-probe] private receipt: {}",
        output_path.display()
    );
    Ok(())
}

fn validate_input(input: &CandidateProbeInput) -> Result<(), String> {
    if input.schema != INPUT_SCHEMA {
        return Err(format!(
            "unsupported candidate-probe input schema {}",
            input.schema
        ));
    }
    if input.packets.is_empty() || input.packets.len() > MAX_REFLECTION_PACKETS {
        return Err(format!(
            "candidate probe requires 1..={} reflection packets",
            MAX_REFLECTION_PACKETS
        ));
    }
    Ok(())
}

fn validate_generated_population(
    candidates: &[GeneratedCandidate],
    parent: &ConductorPromptGenome,
) -> Result<(), String> {
    if candidates.len() != TARGET_CANDIDATE_POPULATION {
        return Err(format!(
            "candidate probe produced {} candidates; {} required",
            candidates.len(),
            TARGET_CANDIDATE_POPULATION
        ));
    }
    let mut profile_hashes = BTreeSet::new();
    let mut route_hashes = BTreeSet::new();
    for candidate in candidates {
        candidate.snapshot.validate()?;
        let profile_hash = prompt_genome_sha256(&candidate.genome)?;
        if profile_hash != candidate.identity.profile_sha256
            || candidate.snapshot.genome != candidate.genome
            || candidate.snapshot.candidate_sha256 != profile_hash
            || candidate.snapshot.artifact_sha256()?
                != candidate.identity.snapshot_artifact_sha256
            || candidate.identity.parent_profile_id != parent.id
            || !(1..=2).contains(&candidate.identity.mutated_genes.len())
        {
            return Err(format!(
                "candidate {} failed frozen identity validation",
                candidate.identity.profile_id
            ));
        }
        let route_hash = candidate
            .genome
            .route_decision_profile_sha256(AgentPolicy::Pro.label())?;
        if route_hash != candidate.identity.route_profile_sha256 {
            return Err(format!(
                "candidate {} failed route identity validation",
                candidate.identity.profile_id
            ));
        }
        profile_hashes.insert(profile_hash);
        route_hashes.insert(route_hash);
    }
    if profile_hashes.len() != TARGET_CANDIDATE_POPULATION
        || route_hashes.len() != TARGET_CANDIDATE_POPULATION
    {
        return Err("candidate probe population is not structurally distinct".to_string());
    }
    Ok(())
}

fn mutation_run_context() -> Metadata {
    [
        (
            "project_id".to_string(),
            "project-workflow-gepa-candidate-probe".to_string(),
        ),
        (
            "session_id".to_string(),
            "session-workflow-gepa-candidate-probe".to_string(),
        ),
        ("effort".to_string(), AgentPolicy::Pro.label().to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_input_requires_bounded_real_reflection_evidence() {
        let input = CandidateProbeInput {
            schema: INPUT_SCHEMA.to_string(),
            packets: Vec::new(),
        };
        assert_eq!(
            validate_input(&input).unwrap_err(),
            "candidate probe requires 1..=64 reflection packets"
        );

        let wrong_schema = CandidateProbeInput {
            schema: "cindx.workflow-gepa-candidate-input.v0".to_string(),
            packets: Vec::new(),
        };
        assert!(validate_input(&wrong_schema)
            .unwrap_err()
            .starts_with("unsupported candidate-probe input schema"));
    }
}
