use super::ConductorPromptGenome;
use crate::AgentEvaluationReflectionPacket;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvaluationSplit {
    Train,
    Holdout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvaluationMode {
    Live,
    PairedShadow,
    ReplayHoldout,
    PairedExecution,
    ReplayExecution,
}

impl PromptEvaluationMode {
    pub fn is_paired(self) -> bool {
        matches!(self, Self::PairedShadow | Self::PairedExecution)
    }

    pub fn is_replay(self) -> bool {
        matches!(self, Self::ReplayHoldout | Self::ReplayExecution)
    }

    pub fn is_execution(self) -> bool {
        matches!(self, Self::PairedExecution | Self::ReplayExecution)
    }

    pub fn is_paired_execution(self) -> bool {
        self == Self::PairedExecution
    }

    pub fn is_replay_execution(self) -> bool {
        self == Self::ReplayExecution
    }
}

fn default_evaluation_mode() -> PromptEvaluationMode {
    PromptEvaluationMode::Live
}

pub const PROMPT_EVALUATION_PROTOCOL_BLIND_PAIRWISE_SWAP_V1: &str = "blind_pairwise_swap_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTransferProvenance {
    pub source_effort: String,
    pub target_effort: String,
    pub source_run_id: String,
    #[serde(default)]
    pub source_steer_epoch: Option<u64>,
    pub source_profile_id: String,
    pub source_profile_sha256: String,
    pub source_output_sha256: String,
}

impl PromptTransferProvenance {
    pub fn auto_to_pro(
        source_run_id: impl Into<String>,
        source_steer_epoch: u64,
        source_profile_id: impl Into<String>,
        source_profile_sha256: impl Into<String>,
        source_output_sha256: impl Into<String>,
    ) -> Self {
        Self {
            source_effort: "auto".to_string(),
            target_effort: "pro".to_string(),
            source_run_id: source_run_id.into(),
            source_steer_epoch: Some(source_steer_epoch),
            source_profile_id: source_profile_id.into(),
            source_profile_sha256: source_profile_sha256.into(),
            source_output_sha256: source_output_sha256.into(),
        }
    }

    pub fn is_valid_auto_to_pro(&self) -> bool {
        self.source_effort == "auto"
            && self.target_effort == "pro"
            && !self.source_run_id.trim().is_empty()
            && self.source_steer_epoch.is_some()
            && !self.source_profile_id.trim().is_empty()
            && is_sha256(&self.source_profile_sha256)
            && is_sha256(&self.source_output_sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromptEvaluationProvenance {
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub evaluator_models: Vec<String>,
    #[serde(default)]
    pub participant_models: Vec<String>,
    #[serde(default)]
    pub evaluator_independent: bool,
    #[serde(default)]
    pub dataset_sha256: String,
    #[serde(default)]
    pub candidate_prompt_sha256: String,
    #[serde(default)]
    pub opponent_prompt_sha256: String,
    #[serde(default)]
    pub transfer: Option<PromptTransferProvenance>,
}

impl PromptEvaluationProvenance {
    pub fn blind_pairwise_swap(
        evaluator_models: Vec<String>,
        participant_models: Vec<String>,
        dataset_sha256: impl Into<String>,
        candidate_prompt_sha256: impl Into<String>,
        opponent_prompt_sha256: impl Into<String>,
    ) -> Self {
        let evaluator_set = evaluator_models
            .iter()
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
            .collect::<BTreeSet<_>>();
        let participant_set = participant_models
            .iter()
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
            .collect::<BTreeSet<_>>();
        let evaluator_independent = !evaluator_set.is_empty()
            && !participant_set.is_empty()
            && evaluator_set.is_disjoint(&participant_set);
        Self {
            protocol: PROMPT_EVALUATION_PROTOCOL_BLIND_PAIRWISE_SWAP_V1.to_string(),
            evaluator_models,
            participant_models,
            evaluator_independent,
            dataset_sha256: dataset_sha256.into(),
            candidate_prompt_sha256: candidate_prompt_sha256.into(),
            opponent_prompt_sha256: opponent_prompt_sha256.into(),
            transfer: None,
        }
    }

    pub fn with_transfer(mut self, transfer: PromptTransferProvenance) -> Self {
        self.transfer = Some(transfer);
        self
    }

    fn has_scientific_core(&self) -> bool {
        let evaluator_set = self
            .evaluator_models
            .iter()
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
            .collect::<BTreeSet<_>>();
        let participant_set = self
            .participant_models
            .iter()
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
            .collect::<BTreeSet<_>>();
        self.protocol == PROMPT_EVALUATION_PROTOCOL_BLIND_PAIRWISE_SWAP_V1
            && self.evaluator_independent
            && evaluator_set.len() == self.evaluator_models.len()
            && participant_set.len() == self.participant_models.len()
            && !evaluator_set.is_empty()
            && !participant_set.is_empty()
            && evaluator_set.is_disjoint(&participant_set)
            && is_sha256(&self.dataset_sha256)
            && is_sha256(&self.candidate_prompt_sha256)
            && is_sha256(&self.opponent_prompt_sha256)
    }

    pub fn is_scientific(&self) -> bool {
        self.transfer.is_none() && self.has_scientific_core()
    }

    pub fn is_scientific_transfer(&self) -> bool {
        self.has_scientific_core()
            && self
                .transfer
                .as_ref()
                .is_some_and(PromptTransferProvenance::is_valid_auto_to_pro)
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptStepCredit {
    pub step_id: String,
    pub role: String,
    pub succeeded: bool,
    pub attempts: usize,
    pub evidence_count: usize,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub credit: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptEvolutionObservation {
    pub profile_id: String,
    #[serde(default)]
    pub evaluation_id: String,
    #[serde(default)]
    pub case_id: String,
    #[serde(default)]
    pub opponent_profile_id: Option<String>,
    pub task_class: String,
    pub split: PromptEvaluationSplit,
    #[serde(default = "default_evaluation_mode")]
    pub mode: PromptEvaluationMode,
    #[serde(default = "default_true")]
    pub format_valid: bool,
    pub succeeded: bool,
    pub quality_score: f64,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub estimated_cost_microusd: u64,
    pub safety_violations: u64,
    #[serde(default)]
    pub relative_reward: Option<f64>,
    #[serde(default)]
    pub step_credits: Vec<PromptStepCredit>,
    #[serde(default)]
    pub reflection_packet: Option<AgentEvaluationReflectionPacket>,
    #[serde(default)]
    pub provenance: PromptEvaluationProvenance,
}

fn default_true() -> bool {
    true
}

impl PromptEvolutionObservation {
    pub fn is_scientific_evidence(&self) -> bool {
        self.mode.is_execution()
            && !self.evaluation_id.trim().is_empty()
            && !self.case_id.trim().is_empty()
            && self.quality_score.is_finite()
            && self.relative_reward.is_some_and(f64::is_finite)
            && self.provenance.is_scientific()
    }

    pub fn is_scientific_transfer_evidence(&self) -> bool {
        let transfer_lineage_matches = self.provenance.transfer.as_ref().is_some_and(|transfer| {
            if self.profile_id == transfer.source_profile_id {
                self.provenance.candidate_prompt_sha256 == transfer.source_profile_sha256
            } else if self.opponent_profile_id.as_deref() == Some(&transfer.source_profile_id) {
                self.provenance.opponent_prompt_sha256 == transfer.source_profile_sha256
            } else {
                false
            }
        });
        self.mode.is_execution()
            && !self.evaluation_id.trim().is_empty()
            && !self.case_id.trim().is_empty()
            && self.quality_score.is_finite()
            && self.relative_reward.is_some_and(f64::is_finite)
            && self.provenance.is_scientific_transfer()
            && transfer_lineage_matches
    }

    pub fn evidence_identity(&self) -> String {
        if !self.evaluation_id.trim().is_empty() {
            return format!(
                "{}:{}:{}:{}",
                self.profile_id,
                self.evaluation_id.trim(),
                self.provenance.dataset_sha256,
                self.provenance.candidate_prompt_sha256
            );
        }
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                "legacy:{}:{}:{}:{}:{}",
                self.profile_id,
                self.case_id,
                self.opponent_profile_id.as_deref().unwrap_or_default(),
                self.latency_ms,
                self.total_tokens
            )
        })
    }

    pub fn reward(&self) -> f64 {
        if !self.format_valid || self.safety_violations > 0 {
            return 0.0;
        }
        let quality = self.quality_score.clamp(0.0, 1.0);
        let absolute = if self.succeeded {
            0.5 + quality * 0.5
        } else {
            quality * 0.5
        };
        if let Some(relative) = self.relative_reward {
            let relative = (relative.clamp(-1.0, 1.0) + 1.0) * 0.5;
            absolute * 0.7 + relative * 0.3
        } else {
            absolute
        }
    }

    pub fn group_relative_reward(&self) -> f64 {
        if !self.format_valid || self.safety_violations > 0 {
            return -1.0;
        }
        self.relative_reward.unwrap_or_default().clamp(-1.0, 1.0)
    }
}

pub fn latest_scientific_dataset_digest(
    observations: &[PromptEvolutionObservation],
) -> Option<&str> {
    observations
        .iter()
        .rev()
        .find(|observation| observation.is_scientific_evidence())
        .map(|observation| observation.provenance.dataset_sha256.as_str())
}

pub fn latest_scientific_training_dataset_digest(
    observations: &[PromptEvolutionObservation],
) -> Option<&str> {
    observations
        .iter()
        .rev()
        .find(|observation| {
            observation.split == PromptEvaluationSplit::Train
                && observation.mode == PromptEvaluationMode::PairedExecution
                && observation.is_scientific_evidence()
        })
        .map(|observation| observation.provenance.dataset_sha256.as_str())
}

pub fn latest_scientific_transfer_dataset_digest(
    observations: &[PromptEvolutionObservation],
) -> Option<&str> {
    observations
        .iter()
        .rev()
        .find(|observation| observation.is_scientific_transfer_evidence())
        .map(|observation| observation.provenance.dataset_sha256.as_str())
}

pub fn prompt_reflection_packets(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    limit: usize,
) -> Vec<AgentEvaluationReflectionPacket> {
    if limit == 0 {
        return Vec::new();
    }

    let Some(active_dataset_sha256) = latest_scientific_training_dataset_digest(observations)
    else {
        return Vec::new();
    };
    let mut seen_runs = BTreeSet::new();
    observations
        .iter()
        .rev()
        .filter(|observation| {
            observation.profile_id == profile_id
                && observation.split == PromptEvaluationSplit::Train
                && observation.mode == PromptEvaluationMode::PairedExecution
                && observation.is_scientific_evidence()
                && observation.provenance.dataset_sha256 == active_dataset_sha256
        })
        .filter_map(|observation| observation.reflection_packet.as_ref())
        .filter(|packet| packet.candidate_id == profile_id)
        .filter(|packet| seen_runs.insert((packet.run_id.clone(), packet.case_id.clone())))
        .take(limit)
        .cloned()
        .collect()
}

pub fn prompt_transfer_reflection_packets(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    limit: usize,
) -> Vec<AgentEvaluationReflectionPacket> {
    if limit == 0 {
        return Vec::new();
    }

    let Some(active_dataset_sha256) = latest_scientific_transfer_dataset_digest(observations)
    else {
        return Vec::new();
    };
    let mut seen_runs = BTreeSet::new();
    observations
        .iter()
        .rev()
        .filter(|observation| {
            observation.profile_id == profile_id
                && observation.split == PromptEvaluationSplit::Train
                && observation.mode == PromptEvaluationMode::PairedExecution
                && observation.is_scientific_transfer_evidence()
                && observation.provenance.dataset_sha256 == active_dataset_sha256
        })
        .filter_map(|observation| observation.reflection_packet.as_ref())
        .filter(|packet| packet.candidate_id == profile_id)
        .filter(|packet| seen_runs.insert((packet.run_id.clone(), packet.case_id.clone())))
        .take(limit)
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptFitness {
    pub runs: usize,
    pub paired_runs: usize,
    pub replay_runs: usize,
    pub execution_runs: usize,
    pub average_reward: f64,
    pub average_relative_reward: f64,
    pub average_step_credit: f64,
    pub format_valid_rate: f64,
    pub success_rate: f64,
    pub average_quality: f64,
    pub average_latency_ms: f64,
    pub average_total_tokens: f64,
    pub average_cost_microusd: f64,
    pub safety_violations: u64,
    pub task_class_coverage: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptPromotionConfidence {
    pub comparisons: usize,
    pub wins: usize,
    pub losses: usize,
    pub ties: usize,
    pub observed_win_rate: f64,
    pub wilson_lower_bound: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptProposalMinibatchDecision {
    NotRequired,
    Pending {
        comparisons: usize,
        required: usize,
    },
    Accepted {
        comparisons: usize,
        wins: usize,
        losses: usize,
        average_relative_reward: f64,
    },
    Rejected {
        comparisons: usize,
        wins: usize,
        losses: usize,
        average_relative_reward: f64,
        reason: String,
    },
}

impl PromptProposalMinibatchDecision {
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }

    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Accepted { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptParetoCandidate {
    pub genome: ConductorPromptGenome,
    pub train: PromptFitness,
    pub holdout: PromptFitness,
    pub quality_generalization_gap: f64,
    pub confidence: PromptPromotionConfidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptParetoArchive {
    pub candidates: Vec<PromptParetoCandidate>,
    pub rejected_profiles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSearchCandidate {
    pub genome: ConductorPromptGenome,
    pub train: PromptFitness,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSearchArchive {
    pub candidates: Vec<PromptSearchCandidate>,
    pub rejected_profiles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptInstanceParetoCandidate {
    pub profile_id: String,
    pub leading_cases: Vec<String>,
    pub average_score: f64,
    pub verified_success_rate: f64,
    pub evaluations: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptInstanceParetoArchive {
    pub candidates: Vec<PromptInstanceParetoCandidate>,
    pub case_best_scores: BTreeMap<String, f64>,
    pub profile_average_scores: BTreeMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scientific_provenance() -> PromptEvaluationProvenance {
        PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["independent-reviewer".to_string()],
            vec!["worker-a".to_string(), "worker-b".to_string()],
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
        )
    }

    #[test]
    fn auto_transfer_evidence_is_valid_but_isolated_from_same_effort_evidence() {
        let ordinary = scientific_provenance();
        assert!(ordinary.is_scientific());
        assert!(!ordinary.is_scientific_transfer());

        let transfer =
            scientific_provenance().with_transfer(PromptTransferProvenance::auto_to_pro(
                "auto-run-1",
                0,
                "auto-profile-1",
                "d".repeat(64),
                "e".repeat(64),
            ));
        assert!(!transfer.is_scientific());
        assert!(transfer.is_scientific_transfer());
    }

    #[test]
    fn malformed_auto_transfer_fails_closed() {
        let transfer =
            scientific_provenance().with_transfer(PromptTransferProvenance::auto_to_pro(
                "auto-run-1",
                0,
                "auto-profile-1",
                "not-a-fingerprint",
                "e".repeat(64),
            ));

        assert!(!transfer.is_scientific());
        assert!(!transfer.is_scientific_transfer());
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptEvolutionConvergence {
    pub frozen: bool,
    pub reason: Option<String>,
    pub champion: Option<PromptParetoCandidate>,
    pub best_score: f64,
    pub stagnant_generations: usize,
    pub evaluated_generations: usize,
}
