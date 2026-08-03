use super::{
    AutoTeacherSourceContextV1, ConductorPromptGenome, PromptProToAutoDistillationProvenanceV1,
};
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
    #[serde(default)]
    pub source_context_sha256: String,
    #[serde(default)]
    pub evaluator_receipt_sha256: String,
    #[serde(default)]
    pub checkpoint_sha256: String,
    #[serde(default)]
    pub learning_receipt_sha256: String,
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
            source_context_sha256: String::new(),
            evaluator_receipt_sha256: String::new(),
            checkpoint_sha256: String::new(),
            learning_receipt_sha256: String::new(),
        }
    }

    pub fn with_source_context(
        mut self,
        context: &AutoTeacherSourceContextV1,
    ) -> Result<Self, String> {
        self.source_context_sha256 = context.digest()?;
        self.evaluator_receipt_sha256 = context.evaluator_receipt_sha256.clone();
        self.checkpoint_sha256 = context.checkpoint_sha256.clone();
        self.learning_receipt_sha256 = context.learning_receipt_sha256.clone();
        Ok(self)
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

    pub fn has_strict_source_lineage(&self) -> bool {
        self.is_valid_auto_to_pro()
            && [
                &self.source_context_sha256,
                &self.evaluator_receipt_sha256,
                &self.checkpoint_sha256,
                &self.learning_receipt_sha256,
            ]
            .into_iter()
            .all(|digest| is_sha256(digest))
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
    #[serde(default)]
    pub pro_to_auto_distillation: Option<PromptProToAutoDistillationProvenanceV1>,
    #[serde(default)]
    pub matched_evaluation: Option<super::PromptMatchedEvaluationIdentityV1>,
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
            pro_to_auto_distillation: None,
            matched_evaluation: None,
        }
    }

    pub fn with_transfer(mut self, transfer: PromptTransferProvenance) -> Self {
        self.transfer = Some(transfer);
        self
    }

    pub fn with_pro_to_auto_distillation(
        mut self,
        provenance: PromptProToAutoDistillationProvenanceV1,
    ) -> Self {
        self.pro_to_auto_distillation = Some(provenance);
        self
    }

    pub fn with_matched_evaluation(
        mut self,
        identity: super::PromptMatchedEvaluationIdentityV1,
    ) -> Self {
        self.matched_evaluation = Some(identity);
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
        self.transfer.is_none()
            && self.pro_to_auto_distillation.is_none()
            && self.has_scientific_core()
    }

    pub fn is_scientific_transfer(&self) -> bool {
        self.has_scientific_core()
            && self.pro_to_auto_distillation.is_none()
            && self
                .transfer
                .as_ref()
                .is_some_and(PromptTransferProvenance::is_valid_auto_to_pro)
    }

    pub fn is_source_attested_transfer(&self) -> bool {
        self.is_scientific_transfer()
            && self
                .transfer
                .as_ref()
                .is_some_and(PromptTransferProvenance::has_strict_source_lineage)
    }

    pub fn is_scientific_pro_to_auto_distillation(&self) -> bool {
        self.has_scientific_core()
            && self.transfer.is_none()
            && self
                .pro_to_auto_distillation
                .as_ref()
                .is_some_and(|provenance| {
                    provenance.permits_evaluation_digest_lineage(
                        &self.dataset_sha256,
                        self.matched_evaluation
                            .as_ref()
                            .map(|identity| identity.cohort_sha256.as_str()),
                    )
                })
    }

    pub fn is_strict_matched(&self) -> bool {
        self.has_scientific_core()
            && self.matched_evaluation.as_ref().is_some_and(|identity| {
                identity.validate().is_ok() && identity.dataset_sha256 == self.dataset_sha256
            })
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

    pub fn is_scientific_pro_to_auto_distillation_evidence(&self) -> bool {
        let lineage_matches = self
            .provenance
            .pro_to_auto_distillation
            .as_ref()
            .is_some_and(|distillation| {
                if self.profile_id == distillation.auto_child_profile_id
                    && self.opponent_profile_id.as_deref()
                        == Some(distillation.auto_parent_profile_id.as_str())
                {
                    self.provenance.candidate_prompt_sha256
                        == distillation.auto_child_profile_sha256
                        && self.provenance.opponent_prompt_sha256
                            == distillation.auto_parent_profile_sha256
                } else if self.profile_id == distillation.auto_parent_profile_id
                    && self.opponent_profile_id.as_deref()
                        == Some(distillation.auto_child_profile_id.as_str())
                {
                    self.provenance.candidate_prompt_sha256
                        == distillation.auto_parent_profile_sha256
                        && self.provenance.opponent_prompt_sha256
                            == distillation.auto_child_profile_sha256
                } else {
                    false
                }
            });
        self.mode.is_execution()
            && !self.evaluation_id.trim().is_empty()
            && !self.case_id.trim().is_empty()
            && self.quality_score.is_finite()
            && self.relative_reward.is_some_and(f64::is_finite)
            && self.provenance.is_scientific_pro_to_auto_distillation()
            && lineage_matches
    }

    pub fn is_source_attested_transfer_evidence(&self) -> bool {
        self.is_scientific_transfer_evidence() && self.provenance.is_source_attested_transfer()
    }

    pub fn is_strict_matched_evidence(&self) -> bool {
        self.is_scientific_evidence()
            && self
                .provenance
                .matched_evaluation
                .as_ref()
                .is_some_and(|identity| {
                    self.provenance.is_strict_matched()
                        && identity.evaluation_id == self.evaluation_id
                        && identity.case_id == self.case_id
                        && identity.split == self.split
                        && identity.mode == self.mode
                })
    }

    pub fn is_strict_matched_transfer_evidence(&self) -> bool {
        self.is_scientific_transfer_evidence()
            && self
                .provenance
                .matched_evaluation
                .as_ref()
                .is_some_and(|identity| {
                    self.provenance.is_strict_matched()
                        && identity.evaluation_id == self.evaluation_id
                        && identity.case_id == self.case_id
                        && identity.split == self.split
                        && identity.mode == self.mode
                })
    }

    pub fn is_strict_source_attested_transfer_evidence(&self) -> bool {
        self.is_strict_matched_transfer_evidence() && self.is_source_attested_transfer_evidence()
    }

    pub fn is_strict_pro_to_auto_distillation_evidence(&self) -> bool {
        self.is_scientific_pro_to_auto_distillation_evidence()
            && self
                .provenance
                .matched_evaluation
                .as_ref()
                .is_some_and(|identity| {
                    self.provenance.is_strict_matched()
                        && identity.evaluation_id == self.evaluation_id
                        && identity.case_id == self.case_id
                        && identity.split == self.split
                        && identity.mode == self.mode
                })
    }

    pub fn scientific_cohort_sha256(&self) -> Option<&str> {
        match self.provenance.matched_evaluation.as_ref() {
            Some(identity)
                if self.is_strict_matched_evidence()
                    || self.is_strict_matched_transfer_evidence()
                    || self.is_strict_pro_to_auto_distillation_evidence() =>
            {
                Some(identity.cohort_sha256.as_str())
            }
            Some(_) => None,
            None if self.is_scientific_evidence()
                || self.is_scientific_transfer_evidence()
                || self.is_scientific_pro_to_auto_distillation_evidence() =>
            {
                Some(self.provenance.dataset_sha256.as_str())
            }
            None => None,
        }
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
        .filter(|observation| observation.is_scientific_evidence())
        .find_map(|observation| observation.scientific_cohort_sha256())
}

pub fn latest_scientific_training_dataset_digest(
    observations: &[PromptEvolutionObservation],
) -> Option<&str> {
    observations
        .iter()
        .rev()
        .filter(|observation| {
            observation.split == PromptEvaluationSplit::Train
                && observation.mode == PromptEvaluationMode::PairedExecution
                && observation.is_scientific_evidence()
        })
        .find_map(|observation| observation.scientific_cohort_sha256())
}

pub fn latest_scientific_transfer_dataset_digest(
    observations: &[PromptEvolutionObservation],
) -> Option<&str> {
    observations
        .iter()
        .rev()
        .filter(|observation| observation.is_source_attested_transfer_evidence())
        .find_map(|observation| observation.scientific_cohort_sha256())
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
                && observation.scientific_cohort_sha256() == Some(active_dataset_sha256)
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
                && observation.is_source_attested_transfer_evidence()
                && observation.scientific_cohort_sha256() == Some(active_dataset_sha256)
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

    fn source_context() -> AutoTeacherSourceContextV1 {
        AutoTeacherSourceContextV1 {
            schema: super::super::AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            system_prompt_sha256: "3".repeat(64),
            policy_sha256: "4".repeat(64),
            budget_sha256: "5".repeat(64),
            tool_contract_sha256: "6".repeat(64),
            source_revision_sha256: "7".repeat(64),
            workspace_revision_sha256: "8".repeat(64),
            evaluator_identity_sha256: "9".repeat(64),
            evaluator_receipt_sha256: "a".repeat(64),
            checkpoint_sha256: "b".repeat(64),
            learning_receipt_sha256: "c".repeat(64),
        }
    }

    fn distillation_provenance() -> PromptProToAutoDistillationProvenanceV1 {
        let pro_genome = ConductorPromptGenome::seed_for_effort("pro")
            .mutations()
            .into_iter()
            .next()
            .unwrap();
        let snapshot = super::super::FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            pro_genome,
            "seed-pro-v1",
            "1".repeat(64),
            "2".repeat(64),
        )
        .unwrap()
        .with_auto_teacher_evidence(super::super::FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: "auto-source".to_string(),
            source_profile_sha256: "3".repeat(64),
            dataset_sha256: "4".repeat(64),
            cohort_sha256: Some("5".repeat(64)),
            paired_evidence_sha256: "6".repeat(64),
            promotion_gate_protocol: super::super::PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(
                super::super::FrozenPromptSourceProfileLineageV1::undistilled("3".repeat(64))
                    .unwrap(),
            ),
        })
        .unwrap();
        let teacher = super::super::ProTeacherAttestationV1::from_stable_snapshot(
            &snapshot,
            &snapshot.genome.id,
        )
        .unwrap();
        PromptProToAutoDistillationProvenanceV1::new(
            teacher,
            "auto-parent",
            "c".repeat(64),
            "auto-child",
            "b".repeat(64),
        )
        .unwrap()
    }

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
        assert!(!transfer.is_source_attested_transfer());

        let attested = scientific_provenance().with_transfer(
            PromptTransferProvenance::auto_to_pro(
                "auto-run-1",
                0,
                "auto-profile-1",
                "d".repeat(64),
                "e".repeat(64),
            )
            .with_source_context(&source_context())
            .unwrap(),
        );
        assert!(attested.is_source_attested_transfer());
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

    #[test]
    fn legacy_transfer_serde_replay_cannot_gain_strict_lineage() {
        let encoded = r#"{
            "source_effort":"auto",
            "target_effort":"pro",
            "source_run_id":"run",
            "source_steer_epoch":0,
            "source_profile_id":"profile",
            "source_profile_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "source_output_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        }"#;
        let legacy: PromptTransferProvenance = serde_json::from_str(encoded).unwrap();
        assert!(legacy.is_valid_auto_to_pro());
        assert!(!legacy.has_strict_source_lineage());
    }

    #[test]
    fn pro_to_auto_evidence_is_typed_and_isolated_from_other_tracks() {
        let provenance =
            scientific_provenance().with_pro_to_auto_distillation(distillation_provenance());

        assert!(!provenance.is_scientific());
        assert!(!provenance.is_scientific_transfer());
        assert!(provenance.is_scientific_pro_to_auto_distillation());

        let mut observation = PromptEvolutionObservation {
            profile_id: "auto-child".to_string(),
            evaluation_id: "distill-eval".to_string(),
            case_id: "distill-case".to_string(),
            opponent_profile_id: Some("auto-parent".to_string()),
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::PairedExecution,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.2),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance,
        };
        assert!(observation.is_scientific_pro_to_auto_distillation_evidence());

        observation.profile_id = "auto-parent".to_string();
        assert!(!observation.is_scientific_pro_to_auto_distillation_evidence());
    }

    #[test]
    fn legacy_provenance_cannot_become_distillation_evidence() {
        let encoded = serde_json::to_vec(&scientific_provenance()).unwrap();
        let legacy: PromptEvaluationProvenance = serde_json::from_slice(&encoded).unwrap();

        assert!(legacy.pro_to_auto_distillation.is_none());
        assert!(!legacy.is_scientific_pro_to_auto_distillation());
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
