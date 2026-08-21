use crate::{Metadata, LEARNING_EVIDENCE_METADATA_KEY};
use serde::{Deserialize, Serialize};
pub const LEARNING_EVIDENCE_SCHEMA_V1: &str = "cindx.learning-evidence.v1";
pub const LEARNING_EVIDENCE_MAX_BYTES: usize = 1_024;
const LEARNING_EVIDENCE_FIELDS_V1: [&str; 10] = [
    "schema",
    "termination",
    "disposition",
    "verification",
    "attribution",
    "usage_completeness",
    "steer_epoch",
    "budget_fingerprint",
    "independent_quality_source",
    "quality_bps",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LearningEvidenceSchema {
    #[default]
    #[serde(rename = "cindx.learning-evidence.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningTermination {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningDisposition {
    Positive,
    Negative,
    #[default]
    Censored,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningVerification {
    Passed,
    Failed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningAttribution {
    Model,
    Workflow,
    Tool,
    Provider,
    System,
    User,
    Permission,
    Budget,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningUsageCompleteness {
    Complete,
    Partial,
    #[default]
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndependentQualitySource {
    CollaborationQualityGate,
    AnytimeSelector,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningEvidenceV1 {
    pub schema: LearningEvidenceSchema,
    pub termination: LearningTermination,
    pub disposition: LearningDisposition,
    pub verification: LearningVerification,
    pub attribution: LearningAttribution,
    pub usage_completeness: LearningUsageCompleteness,
    pub steer_epoch: Option<u64>,
    pub budget_fingerprint: Option<String>,
    pub independent_quality_source: Option<IndependentQualitySource>,
    pub quality_bps: Option<u16>,
}

impl LearningEvidenceV1 {
    pub fn censored(
        termination: LearningTermination,
        attribution: LearningAttribution,
        usage_completeness: LearningUsageCompleteness,
        steer_epoch: Option<u64>,
        budget_fingerprint: Option<String>,
    ) -> Self {
        Self {
            termination,
            attribution,
            usage_completeness,
            steer_epoch,
            budget_fingerprint,
            ..Self::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn independent_quality(
        termination: LearningTermination,
        attribution: LearningAttribution,
        usage_completeness: LearningUsageCompleteness,
        steer_epoch: u64,
        budget_fingerprint: String,
        source: IndependentQualitySource,
        quality_bps: u16,
        passed: bool,
    ) -> Self {
        Self {
            schema: LearningEvidenceSchema::V1,
            termination,
            disposition: if passed {
                LearningDisposition::Positive
            } else {
                LearningDisposition::Negative
            },
            verification: if passed {
                LearningVerification::Passed
            } else {
                LearningVerification::Failed
            },
            attribution,
            usage_completeness,
            steer_epoch: Some(steer_epoch),
            budget_fingerprint: Some(budget_fingerprint),
            independent_quality_source: Some(source),
            quality_bps: Some(quality_bps),
        }
    }

    pub fn verified_postcondition(
        usage_completeness: LearningUsageCompleteness,
        steer_epoch: u64,
        budget_fingerprint: String,
    ) -> Self {
        Self {
            schema: LearningEvidenceSchema::V1,
            termination: LearningTermination::Completed,
            disposition: LearningDisposition::Positive,
            verification: LearningVerification::Passed,
            attribution: LearningAttribution::Tool,
            usage_completeness,
            steer_epoch: Some(steer_epoch),
            budget_fingerprint: Some(budget_fingerprint),
            independent_quality_source: None,
            quality_bps: None,
        }
    }

    pub fn from_metadata(metadata: &Metadata) -> Option<Self> {
        let encoded = metadata.get(LEARNING_EVIDENCE_METADATA_KEY)?;
        if encoded.len() > LEARNING_EVIDENCE_MAX_BYTES {
            return None;
        }
        let evidence = serde_json::from_str::<Self>(encoded).ok()?;
        let value = serde_json::from_str::<serde_json::Value>(encoded).ok()?;
        let fields = value.as_object()?;
        if fields.len() != LEARNING_EVIDENCE_FIELDS_V1.len()
            || LEARNING_EVIDENCE_FIELDS_V1
                .iter()
                .any(|field| !fields.contains_key(*field))
        {
            return None;
        }
        evidence.contract_is_valid().then_some(evidence)
    }

    pub fn to_metadata_value(&self) -> Option<String> {
        if !self.contract_is_valid() {
            return None;
        }
        let encoded = serde_json::to_string(self).ok()?;
        (encoded.len() <= LEARNING_EVIDENCE_MAX_BYTES).then_some(encoded)
    }

    pub fn is_learnable(&self) -> bool {
        self.disposition != LearningDisposition::Censored && self.contract_is_valid()
    }

    pub fn contract_is_valid(&self) -> bool {
        if self.quality_bps.is_some_and(|quality| quality > 10_000)
            || self
                .budget_fingerprint
                .as_deref()
                .is_some_and(|fingerprint| !is_sha256_hex(fingerprint))
            || serde_json::to_vec(self)
                .map(|encoded| encoded.len() > LEARNING_EVIDENCE_MAX_BYTES)
                .unwrap_or(true)
        {
            return false;
        }
        if self.disposition == LearningDisposition::Censored {
            return true;
        }
        if self.usage_completeness == LearningUsageCompleteness::Missing
            || self.steer_epoch.is_none()
            || self.budget_fingerprint.is_none()
            || self.termination != LearningTermination::Completed
        {
            return false;
        }
        match self.disposition {
            LearningDisposition::Positive => {
                if self.verification != LearningVerification::Passed {
                    return false;
                }
            }
            LearningDisposition::Negative => {
                if self.verification != LearningVerification::Failed {
                    return false;
                }
            }
            LearningDisposition::Censored => return true,
        }
        match (
            self.independent_quality_source,
            self.quality_bps,
            self.attribution,
        ) {
            (Some(_), Some(_), LearningAttribution::Model | LearningAttribution::Workflow) => true,
            (None, None, LearningAttribution::Tool) => {
                self.disposition == LearningDisposition::Positive
            }
            _ => false,
        }
    }

    pub fn quality_score(&self) -> Option<f32> {
        self.quality_bps
            .map(|quality| f32::from(quality) / 10_000.0)
    }

    pub fn verification_passed(&self) -> Option<bool> {
        match self.verification {
            LearningVerification::Passed => Some(true),
            LearningVerification::Failed => Some(false),
            LearningVerification::Unknown => None,
        }
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
