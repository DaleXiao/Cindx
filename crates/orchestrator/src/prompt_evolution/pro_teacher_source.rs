use super::{
    FrozenPromptProfileSnapshot, FrozenPromptSourceProfileLineageV1,
    PROMPT_AUTO_TRANSFER_GATE_PROTOCOL,
};
use crate::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PRO_TEACHER_ATTESTATION_SCHEMA_V1: &str = "cindx.pro-teacher-attestation.v1";
pub const PRO_TO_AUTO_DISTILLATION_PROVENANCE_SCHEMA_V1: &str =
    "cindx.pro-to-auto-distillation-provenance.v1";
pub const PRO_TEACHER_SOURCE_EVIDENCE_PROTOCOL_V1: &str = "frozen-pro-dual-gate-source-evidence-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptDistillationDirection {
    ProToAutoDistillation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProTeacherAttestationV1 {
    pub schema: String,
    pub direction: PromptDistillationDirection,
    pub source_evidence_protocol: String,
    pub teacher_profile_id: String,
    pub teacher_profile_sha256: String,
    pub teacher_snapshot_sha256: String,
    pub ordinary_dataset_sha256: String,
    pub ordinary_paired_evidence_sha256: String,
    pub auto_transfer_dataset_sha256: String,
    pub auto_transfer_cohort_sha256: String,
    pub auto_transfer_paired_evidence_sha256: String,
    pub auto_source_lineage: FrozenPromptSourceProfileLineageV1,
    pub source_evidence_sha256: String,
}

impl ProTeacherAttestationV1 {
    pub fn from_stable_snapshot(
        snapshot: &FrozenPromptProfileSnapshot,
        active_stable_profile_id: &str,
    ) -> Result<Self, String> {
        snapshot.validate()?;
        if snapshot.effort != "pro" || snapshot.genome.id != active_stable_profile_id {
            return Err("Pro teacher must be the active stable frozen Pro profile".to_string());
        }
        let transfer = snapshot.auto_teacher_evidence.as_ref().ok_or_else(|| {
            "Pro teacher snapshot is missing certified Auto transfer evidence".to_string()
        })?;
        transfer.validate()?;
        if transfer.promotion_gate_protocol != PROMPT_AUTO_TRANSFER_GATE_PROTOCOL {
            return Err(
                "Pro teacher requires the current source-attested transfer gate".to_string(),
            );
        }
        let auto_transfer_cohort_sha256 = transfer.cohort_sha256.clone().ok_or_else(|| {
            "Pro teacher transfer evidence is missing its frozen cohort".to_string()
        })?;
        let auto_source_lineage = transfer.source_profile_lineage.clone().ok_or_else(|| {
            "Pro teacher transfer evidence is missing source profile lineage".to_string()
        })?;
        auto_source_lineage.validate()?;
        let teacher_snapshot_sha256 = snapshot.artifact_sha256()?;
        let source_evidence_sha256 = source_evidence_sha256(
            &snapshot.dataset_sha256,
            &snapshot.paired_evidence_sha256,
            &transfer.dataset_sha256,
            &auto_transfer_cohort_sha256,
            &transfer.paired_evidence_sha256,
            &auto_source_lineage.lineage_sha256,
        )?;
        let attestation = Self {
            schema: PRO_TEACHER_ATTESTATION_SCHEMA_V1.to_string(),
            direction: PromptDistillationDirection::ProToAutoDistillation,
            source_evidence_protocol: PRO_TEACHER_SOURCE_EVIDENCE_PROTOCOL_V1.to_string(),
            teacher_profile_id: snapshot.genome.id.clone(),
            teacher_profile_sha256: snapshot.candidate_sha256.clone(),
            teacher_snapshot_sha256,
            ordinary_dataset_sha256: snapshot.dataset_sha256.clone(),
            ordinary_paired_evidence_sha256: snapshot.paired_evidence_sha256.clone(),
            auto_transfer_dataset_sha256: transfer.dataset_sha256.clone(),
            auto_transfer_cohort_sha256,
            auto_transfer_paired_evidence_sha256: transfer.paired_evidence_sha256.clone(),
            auto_source_lineage,
            source_evidence_sha256,
        };
        attestation.validate()?;
        Ok(attestation)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.auto_source_lineage.validate()?;
        if self.schema != PRO_TEACHER_ATTESTATION_SCHEMA_V1
            || self.direction != PromptDistillationDirection::ProToAutoDistillation
            || self.source_evidence_protocol != PRO_TEACHER_SOURCE_EVIDENCE_PROTOCOL_V1
            || !is_profile_id(&self.teacher_profile_id)
            || [
                &self.teacher_profile_sha256,
                &self.teacher_snapshot_sha256,
                &self.ordinary_dataset_sha256,
                &self.ordinary_paired_evidence_sha256,
                &self.auto_transfer_dataset_sha256,
                &self.auto_transfer_cohort_sha256,
                &self.auto_transfer_paired_evidence_sha256,
                &self.auto_source_lineage.lineage_sha256,
                &self.source_evidence_sha256,
            ]
            .into_iter()
            .any(|digest| !is_sha256(digest))
        {
            return Err("Pro teacher attestation is malformed".to_string());
        }
        let expected = source_evidence_sha256(
            &self.ordinary_dataset_sha256,
            &self.ordinary_paired_evidence_sha256,
            &self.auto_transfer_dataset_sha256,
            &self.auto_transfer_cohort_sha256,
            &self.auto_transfer_paired_evidence_sha256,
            &self.auto_source_lineage.lineage_sha256,
        )?;
        if self.source_evidence_sha256 != expected {
            return Err("Pro teacher source evidence fingerprint does not match".to_string());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("Pro teacher attestation serialization failed: {error}"))
    }

    pub fn excludes_evidence_sha256(&self, digest: &str) -> bool {
        self.auto_source_lineage.excludes_evidence_sha256(digest)
            || [
                self.ordinary_dataset_sha256.as_str(),
                self.ordinary_paired_evidence_sha256.as_str(),
                self.auto_transfer_dataset_sha256.as_str(),
                self.auto_transfer_cohort_sha256.as_str(),
                self.auto_transfer_paired_evidence_sha256.as_str(),
                self.auto_source_lineage.lineage_sha256.as_str(),
                self.source_evidence_sha256.as_str(),
            ]
            .contains(&digest)
    }

    pub fn excluded_evidence_sha256(&self) -> Vec<String> {
        let mut digests = BTreeSet::from([
            self.ordinary_dataset_sha256.clone(),
            self.ordinary_paired_evidence_sha256.clone(),
            self.auto_transfer_dataset_sha256.clone(),
            self.auto_transfer_cohort_sha256.clone(),
            self.auto_transfer_paired_evidence_sha256.clone(),
            self.auto_source_lineage.lineage_sha256.clone(),
            self.source_evidence_sha256.clone(),
        ]);
        digests.extend(
            self.auto_source_lineage
                .ancestor_evidence_sha256
                .iter()
                .cloned(),
        );
        digests.into_iter().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptProToAutoDistillationProvenanceV1 {
    pub schema: String,
    pub direction: PromptDistillationDirection,
    pub teacher: ProTeacherAttestationV1,
    pub teacher_attestation_sha256: String,
    pub auto_parent_profile_id: String,
    pub auto_parent_profile_sha256: String,
    pub auto_child_profile_id: String,
    pub auto_child_profile_sha256: String,
}

impl PromptProToAutoDistillationProvenanceV1 {
    pub fn new(
        teacher: ProTeacherAttestationV1,
        auto_parent_profile_id: impl Into<String>,
        auto_parent_profile_sha256: impl Into<String>,
        auto_child_profile_id: impl Into<String>,
        auto_child_profile_sha256: impl Into<String>,
    ) -> Result<Self, String> {
        let teacher_attestation_sha256 = teacher.digest()?;
        let provenance = Self {
            schema: PRO_TO_AUTO_DISTILLATION_PROVENANCE_SCHEMA_V1.to_string(),
            direction: PromptDistillationDirection::ProToAutoDistillation,
            teacher,
            teacher_attestation_sha256,
            auto_parent_profile_id: auto_parent_profile_id.into(),
            auto_parent_profile_sha256: auto_parent_profile_sha256.into(),
            auto_child_profile_id: auto_child_profile_id.into(),
            auto_child_profile_sha256: auto_child_profile_sha256.into(),
        };
        provenance.validate()?;
        Ok(provenance)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.teacher.validate()?;
        if self.schema != PRO_TO_AUTO_DISTILLATION_PROVENANCE_SCHEMA_V1
            || self.direction != PromptDistillationDirection::ProToAutoDistillation
            || self.teacher.direction != self.direction
            || self.teacher_attestation_sha256 != self.teacher.digest()?
            || !is_profile_id(&self.auto_parent_profile_id)
            || !is_profile_id(&self.auto_child_profile_id)
            || !is_sha256(&self.auto_parent_profile_sha256)
            || !is_sha256(&self.auto_child_profile_sha256)
        {
            return Err("Pro-to-Auto distillation provenance is malformed".to_string());
        }
        let profile_ids = BTreeSet::from([
            self.teacher.teacher_profile_id.as_str(),
            self.auto_parent_profile_id.as_str(),
            self.auto_child_profile_id.as_str(),
        ]);
        let profile_sha256 = BTreeSet::from([
            self.teacher.teacher_profile_sha256.as_str(),
            self.auto_parent_profile_sha256.as_str(),
            self.auto_child_profile_sha256.as_str(),
        ]);
        if profile_ids.len() != 3 || profile_sha256.len() != 3 {
            return Err(
                "Pro-to-Auto distillation cannot self-bootstrap or copy its teacher".to_string(),
            );
        }
        Ok(())
    }

    /// Checks digest lineage only. The canonical read model must separately reject
    /// overlap between source and distillation case identities.
    pub fn permits_evaluation_digest_lineage(
        &self,
        dataset_sha256: &str,
        cohort_sha256: Option<&str>,
    ) -> bool {
        self.validate().is_ok()
            && is_sha256(dataset_sha256)
            && !self.teacher.excludes_evidence_sha256(dataset_sha256)
            && cohort_sha256.is_none_or(|digest| {
                is_sha256(digest)
                    && digest != dataset_sha256
                    && !self.teacher.excludes_evidence_sha256(digest)
            })
    }
}

fn source_evidence_sha256(
    ordinary_dataset_sha256: &str,
    ordinary_paired_evidence_sha256: &str,
    auto_transfer_dataset_sha256: &str,
    auto_transfer_cohort_sha256: &str,
    auto_transfer_paired_evidence_sha256: &str,
    auto_source_lineage_sha256: &str,
) -> Result<String, String> {
    let digests = [
        ordinary_dataset_sha256,
        ordinary_paired_evidence_sha256,
        auto_transfer_dataset_sha256,
        auto_transfer_cohort_sha256,
        auto_transfer_paired_evidence_sha256,
        auto_source_lineage_sha256,
    ];
    if digests.into_iter().any(|digest| !is_sha256(digest)) {
        return Err("Pro teacher source evidence contains an invalid fingerprint".to_string());
    }
    serde_json::to_vec(&(PRO_TEACHER_SOURCE_EVIDENCE_PROTOCOL_V1, digests))
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("Pro teacher source evidence serialization failed: {error}"))
}

fn is_profile_id(value: &str) -> bool {
    !value.trim().is_empty() && value == value.trim() && value.len() <= 256
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConductorPromptGenome, FrozenPromptTransferEvidence, PROMPT_AUTO_TRANSFER_GATE_PROTOCOL,
    };

    fn certified_pro_snapshot() -> FrozenPromptProfileSnapshot {
        let genome = ConductorPromptGenome::seed_for_effort("pro")
            .mutations()
            .into_iter()
            .next()
            .expect("Pro seed should mutate");
        FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            genome,
            "seed-pro-v1",
            "1".repeat(64),
            "2".repeat(64),
        )
        .expect("Pro snapshot should validate")
        .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: "auto-stable-v2".to_string(),
            source_profile_sha256: "3".repeat(64),
            dataset_sha256: "4".repeat(64),
            cohort_sha256: Some("5".repeat(64)),
            paired_evidence_sha256: "6".repeat(64),
            promotion_gate_protocol: PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(
                FrozenPromptSourceProfileLineageV1::undistilled("3".repeat(64)).unwrap(),
            ),
        })
        .expect("source-attested transfer evidence should validate")
    }

    #[test]
    fn teacher_attestation_requires_the_active_certified_pro_snapshot() {
        let snapshot = certified_pro_snapshot();
        let attestation =
            ProTeacherAttestationV1::from_stable_snapshot(&snapshot, snapshot.genome.id.as_str())
                .expect("active certified Pro should be attestable");

        assert_eq!(
            attestation.teacher_profile_sha256,
            snapshot.candidate_sha256
        );
        assert_eq!(
            attestation.teacher_snapshot_sha256,
            snapshot.artifact_sha256().unwrap()
        );
        assert!(ProTeacherAttestationV1::from_stable_snapshot(&snapshot, "stale-pro").is_err());
    }

    #[test]
    fn legacy_snapshot_without_source_lineage_cannot_become_a_teacher() {
        let mut snapshot = certified_pro_snapshot();
        snapshot
            .auto_teacher_evidence
            .as_mut()
            .unwrap()
            .source_profile_lineage = None;

        assert!(snapshot.validate().is_ok());
        assert!(ProTeacherAttestationV1::from_stable_snapshot(
            &snapshot,
            snapshot.genome.id.as_str(),
        )
        .is_err());
    }

    #[test]
    fn provenance_rejects_self_bootstrap_and_teacher_copying() {
        let snapshot = certified_pro_snapshot();
        let teacher =
            ProTeacherAttestationV1::from_stable_snapshot(&snapshot, snapshot.genome.id.as_str())
                .unwrap();

        assert!(PromptProToAutoDistillationProvenanceV1::new(
            teacher.clone(),
            "auto-parent",
            "7".repeat(64),
            "auto-parent",
            "8".repeat(64),
        )
        .is_err());
        assert!(PromptProToAutoDistillationProvenanceV1::new(
            teacher.clone(),
            "auto-parent",
            "7".repeat(64),
            "auto-child",
            teacher.teacher_profile_sha256.clone(),
        )
        .is_err());
    }

    #[test]
    fn distillation_evaluation_cannot_reuse_teacher_evidence() {
        let snapshot = certified_pro_snapshot();
        let teacher =
            ProTeacherAttestationV1::from_stable_snapshot(&snapshot, snapshot.genome.id.as_str())
                .unwrap();
        let source_dataset = teacher.ordinary_dataset_sha256.clone();
        let provenance = PromptProToAutoDistillationProvenanceV1::new(
            teacher,
            "auto-parent",
            "7".repeat(64),
            "auto-child",
            "8".repeat(64),
        )
        .unwrap();

        assert!(
            !provenance.permits_evaluation_digest_lineage(&source_dataset, Some(&"9".repeat(64)))
        );
        assert!(
            provenance.permits_evaluation_digest_lineage(&"9".repeat(64), Some(&"a".repeat(64)))
        );
    }
}
