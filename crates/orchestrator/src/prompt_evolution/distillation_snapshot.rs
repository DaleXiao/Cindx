use super::{
    ProTeacherAttestationV1, PromptDistillationDirection, PromptProToAutoDistillationProvenanceV1,
    PRO_TO_AUTO_DISTILLATION_PROVENANCE_SCHEMA_V1,
};
use crate::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FROZEN_PROMPT_PRO_TO_AUTO_DISTILLATION_SCHEMA_V1: &str =
    "cindx.frozen-pro-to-auto-distillation-evidence.v1";
pub const PROMPT_PRO_TO_AUTO_DISTILLATION_GATE_PROTOCOL: &str =
    "pro-to-auto-matched-resource-non-regression-v1";
pub const FROZEN_PROMPT_SOURCE_PROFILE_LINEAGE_SCHEMA_V1: &str =
    "cindx.frozen-prompt-source-profile-lineage.v1";
const MAX_ANCESTOR_EVIDENCE_DIGESTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenPromptSourceProfileLineageV1 {
    pub schema: String,
    pub source_profile_sha256: String,
    pub ancestor_evidence_sha256: Vec<String>,
    #[serde(default)]
    pub ancestor_dataset_sha256: Vec<String>,
    pub lineage_sha256: String,
}

impl FrozenPromptSourceProfileLineageV1 {
    pub fn new(
        source_profile_sha256: impl Into<String>,
        ancestor_evidence_sha256: impl IntoIterator<Item = String>,
        ancestor_dataset_sha256: impl IntoIterator<Item = String>,
    ) -> Result<Self, String> {
        let source_profile_sha256 = source_profile_sha256.into();
        let ancestor_evidence_sha256 = ancestor_evidence_sha256
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let ancestor_dataset_sha256 = ancestor_dataset_sha256
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let lineage_sha256 = source_profile_lineage_sha256(
            &source_profile_sha256,
            &ancestor_evidence_sha256,
            &ancestor_dataset_sha256,
        )?;
        let lineage = Self {
            schema: FROZEN_PROMPT_SOURCE_PROFILE_LINEAGE_SCHEMA_V1.to_string(),
            source_profile_sha256,
            ancestor_evidence_sha256,
            ancestor_dataset_sha256,
            lineage_sha256,
        };
        lineage.validate()?;
        Ok(lineage)
    }

    pub fn undistilled(source_profile_sha256: impl Into<String>) -> Result<Self, String> {
        Self::new(source_profile_sha256, Vec::new(), Vec::new())
    }

    pub fn from_distillation(
        source_profile_sha256: impl Into<String>,
        evidence: &FrozenPromptProToAutoDistillationEvidence,
    ) -> Result<Self, String> {
        Self::new(
            source_profile_sha256,
            evidence.descendant_excluded_evidence_sha256()?,
            evidence.descendant_dataset_sha256(),
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        let canonical = self
            .ancestor_evidence_sha256
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let canonical_datasets = self
            .ancestor_dataset_sha256
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if self.schema != FROZEN_PROMPT_SOURCE_PROFILE_LINEAGE_SCHEMA_V1
            || !is_sha256(&self.source_profile_sha256)
            || self.ancestor_evidence_sha256.len() > MAX_ANCESTOR_EVIDENCE_DIGESTS
            || self.ancestor_dataset_sha256.len() > MAX_ANCESTOR_EVIDENCE_DIGESTS
            || canonical != self.ancestor_evidence_sha256
            || canonical_datasets != self.ancestor_dataset_sha256
            || self
                .ancestor_evidence_sha256
                .iter()
                .any(|digest| !is_sha256(digest))
            || self.ancestor_dataset_sha256.iter().any(|digest| {
                !is_sha256(digest) || self.ancestor_evidence_sha256.binary_search(digest).is_err()
            })
            || self.lineage_sha256
                != source_profile_lineage_sha256(
                    &self.source_profile_sha256,
                    &self.ancestor_evidence_sha256,
                    &self.ancestor_dataset_sha256,
                )?
        {
            return Err("frozen source profile lineage is malformed".to_string());
        }
        Ok(())
    }

    pub fn excludes_evidence_sha256(&self, digest: &str) -> bool {
        self.ancestor_evidence_sha256
            .binary_search_by(|candidate| candidate.as_str().cmp(digest))
            .is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenPromptProToAutoDistillationEvidence {
    pub schema: String,
    pub direction: PromptDistillationDirection,
    pub teacher: ProTeacherAttestationV1,
    pub teacher_attestation_sha256: String,
    pub auto_parent_profile_id: String,
    pub auto_parent_profile_sha256: String,
    pub auto_child_profile_id: String,
    pub auto_child_profile_sha256: String,
    pub dataset_sha256: String,
    pub cohort_sha256: String,
    pub train_evidence_sha256: String,
    pub holdout_evidence_sha256: String,
    pub paired_evidence_sha256: String,
    pub promotion_gate_protocol: String,
}

impl FrozenPromptProToAutoDistillationEvidence {
    pub fn new(
        provenance: &PromptProToAutoDistillationProvenanceV1,
        dataset_sha256: impl Into<String>,
        cohort_sha256: impl Into<String>,
        train_evidence_sha256: impl Into<String>,
        holdout_evidence_sha256: impl Into<String>,
        paired_evidence_sha256: impl Into<String>,
    ) -> Result<Self, String> {
        provenance.validate()?;
        let evidence = Self {
            schema: FROZEN_PROMPT_PRO_TO_AUTO_DISTILLATION_SCHEMA_V1.to_string(),
            direction: PromptDistillationDirection::ProToAutoDistillation,
            teacher: provenance.teacher.clone(),
            teacher_attestation_sha256: provenance.teacher_attestation_sha256.clone(),
            auto_parent_profile_id: provenance.auto_parent_profile_id.clone(),
            auto_parent_profile_sha256: provenance.auto_parent_profile_sha256.clone(),
            auto_child_profile_id: provenance.auto_child_profile_id.clone(),
            auto_child_profile_sha256: provenance.auto_child_profile_sha256.clone(),
            dataset_sha256: dataset_sha256.into(),
            cohort_sha256: cohort_sha256.into(),
            train_evidence_sha256: train_evidence_sha256.into(),
            holdout_evidence_sha256: holdout_evidence_sha256.into(),
            paired_evidence_sha256: paired_evidence_sha256.into(),
            promotion_gate_protocol: PROMPT_PRO_TO_AUTO_DISTILLATION_GATE_PROTOCOL.to_string(),
        };
        evidence.validate()?;
        Ok(evidence)
    }

    pub fn validate(&self) -> Result<(), String> {
        let provenance = PromptProToAutoDistillationProvenanceV1 {
            schema: PRO_TO_AUTO_DISTILLATION_PROVENANCE_SCHEMA_V1.to_string(),
            direction: self.direction,
            teacher: self.teacher.clone(),
            teacher_attestation_sha256: self.teacher_attestation_sha256.clone(),
            auto_parent_profile_id: self.auto_parent_profile_id.clone(),
            auto_parent_profile_sha256: self.auto_parent_profile_sha256.clone(),
            auto_child_profile_id: self.auto_child_profile_id.clone(),
            auto_child_profile_sha256: self.auto_child_profile_sha256.clone(),
        };
        provenance.validate()?;
        if self.schema != FROZEN_PROMPT_PRO_TO_AUTO_DISTILLATION_SCHEMA_V1
            || self.promotion_gate_protocol != PROMPT_PRO_TO_AUTO_DISTILLATION_GATE_PROTOCOL
        {
            return Err("frozen Pro-to-Auto distillation evidence is malformed".to_string());
        }
        let evidence_digests = [
            self.dataset_sha256.as_str(),
            self.cohort_sha256.as_str(),
            self.train_evidence_sha256.as_str(),
            self.holdout_evidence_sha256.as_str(),
            self.paired_evidence_sha256.as_str(),
        ];
        if evidence_digests
            .into_iter()
            .any(|digest| !is_sha256(digest) || self.teacher.excludes_evidence_sha256(digest))
            || BTreeSet::from(evidence_digests).len() != evidence_digests.len()
        {
            return Err(
                "distillation train and holdout evidence must be fresh and isolated".to_string(),
            );
        }
        Ok(())
    }

    pub fn descendant_excluded_evidence_sha256(&self) -> Result<Vec<String>, String> {
        self.validate()?;
        let mut digests = BTreeSet::from([
            self.teacher_attestation_sha256.clone(),
            self.dataset_sha256.clone(),
            self.cohort_sha256.clone(),
            self.train_evidence_sha256.clone(),
            self.holdout_evidence_sha256.clone(),
            self.paired_evidence_sha256.clone(),
        ]);
        digests.extend(self.teacher.excluded_evidence_sha256());
        if digests.len() > MAX_ANCESTOR_EVIDENCE_DIGESTS {
            return Err("distillation evidence lineage exceeds its bounded capacity".to_string());
        }
        Ok(digests.into_iter().collect())
    }

    pub fn descendant_dataset_sha256(&self) -> Vec<String> {
        let mut datasets = BTreeSet::from([
            self.teacher.ordinary_dataset_sha256.clone(),
            self.teacher.auto_transfer_dataset_sha256.clone(),
            self.dataset_sha256.clone(),
        ]);
        datasets.extend(
            self.teacher
                .auto_source_lineage
                .ancestor_dataset_sha256
                .iter()
                .cloned(),
        );
        datasets.into_iter().collect()
    }
}

fn source_profile_lineage_sha256(
    source_profile_sha256: &str,
    ancestor_evidence_sha256: &[String],
    ancestor_dataset_sha256: &[String],
) -> Result<String, String> {
    if !is_sha256(source_profile_sha256)
        || ancestor_evidence_sha256.len() > MAX_ANCESTOR_EVIDENCE_DIGESTS
        || ancestor_dataset_sha256.len() > MAX_ANCESTOR_EVIDENCE_DIGESTS
        || ancestor_evidence_sha256
            .iter()
            .any(|digest| !is_sha256(digest))
        || ancestor_dataset_sha256
            .iter()
            .any(|digest| !is_sha256(digest))
    {
        return Err("source profile lineage contains an invalid fingerprint".to_string());
    }
    serde_json::to_vec(&(
        FROZEN_PROMPT_SOURCE_PROFILE_LINEAGE_SCHEMA_V1,
        source_profile_sha256,
        ancestor_evidence_sha256,
        ancestor_dataset_sha256,
    ))
    .map(|encoded| sha256_hex(&encoded))
    .map_err(|error| format!("source profile lineage serialization failed: {error}"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
