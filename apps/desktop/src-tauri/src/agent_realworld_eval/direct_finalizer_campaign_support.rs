use super::direct_finalizer_campaign_contract::*;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, ActionableSideInformation, AgentPolicy,
    ConductorPromptGenome, PromptDatasetCaseIdentityV1, PromptEvaluationSplit,
    PromptExecutionContextV1, PromptLearningCohortV1, PROMPT_EXECUTION_CONTEXT_SCHEMA_V1,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHECKPOINT_SCHEMA: &str = "cindx.direct-finalizer-gepa-private-checkpoint.v1";
const GATE_A_PROTOCOL: &str = "direct_finalizer_manual_calibration_v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DirectFinalizerCampaignPairEvidence {
    pub(super) receipt: DirectFinalizerPairReceipt,
    pub(super) parent_output: String,
    pub(super) candidate_output: String,
    pub(super) parent_feedback: ActionableSideInformation,
    pub(super) candidate_feedback: ActionableSideInformation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PrivateCampaignCheckpoint {
    schema: String,
    source_commit: String,
    suite_sha256: String,
    cohort_sha256: String,
    parent_profile: DirectFinalizerProfileReceipt,
    manual_candidate_profile: DirectFinalizerProfileReceipt,
    pub(super) pairs: Vec<DirectFinalizerCampaignPairEvidence>,
    pub(super) gepa_response: Option<String>,
    pub(super) gepa_attempts: u32,
    pub(super) call_count: u64,
}

pub(super) fn profile_receipt(
    genome: &ConductorPromptGenome,
) -> Result<DirectFinalizerProfileReceipt, String> {
    Ok(DirectFinalizerProfileReceipt {
        profile_id: genome.id.clone(),
        profile_sha256: prompt_genome_sha256(genome)?,
    })
}

pub(super) fn evaluate_gate_a(
    suite: &DirectFinalizerCampaignSuite,
    pairs: &[DirectFinalizerCampaignPairEvidence],
) -> DirectFinalizerGateReceipt {
    let gate_ids = suite
        .gate_a_case_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let selected = pairs
        .iter()
        .filter(|pair| gate_ids.contains(pair.receipt.case_id.as_str()))
        .collect::<Vec<_>>();
    let mut blockers = Vec::new();
    if selected.len() != gate_ids.len() {
        blockers.push("incomplete_gate_a_pairs".to_string());
    }
    let regressions = selected
        .iter()
        .filter(|pair| {
            let receipt = &pair.receipt;
            let candidate_regressed_verification =
                !receipt.candidate.verification.passed && receipt.parent.verification.passed;
            candidate_regressed_verification
                || receipt.candidate.deterministic_score + f64::EPSILON
                    < receipt.parent.deterministic_score
                || receipt.reviewer_score_candidate + 0.05 < receipt.reviewer_score_parent
                || receipt.reviewer_safety_violations_candidate
                    > receipt.reviewer_safety_violations_parent
        })
        .count();
    if regressions > 0 {
        blockers.push("candidate_regression".to_string());
    }
    let improvements = selected
        .iter()
        .filter(|pair| {
            (!pair.receipt.parent.verification.passed && pair.receipt.candidate.verification.passed)
                || pair.receipt.reviewer_score_candidate
                    >= pair.receipt.reviewer_score_parent + 0.10
        })
        .count();
    if improvements < 2 {
        blockers.push("insufficient_expected_direction_gain".to_string());
    }
    if selected.iter().any(|pair| {
        pair.receipt.reviewer_safety_violations_parent > 0
            || pair.receipt.reviewer_safety_violations_candidate > 0
            || pair.receipt.pre_treatment_state_sha256.len() != 64
            || pair.receipt.parent.canonical_request_sha256
                == pair.receipt.candidate.canonical_request_sha256
    }) {
        blockers.push("invalid_causal_or_safety_receipt".to_string());
    }
    DirectFinalizerGateReceipt {
        protocol: GATE_A_PROTOCOL.to_string(),
        status: if blockers.is_empty() {
            "passed".to_string()
        } else {
            "blocked".to_string()
        },
        passed: blockers.is_empty(),
        evaluated_case_ids: selected
            .iter()
            .map(|pair| pair.receipt.case_id.clone())
            .collect(),
        blocker_codes: blockers,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_learning_cohort(
    suite: &DirectFinalizerCampaignSuite,
    source_commit: &str,
    suite_sha256: &str,
    provider_id: &str,
    provider_endpoint: &str,
    producer_model: &str,
    reviewer_model: &str,
    gepa_model: &str,
    system_prompt: &str,
) -> Result<PromptLearningCohortV1, String> {
    let cases = suite
        .cases
        .iter()
        .map(|case| PromptDatasetCaseIdentityV1 {
            case_id: case.id.clone(),
            objective_sha256: sha256_hex(case.objective.trim().as_bytes()),
            task_family_sha256: sha256_hex(case.task_class.trim().as_bytes()),
            split: match case.split {
                DirectFinalizerCaseSplit::Train => PromptEvaluationSplit::Train,
                DirectFinalizerCaseSplit::Holdout => PromptEvaluationSplit::Holdout,
            },
        })
        .collect();
    let dataset = orchestrator::PromptDatasetIdentityV1::new(suite_sha256, suite.version, cases)?;
    let model_pool = serde_json::to_vec(&(producer_model, reviewer_model, gepa_model))
        .map_err(|error| format!("failed to encode Direct-finalizer model pool: {error}"))?;
    let execution = PromptExecutionContextV1 {
        schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
        provider_sha256: sha256_hex(format!("{provider_id}\0{provider_endpoint}").as_bytes()),
        model_pool_sha256: sha256_hex(&model_pool),
        harness_sha256: sha256_hex(format!("direct-finalizer-gepa-v1\0{source_commit}").as_bytes()),
        system_prompt_sha256: sha256_hex(system_prompt.as_bytes()),
        policy: AgentPolicy::Auto,
        policy_sha256: sha256_hex(b"auto"),
        budget_sha256: sha256_hex(b"direct-finalizer-gepa-58-provider-call-cap"),
        tool_contract_sha256: sha256_hex(b"evaluation.frozen_evidence:read_only:v1"),
        source_revision_sha256: sha256_hex(source_commit.as_bytes()),
        workspace_revision_sha256: sha256_hex(suite_sha256.as_bytes()),
    };
    PromptLearningCohortV1::new(dataset, execution)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn base_campaign_receipt(
    suite: &DirectFinalizerCampaignSuite,
    suite_sha256: &str,
    dataset_sha256: &str,
    cohort_sha256: &str,
    source_commit: &str,
    provider_id: &str,
    producer_model: &str,
    reviewer_model: &str,
    gepa_model: &str,
    parent_profile: DirectFinalizerProfileReceipt,
    candidate_profile: Option<DirectFinalizerProfileReceipt>,
    call_count: u64,
    gate_a: DirectFinalizerGateReceipt,
    gepa: DirectFinalizerGepaReceipt,
    promotion: Option<DirectFinalizerPromotionReceipt>,
    paired_evidence_sha256: Option<String>,
    snapshot_artifact_sha256: Option<String>,
    pairs: &[DirectFinalizerCampaignPairEvidence],
) -> DirectFinalizerCampaignReceipt {
    DirectFinalizerCampaignReceipt {
        schema: DIRECT_FINALIZER_CAMPAIGN_RECEIPT_SCHEMA.to_string(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        suite_sha256: suite_sha256.to_string(),
        dataset_sha256: dataset_sha256.to_string(),
        cohort_sha256: cohort_sha256.to_string(),
        source_commit: source_commit.to_string(),
        provider_id: provider_id.to_string(),
        models: DirectFinalizerCampaignModels {
            producer_model: producer_model.to_string(),
            reviewer_model: reviewer_model.to_string(),
            gepa_model: gepa_model.to_string(),
        },
        parent_profile,
        candidate_profile,
        call_count,
        gate_a,
        gepa,
        promotion,
        paired_evidence_sha256,
        snapshot_artifact_sha256,
        pairs: pairs.iter().map(|pair| pair.receipt.clone()).collect(),
    }
}

pub(super) fn load_or_initialize_checkpoint(
    path: &Path,
    source_commit: &str,
    suite_sha256: &str,
    cohort_sha256: &str,
    parent_profile: &DirectFinalizerProfileReceipt,
    manual_candidate_profile: &DirectFinalizerProfileReceipt,
    suite: &DirectFinalizerCampaignSuite,
) -> Result<PrivateCampaignCheckpoint, String> {
    let mut checkpoint = match fs::read(path) {
        Ok(encoded) => serde_json::from_slice::<PrivateCampaignCheckpoint>(&encoded)
            .map_err(|error| format!("private checkpoint is invalid: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PrivateCampaignCheckpoint {
                schema: CHECKPOINT_SCHEMA.to_string(),
                source_commit: source_commit.to_string(),
                suite_sha256: suite_sha256.to_string(),
                cohort_sha256: cohort_sha256.to_string(),
                parent_profile: parent_profile.clone(),
                manual_candidate_profile: manual_candidate_profile.clone(),
                pairs: Vec::new(),
                gepa_response: None,
                gepa_attempts: 0,
                call_count: 0,
            });
        }
        Err(error) => {
            return Err(format!(
                "failed to read private checkpoint {}: {error}",
                path.display()
            ));
        }
    };
    if checkpoint.schema != CHECKPOINT_SCHEMA
        || checkpoint.source_commit != source_commit
        || checkpoint.suite_sha256 != suite_sha256
        || checkpoint.cohort_sha256 != cohort_sha256
        || checkpoint.parent_profile != *parent_profile
        || checkpoint.manual_candidate_profile != *manual_candidate_profile
        || checkpoint.gepa_attempts > 2
        || checkpoint.call_count > 58
        || checkpoint.gepa_response.is_some() != (checkpoint.gepa_attempts > 0)
    {
        return Err("private checkpoint identity does not match the frozen campaign".to_string());
    }
    let known_cases = suite
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let mut observed = BTreeSet::new();
    for pair in &mut checkpoint.pairs {
        let case = known_cases
            .get(pair.receipt.case_id.as_str())
            .ok_or_else(|| "private checkpoint contains an unknown case".to_string())?;
        if !observed.insert(pair.receipt.case_id.as_str())
            || pair.receipt.split != case.split
            || pair.receipt.task_class != case.task_class
            || sha256_hex(pair.parent_output.as_bytes()) != pair.receipt.parent.output_sha256
            || sha256_hex(pair.candidate_output.as_bytes()) != pair.receipt.candidate.output_sha256
            || verify_direct_finalizer_output(case, &pair.parent_output)
                != pair.receipt.parent.verification
            || verify_direct_finalizer_output(case, &pair.candidate_output)
                != pair.receipt.candidate.verification
        {
            return Err("private checkpoint pair failed frozen receipt validation".to_string());
        }
    }
    Ok(checkpoint)
}

pub(super) fn write_checkpoint(
    path: &Path,
    checkpoint: &PrivateCampaignCheckpoint,
) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(checkpoint)
        .map_err(|error| format!("failed to encode private checkpoint: {error}"))?;
    tools::write_private_file_atomically(path, &encoded)
        .map_err(|error| format!("failed to write private checkpoint: {error}"))
}

pub(super) fn required_external_path(
    variable: &str,
    repo_root: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{variable} is required"))?;
    validate_external_path(&path, repo_root, label)?;
    Ok(path)
}

pub(super) fn validate_external_path(
    path: &Path,
    repo_root: &Path,
    label: &str,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{label} path must be absolute"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {label} parent: {error}"))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize {label} parent: {error}"))?;
    if canonical_parent.starts_with(repo_root) {
        return Err(format!("{label} must remain outside the repository"));
    }
    Ok(())
}

pub(super) fn require_clean_source(repo_root: &Path) -> Result<String, String> {
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("failed to inspect worktree: {error}"))?;
    if !status.status.success() {
        return Err("git status failed before provider-backed campaign".to_string());
    }
    if !status.stdout.is_empty() {
        return Err("worktree must be clean before provider-backed campaign".to_string());
    }
    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("failed to resolve source revision: {error}"))?;
    if !revision.status.success() {
        return Err("git rev-parse failed before provider-backed campaign".to_string());
    }
    let revision = String::from_utf8(revision.stdout)
        .map_err(|_| "source revision is not UTF-8".to_string())?
        .trim()
        .to_string();
    if revision.len() != 40
        || revision
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err("source revision is not a full lowercase SHA-1".to_string());
    }
    Ok(revision)
}

pub(super) fn reserve_provider_calls(
    checkpoint: &mut PrivateCampaignCheckpoint,
    checkpoint_path: &Path,
    amount: u64,
) -> Result<(), String> {
    let current = checkpoint.call_count;
    checkpoint.call_count = current
        .checked_add(amount)
        .filter(|next| *next <= 58)
        .ok_or_else(|| "Direct-finalizer campaign exceeded its 58-call hard cap".to_string())?;
    if let Err(error) = write_checkpoint(checkpoint_path, checkpoint) {
        checkpoint.call_count = current;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod call_budget_tests {
    use super::*;

    fn checkpoint() -> PrivateCampaignCheckpoint {
        PrivateCampaignCheckpoint {
            schema: CHECKPOINT_SCHEMA.to_string(),
            source_commit: "0".repeat(40),
            suite_sha256: "1".repeat(64),
            cohort_sha256: "2".repeat(64),
            parent_profile: DirectFinalizerProfileReceipt {
                profile_id: "parent".to_string(),
                profile_sha256: "3".repeat(64),
            },
            manual_candidate_profile: DirectFinalizerProfileReceipt {
                profile_id: "candidate".to_string(),
                profile_sha256: "4".repeat(64),
            },
            pairs: Vec::new(),
            gepa_response: None,
            gepa_attempts: 0,
            call_count: 0,
        }
    }

    #[test]
    fn provider_budget_is_durable_before_execution_and_fails_closed_at_cap() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("checkpoint.json");
        let mut checkpoint = checkpoint();

        reserve_provider_calls(&mut checkpoint, &path, 4).unwrap();
        let durable =
            serde_json::from_slice::<PrivateCampaignCheckpoint>(&std::fs::read(&path).unwrap())
                .unwrap();
        assert_eq!(checkpoint.call_count, 4);
        assert_eq!(durable.call_count, 4);

        let error = reserve_provider_calls(&mut checkpoint, &path, 55).unwrap_err();
        assert!(error.contains("58-call hard cap"));
        assert_eq!(checkpoint.call_count, 4);
        let durable =
            serde_json::from_slice::<PrivateCampaignCheckpoint>(&std::fs::read(&path).unwrap())
                .unwrap();
        assert_eq!(durable.call_count, 4);
    }
}
