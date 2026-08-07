use super::direct_finalizer_campaign_contract::*;
use super::direct_finalizer_campaign_execution::{ensure_pair, evaluate_candidate};
pub(super) use super::direct_finalizer_campaign_support::DirectFinalizerCampaignPairEvidence;
use super::direct_finalizer_campaign_support::{
    base_campaign_receipt, build_learning_cohort, evaluate_gate_a, load_or_initialize_checkpoint,
    profile_receipt, require_clean_source, required_external_path, validate_external_path,
};
use agent_core::ModelRole;
use orchestrator::{sha256_hex, ConductorPromptGenome, PromptVerification};
use std::fs;
use std::path::PathBuf;

pub(super) fn run() -> Result<(), String> {
    super::install_eval_crypto_provider();
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))?;
    let source_commit = require_clean_source(&repo_root)?;
    let suite_path = std::env::var_os("CINDX_DIRECT_FINALIZER_SUITE")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("benchmarks/agent/direct-finalizer-gepa-v1.json"));
    let suite_bytes = fs::read(&suite_path)
        .map_err(|error| format!("failed to read {}: {error}", suite_path.display()))?;
    let suite = parse_and_validate_direct_finalizer_suite(&suite_bytes)?;
    let suite_sha256 = sha256_hex(&suite_bytes);

    let report_path = required_external_path(
        "CINDX_DIRECT_FINALIZER_REPORT",
        &repo_root,
        "campaign report",
    )?;
    let checkpoint_path = std::env::var_os("CINDX_DIRECT_FINALIZER_CHECKPOINT")
        .map(PathBuf::from)
        .unwrap_or_else(|| report_path.with_extension("private-checkpoint.json"));
    validate_external_path(&checkpoint_path, &repo_root, "private checkpoint")?;
    let snapshot_path = std::env::var_os("CINDX_DIRECT_FINALIZER_SNAPSHOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| report_path.with_extension("snapshot.json"));
    validate_external_path(&snapshot_path, &repo_root, "frozen snapshot")?;

    let provider = crate::configuration_persistence::load_provider_config();
    if !provider.is_ready() {
        return Err("configured provider is required; campaign will not synthesize results".into());
    }
    let producer_model = provider.model_for_role(&ModelRole::Summarizer);
    let reviewer_model = provider.model_for_role(&ModelRole::Reviewer);
    let gepa_model = provider.model_for_role(&ModelRole::Planner);
    if producer_model.trim().is_empty()
        || reviewer_model.trim().is_empty()
        || gepa_model.trim().is_empty()
        || producer_model == reviewer_model
    {
        return Err(
            "campaign requires fixed producer/GEPA models and an independent reviewer".into(),
        );
    }

    let parent = ConductorPromptGenome::seed_for_effort("auto");
    let manual_candidate = parent
        .direct_finalizer_mutations()?
        .into_iter()
        .find(|candidate| {
            candidate.direct_finalizer_verification == PromptVerification::Adversarial
        })
        .ok_or_else(|| "adversarial Direct-finalizer calibration variant is missing".to_string())?;
    let parent_profile = profile_receipt(&parent)?;
    let manual_candidate_profile = profile_receipt(&manual_candidate)?;
    let cohort = build_learning_cohort(
        &suite,
        &source_commit,
        &suite_sha256,
        &provider.provider_id,
        &provider.base_url,
        &producer_model,
        &reviewer_model,
        &gepa_model,
        &provider.agent_system_prompt,
    )?;
    let dataset_sha256 = cohort.dataset.dataset_sha256.clone();
    let cohort_sha256 = cohort.cohort_sha256.clone();
    let mut checkpoint = load_or_initialize_checkpoint(
        &checkpoint_path,
        &source_commit,
        &suite_sha256,
        &cohort_sha256,
        &parent_profile,
        &manual_candidate_profile,
        &suite,
    )?;
    eprintln!(
        "[direct-finalizer-gepa] source={} provider={} producer={} reviewer={} gepa={}",
        source_commit, provider.provider_id, producer_model, reviewer_model, gepa_model
    );
    for case in suite.gate_a_cases() {
        ensure_pair(
            &mut checkpoint,
            &checkpoint_path,
            case,
            &provider,
            &producer_model,
            &reviewer_model,
            &parent,
            &manual_candidate,
        )?;
    }
    let gate_a = evaluate_gate_a(&suite, &checkpoint.pairs);
    if !gate_a.passed {
        let receipt = base_campaign_receipt(
            &suite,
            &suite_sha256,
            &dataset_sha256,
            &cohort_sha256,
            &source_commit,
            &provider.provider_id,
            &producer_model,
            &reviewer_model,
            &gepa_model,
            parent_profile,
            Some(manual_candidate_profile.clone()),
            checkpoint.call_count,
            gate_a,
            DirectFinalizerGepaReceipt {
                attempts: 0,
                response_sha256: None,
                decision: None,
            },
            None,
            None,
            None,
            &checkpoint.pairs,
        );
        write_sanitized_direct_finalizer_campaign(&report_path, &receipt)?;
        return Err("manual Direct-finalizer Gate A failed; GEPA and holdout were not run".into());
    }

    for case in suite.train_cases() {
        ensure_pair(
            &mut checkpoint,
            &checkpoint_path,
            case,
            &provider,
            &producer_model,
            &reviewer_model,
            &parent,
            &manual_candidate,
        )?;
    }

    // The evidence module owns the redacted trajectory and scientific-observation projection.
    let producer_model_sha256 = sha256_hex(producer_model.as_bytes());
    let training_packets = super::direct_finalizer_campaign_evidence::reflection_packets(
        &suite,
        &manual_candidate_profile,
        &checkpoint.pairs,
        &producer_model_sha256,
    )?;
    let (gepa_decision, gepa_response_sha256, gepa_attempts) = evaluate_candidate(
        &provider,
        &gepa_model,
        &parent,
        &manual_candidate,
        &training_packets,
        &mut checkpoint,
        &checkpoint_path,
    )?;
    if gepa_decision == orchestrator::DirectFinalizerGepaDecision::Reject {
        let receipt = base_campaign_receipt(
            &suite,
            &suite_sha256,
            &dataset_sha256,
            &cohort_sha256,
            &source_commit,
            &provider.provider_id,
            &producer_model,
            &reviewer_model,
            &gepa_model,
            parent_profile,
            Some(manual_candidate_profile.clone()),
            checkpoint.call_count,
            gate_a,
            DirectFinalizerGepaReceipt {
                attempts: gepa_attempts,
                response_sha256: Some(gepa_response_sha256),
                decision: Some(gepa_decision),
            },
            None,
            None,
            None,
            &checkpoint.pairs,
        );
        write_sanitized_direct_finalizer_campaign(&report_path, &receipt)?;
        return Err("GEPA rejected the exact exercised candidate; holdout was not run".into());
    }

    let generated_candidate = manual_candidate;

    for case in suite.holdout_cases() {
        ensure_pair(
            &mut checkpoint,
            &checkpoint_path,
            case,
            &provider,
            &producer_model,
            &reviewer_model,
            &parent,
            &generated_candidate,
        )?;
    }

    let generated_profile = profile_receipt(&generated_candidate)?;
    let evidence = super::direct_finalizer_campaign_evidence::evaluate_campaign_evidence(
        &suite,
        &parent_profile,
        &generated_profile,
        &dataset_sha256,
        &cohort_sha256,
        &producer_model,
        &reviewer_model,
        &checkpoint.pairs,
        crate::prompt_rollout_runtime::prompt_promotion_gate_config(),
    )?;
    let promotion = DirectFinalizerPromotionReceipt {
        protocol: orchestrator::PROMPT_PROMOTION_GATE_PROTOCOL.to_string(),
        eligible: evidence.gate.eligible,
        blocker_codes: evidence
            .gate
            .blockers
            .iter()
            .map(|blocker| blocker.label().to_string())
            .collect(),
    };
    let mut snapshot_artifact_sha256 = None;
    if evidence.gate.eligible {
        let snapshot = orchestrator::FrozenPromptProfileSnapshot::new_gepa(
            "auto",
            generated_candidate,
            parent.id.clone(),
            dataset_sha256.clone(),
            evidence.paired_evidence_sha256.clone(),
        )?;
        let encoded = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| format!("failed to encode frozen snapshot: {error}"))?;
        tools::write_private_file_atomically(&snapshot_path, &encoded)
            .map_err(|error| format!("failed to write frozen snapshot: {error}"))?;
        snapshot_artifact_sha256 = Some(snapshot.artifact_sha256()?);
    }
    let receipt = base_campaign_receipt(
        &suite,
        &suite_sha256,
        &dataset_sha256,
        &cohort_sha256,
        &source_commit,
        &provider.provider_id,
        &producer_model,
        &reviewer_model,
        &gepa_model,
        parent_profile,
        Some(generated_profile),
        checkpoint.call_count,
        gate_a,
        DirectFinalizerGepaReceipt {
            attempts: gepa_attempts,
            response_sha256: Some(gepa_response_sha256),
            decision: Some(gepa_decision),
        },
        Some(promotion),
        Some(evidence.paired_evidence_sha256),
        snapshot_artifact_sha256,
        &checkpoint.pairs,
    );
    write_sanitized_direct_finalizer_campaign(&report_path, &receipt)?;
    if !evidence.gate.eligible {
        return Err("Direct-finalizer candidate did not pass the frozen promotion gate".into());
    }
    eprintln!(
        "[direct-finalizer-gepa] eligible=true calls={} report={} snapshot={}",
        checkpoint.call_count,
        report_path.display(),
        snapshot_path.display()
    );
    Ok(())
}
