use super::*;
use crate::direct_judge_shadow_runtime::{
    admit_direct_judge_shadow_fitness, direct_judge_shadow_journal_path_for,
};

const PROMPT_EVOLUTION_ADMISSION_CONFIG_FILE: &str = "prompt_evolution.json";
const PROMPT_EVOLUTION_REVIEW_RECEIPT_FILE: &str =
    "prompt-evolution/direct-judge-review-receipt.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct AdmittedDirectJudgeFitnessConfig {
    pub(crate) enabled: bool,
    pub(crate) review_receipt_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct PromptEvolutionAdmissionConfig {
    pub(crate) admitted_direct_judge_fitness: AdmittedDirectJudgeFitnessConfig,
}

/// Read-model surface for one admitted direct-judge fitness window. The
/// fitness mapping stays conservative and permanently promotion-ineligible;
/// this block only makes the admitted window observable on the evolution
/// read path.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptEvolutionAdmissionState {
    pub(crate) admission_sha256: String,
    pub(crate) review_sha256: String,
    pub(crate) reviewer_identity_sha256: String,
    pub(crate) window_size: usize,
    pub(crate) scored_runs: usize,
    pub(crate) average_reward_bps: u16,
    pub(crate) fitness: agent_core::PromptFitness,
}

pub(crate) fn prompt_evolution_admission_config_path_for(data_root: &Path) -> PathBuf {
    data_root.join(PROMPT_EVOLUTION_ADMISSION_CONFIG_FILE)
}

fn prompt_evolution_review_receipt_default_path(data_root: &Path) -> PathBuf {
    data_root.join(PROMPT_EVOLUTION_REVIEW_RECEIPT_FILE)
}

/// Loads the admission configuration fail-closed: a missing or corrupt file
/// yields the default, which keeps the channel disabled.
pub(crate) fn load_prompt_evolution_admission_config_from(
    path: &Path,
) -> PromptEvolutionAdmissionConfig {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Resolves the admitted direct-judge fitness window for one data root.
/// Returns None when the channel is disabled, inputs are missing, or any
/// digest/approval validation fails; it never fails the evolution read path.
pub(crate) fn admitted_direct_judge_fitness_state(
    data_root: &Path,
) -> Option<PromptEvolutionAdmissionState> {
    let config = load_prompt_evolution_admission_config_from(
        &prompt_evolution_admission_config_path_for(data_root),
    );
    if !config.admitted_direct_judge_fitness.enabled {
        return None;
    }
    admitted_direct_judge_fitness_state_with(data_root, &config.admitted_direct_judge_fitness)
}

fn admitted_direct_judge_fitness_state_with(
    data_root: &Path,
    config: &AdmittedDirectJudgeFitnessConfig,
) -> Option<PromptEvolutionAdmissionState> {
    let journal_path = direct_judge_shadow_journal_path_for(data_root);
    let receipt_path = config
        .review_receipt_path
        .clone()
        .unwrap_or_else(|| prompt_evolution_review_receipt_default_path(data_root));
    let receipt_json = match fs::read_to_string(&receipt_path) {
        Ok(content) => content,
        Err(error) => {
            eprintln!(
                "prompt evolution admitted fitness unavailable: receipt {}: {error}",
                receipt_path.display()
            );
            return None;
        }
    };
    match admit_direct_judge_shadow_fitness(&journal_path, &receipt_json) {
        Ok((admission, fitness)) => Some(PromptEvolutionAdmissionState {
            admission_sha256: admission.admission_sha256,
            review_sha256: admission.review_sha256,
            reviewer_identity_sha256: admission.reviewer_identity_sha256,
            window_size: admission.window_size,
            scored_runs: admission.scored_runs,
            average_reward_bps: admission.average_reward_bps,
            fitness,
        }),
        Err(error) => {
            eprintln!("prompt evolution admitted fitness unavailable: {error}");
            None
        }
    }
}
