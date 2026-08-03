use super::{
    ConductorPromptGenome, FrozenPromptProToAutoDistillationEvidence,
    FrozenPromptSourceProfileLineageV1, PROMPT_PRO_TO_AUTO_DISTILLATION_GATE_PROTOCOL,
};
use crate::sha256_hex;
use serde::{Deserialize, Serialize};

pub const FROZEN_PROMPT_PROFILE_SCHEMA: &str = "cindx.prompt-profile-snapshot.v1";
pub const PROMPT_PROMOTION_GATE_PROTOCOL: &str = "paired-wilson-task-diversity-v1";
pub const PROMPT_AUTO_TRANSFER_GATE_PROTOCOL: &str =
    "auto-to-pro-matched-cohort-paired-wilson-task-diversity-v2";
const LEGACY_PROMPT_AUTO_TRANSFER_GATE_PROTOCOL: &str =
    "auto-to-pro-paired-wilson-task-diversity-v1";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvolutionMethod {
    GepaReflectivePaired,
    ProToAutoDistillation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenPromptTransferEvidence {
    pub source_effort: String,
    pub source_profile_id: String,
    pub source_profile_sha256: String,
    pub dataset_sha256: String,
    #[serde(default)]
    pub cohort_sha256: Option<String>,
    pub paired_evidence_sha256: String,
    pub promotion_gate_protocol: String,
    #[serde(default)]
    pub source_profile_lineage: Option<FrozenPromptSourceProfileLineageV1>,
}

impl FrozenPromptTransferEvidence {
    pub fn validate(&self) -> Result<(), String> {
        if self.source_effort != "auto" || self.source_profile_id.trim().is_empty() {
            return Err("frozen prompt transfer source must be an Auto profile".to_string());
        }
        if !is_sha256(&self.source_profile_sha256)
            || !is_sha256(&self.dataset_sha256)
            || !is_sha256(&self.paired_evidence_sha256)
        {
            return Err("frozen prompt transfer fingerprints are invalid".to_string());
        }
        let protocol_valid = match self.promotion_gate_protocol.as_str() {
            PROMPT_AUTO_TRANSFER_GATE_PROTOCOL => {
                self.cohort_sha256.as_deref().is_some_and(is_sha256)
            }
            LEGACY_PROMPT_AUTO_TRANSFER_GATE_PROTOCOL => self.cohort_sha256.is_none(),
            _ => false,
        };
        if !protocol_valid {
            return Err(format!(
                "unsupported prompt transfer gate protocol: {}",
                self.promotion_gate_protocol
            ));
        }
        if let Some(lineage) = &self.source_profile_lineage {
            lineage.validate()?;
            if lineage.source_profile_sha256 != self.source_profile_sha256 {
                return Err(
                    "frozen transfer source profile lineage does not match its source".to_string(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenPromptProfileSnapshot {
    pub schema: String,
    pub effort: String,
    pub genome: ConductorPromptGenome,
    pub candidate_sha256: String,
    pub stable_profile_id: String,
    pub dataset_sha256: String,
    pub paired_evidence_sha256: String,
    pub evolution_method: PromptEvolutionMethod,
    pub promotion_gate_protocol: String,
    #[serde(default)]
    pub auto_teacher_evidence: Option<FrozenPromptTransferEvidence>,
    #[serde(default)]
    pub pro_teacher_evidence: Option<FrozenPromptProToAutoDistillationEvidence>,
}

impl FrozenPromptProfileSnapshot {
    pub fn new_gepa(
        effort: impl Into<String>,
        genome: ConductorPromptGenome,
        stable_profile_id: impl Into<String>,
        dataset_sha256: impl Into<String>,
        paired_evidence_sha256: impl Into<String>,
    ) -> Result<Self, String> {
        let candidate_sha256 = prompt_genome_sha256(&genome)?;
        let snapshot = Self {
            schema: FROZEN_PROMPT_PROFILE_SCHEMA.to_string(),
            effort: effort.into(),
            genome,
            candidate_sha256,
            stable_profile_id: stable_profile_id.into(),
            dataset_sha256: dataset_sha256.into(),
            paired_evidence_sha256: paired_evidence_sha256.into(),
            evolution_method: PromptEvolutionMethod::GepaReflectivePaired,
            promotion_gate_protocol: PROMPT_PROMOTION_GATE_PROTOCOL.to_string(),
            auto_teacher_evidence: None,
            pro_teacher_evidence: None,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn new_pro_to_auto_distillation(
        genome: ConductorPromptGenome,
        stable_profile_id: impl Into<String>,
        dataset_sha256: impl Into<String>,
        paired_evidence_sha256: impl Into<String>,
        evidence: FrozenPromptProToAutoDistillationEvidence,
    ) -> Result<Self, String> {
        let candidate_sha256 = prompt_genome_sha256(&genome)?;
        let snapshot = Self {
            schema: FROZEN_PROMPT_PROFILE_SCHEMA.to_string(),
            effort: "auto".to_string(),
            genome,
            candidate_sha256,
            stable_profile_id: stable_profile_id.into(),
            dataset_sha256: dataset_sha256.into(),
            paired_evidence_sha256: paired_evidence_sha256.into(),
            evolution_method: PromptEvolutionMethod::ProToAutoDistillation,
            promotion_gate_protocol: PROMPT_PRO_TO_AUTO_DISTILLATION_GATE_PROTOCOL.to_string(),
            auto_teacher_evidence: None,
            pro_teacher_evidence: Some(evidence),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn with_auto_teacher_evidence(
        mut self,
        evidence: FrozenPromptTransferEvidence,
    ) -> Result<Self, String> {
        self.auto_teacher_evidence = Some(evidence);
        self.validate()?;
        Ok(self)
    }

    pub fn from_json_slice(value: &[u8]) -> Result<Self, String> {
        let snapshot = serde_json::from_slice::<Self>(value)
            .map_err(|error| format!("frozen prompt profile JSON is invalid: {error}"))?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != FROZEN_PROMPT_PROFILE_SCHEMA {
            return Err(format!(
                "unsupported frozen prompt profile schema: {}",
                self.schema
            ));
        }
        if !matches!(self.effort.trim(), "auto" | "pro") {
            return Err("frozen GEPA prompt profile effort must be auto or pro".to_string());
        }
        self.genome.validate()?;
        if self.genome.generation == 0 || self.genome.parents.is_empty() {
            return Err(
                "frozen GEPA prompt profile must preserve non-seed evolution lineage".to_string(),
            );
        }
        if self.stable_profile_id.trim().is_empty() || self.stable_profile_id == self.genome.id {
            return Err(
                "frozen GEPA prompt profile requires a distinct stable comparison profile"
                    .to_string(),
            );
        }
        let expected_candidate_sha256 = prompt_genome_sha256(&self.genome)?;
        if self.candidate_sha256 != expected_candidate_sha256 {
            return Err("frozen prompt profile candidate fingerprint does not match".to_string());
        }
        if !is_sha256(&self.dataset_sha256) {
            return Err("frozen prompt profile dataset fingerprint is invalid".to_string());
        }
        if !is_sha256(&self.paired_evidence_sha256) {
            return Err("frozen prompt profile paired evidence fingerprint is invalid".to_string());
        }
        let expected_gate_protocol = match self.evolution_method {
            PromptEvolutionMethod::GepaReflectivePaired => PROMPT_PROMOTION_GATE_PROTOCOL,
            PromptEvolutionMethod::ProToAutoDistillation => {
                PROMPT_PRO_TO_AUTO_DISTILLATION_GATE_PROTOCOL
            }
        };
        if self.promotion_gate_protocol != expected_gate_protocol {
            return Err(format!(
                "unsupported prompt promotion gate protocol: {}",
                self.promotion_gate_protocol
            ));
        }
        if let Some(evidence) = &self.auto_teacher_evidence {
            if self.effort != "pro" {
                return Err(
                    "frozen Auto teacher evidence is only valid for Pro profiles".to_string(),
                );
            }
            evidence.validate()?;
        }
        if let Some(evidence) = &self.pro_teacher_evidence {
            if self.effort != "auto"
                || self.evolution_method != PromptEvolutionMethod::ProToAutoDistillation
            {
                return Err(
                    "frozen Pro teacher evidence requires an Auto distillation profile".to_string(),
                );
            }
            evidence.validate()?;
            if evidence.auto_child_profile_id != self.genome.id
                || evidence.auto_child_profile_sha256 != self.candidate_sha256
                || evidence.auto_parent_profile_id != self.stable_profile_id
                || evidence.dataset_sha256 != self.dataset_sha256
                || evidence.paired_evidence_sha256 != self.paired_evidence_sha256
            {
                return Err(
                    "frozen Pro-to-Auto evidence does not match the promoted Auto snapshot"
                        .to_string(),
                );
            }
        }
        if self.auto_teacher_evidence.is_some() && self.pro_teacher_evidence.is_some() {
            return Err("frozen prompt profile cannot mix transfer directions".to_string());
        }
        match self.evolution_method {
            PromptEvolutionMethod::GepaReflectivePaired if self.pro_teacher_evidence.is_some() => {
                return Err("GEPA snapshots cannot claim Pro distillation evidence".to_string());
            }
            PromptEvolutionMethod::ProToAutoDistillation if self.pro_teacher_evidence.is_none() => {
                return Err(
                    "Auto distillation snapshots require frozen Pro teacher evidence".to_string(),
                );
            }
            _ => {}
        }
        Ok(())
    }

    pub fn artifact_sha256(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("frozen prompt profile serialization failed: {error}"))
    }
}

pub fn prompt_genome_sha256(genome: &ConductorPromptGenome) -> Result<String, String> {
    genome.validate()?;
    serde_json::to_vec(genome)
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("prompt genome serialization failed: {error}"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProTeacherAttestationV1, PromptProToAutoDistillationProvenanceV1};

    fn evolved_genome(effort: &str) -> ConductorPromptGenome {
        ConductorPromptGenome::seed_for_effort(effort)
            .mutations()
            .into_iter()
            .next()
            .expect("seed should have mutations")
    }

    #[test]
    fn valid_gepa_snapshot_round_trips_with_stable_fingerprints() {
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "auto",
            evolved_genome("auto"),
            "seed-auto-v1",
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("snapshot should validate");
        let encoded = serde_json::to_vec(&snapshot).expect("snapshot should serialize");
        let decoded = FrozenPromptProfileSnapshot::from_json_slice(&encoded)
            .expect("snapshot should round trip");

        assert_eq!(decoded, snapshot);
        assert_eq!(
            decoded.artifact_sha256().expect("snapshot should hash"),
            snapshot.artifact_sha256().expect("snapshot should hash")
        );
    }

    #[test]
    fn seed_genome_cannot_be_presented_as_frozen_gepa_evidence() {
        let error = FrozenPromptProfileSnapshot::new_gepa(
            "auto",
            ConductorPromptGenome::seed_for_effort("auto"),
            "older-stable",
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect_err("seed profile should be rejected");

        assert!(error.contains("non-seed evolution lineage"));
    }

    #[test]
    fn modified_genome_is_rejected_when_the_frozen_fingerprint_is_stale() {
        let mut snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            evolved_genome("pro"),
            "seed-auto-v1",
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("snapshot should validate");
        snapshot.genome.custom_directive = "changed after promotion".to_string();

        assert!(snapshot
            .validate()
            .expect_err("tampered snapshot should fail")
            .contains("fingerprint does not match"));
    }

    fn transfer_evidence() -> FrozenPromptTransferEvidence {
        FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: "auto-stable-v2".to_string(),
            source_profile_sha256: "c".repeat(64),
            dataset_sha256: "d".repeat(64),
            cohort_sha256: Some("f".repeat(64)),
            paired_evidence_sha256: "e".repeat(64),
            promotion_gate_protocol: PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(
                FrozenPromptSourceProfileLineageV1::undistilled("c".repeat(64)).unwrap(),
            ),
        }
    }

    fn pro_teacher_attestation() -> ProTeacherAttestationV1 {
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            evolved_genome("pro"),
            "seed-pro-v1",
            "1".repeat(64),
            "2".repeat(64),
        )
        .unwrap()
        .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: "auto-source".to_string(),
            source_profile_sha256: "3".repeat(64),
            dataset_sha256: "4".repeat(64),
            cohort_sha256: Some("5".repeat(64)),
            paired_evidence_sha256: "6".repeat(64),
            promotion_gate_protocol: PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(
                FrozenPromptSourceProfileLineageV1::undistilled("3".repeat(64)).unwrap(),
            ),
        })
        .unwrap();
        ProTeacherAttestationV1::from_stable_snapshot(&snapshot, &snapshot.genome.id).unwrap()
    }

    #[test]
    fn pro_snapshot_freezes_validated_auto_teacher_evidence() {
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            evolved_genome("pro"),
            "seed-pro-v1",
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("Pro snapshot should validate")
        .with_auto_teacher_evidence(transfer_evidence())
        .expect("Auto teacher evidence should validate for Pro");

        assert!(snapshot.validate().is_ok());
        assert_eq!(
            snapshot
                .auto_teacher_evidence
                .as_ref()
                .map(|evidence| evidence.source_profile_id.as_str()),
            Some("auto-stable-v2")
        );
    }

    #[test]
    fn auto_snapshot_rejects_cross_effort_teacher_evidence() {
        let error = FrozenPromptProfileSnapshot::new_gepa(
            "auto",
            evolved_genome("auto"),
            "seed-auto-v1",
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("Auto snapshot should validate")
        .with_auto_teacher_evidence(transfer_evidence())
        .expect_err("Auto must not consume its own evidence as cross-effort transfer");

        assert!(error.contains("only valid for Pro"));
    }

    #[test]
    fn auto_snapshot_freezes_fresh_pro_distillation_evidence() {
        let genome = evolved_genome("auto");
        let provenance = PromptProToAutoDistillationProvenanceV1::new(
            pro_teacher_attestation(),
            "seed-auto-v1",
            "7".repeat(64),
            genome.id.clone(),
            prompt_genome_sha256(&genome).unwrap(),
        )
        .unwrap();
        let evidence = FrozenPromptProToAutoDistillationEvidence::new(
            &provenance,
            "8".repeat(64),
            "9".repeat(64),
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
        )
        .unwrap();
        let snapshot = FrozenPromptProfileSnapshot::new_pro_to_auto_distillation(
            genome,
            "seed-auto-v1",
            "8".repeat(64),
            "c".repeat(64),
            evidence,
        )
        .expect("fresh Pro distillation evidence should freeze for Auto");

        assert!(snapshot.validate().is_ok());
        assert_eq!(
            snapshot.evolution_method,
            PromptEvolutionMethod::ProToAutoDistillation
        );
        assert!(snapshot.pro_teacher_evidence.is_some());
    }

    #[test]
    fn frozen_distillation_rejects_reused_teacher_or_holdout_evidence() {
        let teacher = pro_teacher_attestation();
        let provenance = PromptProToAutoDistillationProvenanceV1::new(
            teacher.clone(),
            "seed-auto-v1",
            "7".repeat(64),
            "auto-child",
            "8".repeat(64),
        )
        .unwrap();

        assert!(FrozenPromptProToAutoDistillationEvidence::new(
            &provenance,
            teacher.ordinary_dataset_sha256,
            "9".repeat(64),
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
        )
        .is_err());
        assert!(FrozenPromptProToAutoDistillationEvidence::new(
            &provenance,
            "8".repeat(64),
            "9".repeat(64),
            "a".repeat(64),
            "a".repeat(64),
            "c".repeat(64),
        )
        .is_err());
    }

    #[test]
    fn descendant_lineage_flattens_current_and_ancestor_evidence() {
        let teacher = pro_teacher_attestation();
        let provenance = PromptProToAutoDistillationProvenanceV1::new(
            teacher.clone(),
            "seed-auto-v1",
            "7".repeat(64),
            "auto-child",
            "8".repeat(64),
        )
        .unwrap();
        let evidence = FrozenPromptProToAutoDistillationEvidence::new(
            &provenance,
            "8".repeat(64),
            "9".repeat(64),
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
        )
        .unwrap();
        let lineage = FrozenPromptSourceProfileLineageV1::from_distillation(
            provenance.auto_child_profile_sha256,
            &evidence,
        )
        .unwrap();

        assert!(lineage.excludes_evidence_sha256(&evidence.dataset_sha256));
        assert!(lineage.excludes_evidence_sha256(&teacher.ordinary_dataset_sha256));
        assert!(lineage
            .ancestor_dataset_sha256
            .contains(&evidence.dataset_sha256));
        assert!(lineage
            .ancestor_dataset_sha256
            .contains(&teacher.auto_transfer_dataset_sha256));
        assert!(lineage.validate().is_ok());

        let mut legacy = serde_json::to_value(&lineage).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("ancestor_dataset_sha256");
        let legacy: FrozenPromptSourceProfileLineageV1 = serde_json::from_value(legacy).unwrap();
        assert!(legacy.validate().is_err());
    }
}
