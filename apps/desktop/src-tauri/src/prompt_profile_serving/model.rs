use super::{
    scope_sha256, MAX_ASSIGNMENT_RECEIPT_BYTES, MAX_PROFILE_ID_BYTES,
    PROMPT_PROFILE_ASSIGNMENT_SCHEMA, PROMPT_PROFILE_DEPLOYMENT_SCHEMA,
    PROMPT_PROFILE_RECORD_SCHEMA,
};
use crate::{
    runtime_constants::PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1,
    view_models::PromptDistillationCanaryLeaseV1,
};
use orchestrator::{prompt_genome_sha256, sha256_hex, ConductorPromptGenome};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeployedPromptProfile {
    pub(crate) genome: ConductorPromptGenome,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PromptProfileDeployment {
    pub(super) schema: String,
    pub(super) scope_sha256: String,
    pub(super) effort: String,
    pub(crate) stable: DeployedPromptProfile,
    pub(crate) canary: Option<DeployedPromptProfile>,
    pub(super) rollout_status: String,
    pub(super) canary_percent: u8,
    pub(super) source_revision: u64,
    pub(crate) distillation_lease: Option<PromptProfileDistillationLease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PromptProfileDeploymentState {
    Active,
    Withdrawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PromptProfileDeploymentRecord {
    schema: String,
    scope_sha256: String,
    effort: String,
    pub(super) generation: u64,
    pub(super) state: PromptProfileDeploymentState,
    pub(super) deployment: Option<PromptProfileDeployment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PromptProfileDistillationLease {
    pub(crate) schema: String,
    pub(crate) candidate_profile_id: String,
    pub(crate) candidate_profile_sha256: String,
    pub(crate) stable_profile_id: String,
    pub(crate) stable_profile_sha256: String,
    pub(crate) cohort_sha256: String,
    pub(crate) paired_evidence_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PromptProfileAssignmentSource {
    Stable,
    Canary,
    Frozen,
    SeedFallback,
}

impl PromptProfileAssignmentSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Canary => "canary",
            Self::Frozen => "frozen",
            Self::SeedFallback => "seed_fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PromptProfileFallback {
    Fast,
    EvolutionDisabled,
    NoDeployment,
    Invalid,
    Storage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PromptProfileOperationCounts {
    pub(crate) read_model_loads: u8,
    pub(crate) profile_validations: u8,
    pub(crate) observation_rows_scanned: u64,
    pub(crate) history_rows_scanned: u64,
    pub(crate) writes: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PromptProfileAssignmentReceipt {
    pub(crate) schema: String,
    pub(crate) effort: String,
    pub(crate) source: PromptProfileAssignmentSource,
    pub(crate) fallback: Option<PromptProfileFallback>,
    pub(crate) scope_sha256: Option<String>,
    pub(crate) run_identity_sha256: Option<String>,
    pub(crate) profile_id: String,
    pub(crate) profile_sha256: String,
    pub(crate) stable_profile_sha256: String,
    pub(crate) canary_profile_sha256: Option<String>,
    pub(crate) rollout_status: Option<String>,
    pub(crate) canary_percent: u8,
    pub(crate) source_revision: Option<u64>,
    pub(crate) deployment_generation: Option<u64>,
    pub(crate) distillation_lease: Option<PromptProfileDistillationLease>,
    pub(crate) operations: PromptProfileOperationCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptProfileSelection {
    pub(crate) genome: ConductorPromptGenome,
    pub(crate) receipt: PromptProfileAssignmentReceipt,
}

impl PromptProfileSelection {
    pub(crate) fn receipt_json(&self) -> Result<String, String> {
        let encoded = serde_json::to_string(&self.receipt)
            .map_err(|error| format!("prompt profile receipt serialization failed: {error}"))?;
        if encoded.len() > MAX_ASSIGNMENT_RECEIPT_BYTES {
            return Err("prompt profile assignment receipt exceeds 4 KiB".to_string());
        }
        Ok(encoded)
    }

    pub(crate) fn receipt_json_and_sha256(&self) -> Result<(String, String), String> {
        let receipt = self.receipt_json()?;
        let digest = sha256_hex(receipt.as_bytes());
        Ok((receipt, digest))
    }

    pub(crate) fn source_label(&self) -> String {
        if self.receipt.source == PromptProfileAssignmentSource::Canary {
            format!("canary_{}", self.receipt.canary_percent)
        } else {
            self.receipt.source.label().to_string()
        }
    }

    pub(crate) fn rollout_status(&self) -> Option<&str> {
        self.receipt.rollout_status.as_deref()
    }
}

impl PromptProfileDeployment {
    pub(super) fn validate_for(&self, scope: &str, effort: &str) -> Result<(), String> {
        if self.schema != PROMPT_PROFILE_DEPLOYMENT_SCHEMA
            || self.scope_sha256 != scope_sha256(scope)
            || self.effort != effort
            || !matches!(effort, "auto" | "pro")
        {
            return Err("prompt profile deployment identity is invalid".to_string());
        }
        validate_profile(&self.stable.genome)?;
        if self.stable.sha256 != prompt_genome_sha256(&self.stable.genome)? {
            return Err("stable prompt profile digest mismatch".to_string());
        }
        if !matches!(self.canary_percent, 0 | 10 | 25 | 50)
            || (self.canary.is_none() && self.canary_percent != 0)
        {
            return Err("prompt profile canary deployment is invalid".to_string());
        }
        if self.rollout_status.is_empty() || self.rollout_status.len() > 64 {
            return Err("prompt profile rollout status is invalid".to_string());
        }
        if let Some(canary) = &self.canary {
            validate_profile(&canary.genome)?;
            if canary.sha256 != prompt_genome_sha256(&canary.genome)?
                || canary.genome.id == self.stable.genome.id
            {
                return Err("canary prompt profile is invalid".to_string());
            }
        }
        validate_distillation_lease(self)?;
        Ok(())
    }
}

impl PromptProfileDeploymentRecord {
    pub(super) fn validate_for(&self, scope: &str, effort: &str) -> Result<(), String> {
        if self.schema != PROMPT_PROFILE_RECORD_SCHEMA
            || self.scope_sha256 != scope_sha256(scope)
            || self.effort != effort
            || self.generation == 0
        {
            return Err("prompt profile deployment record identity is invalid".to_string());
        }
        match (&self.state, &self.deployment) {
            (PromptProfileDeploymentState::Active, Some(deployment)) => {
                deployment.validate_for(scope, effort)
            }
            (PromptProfileDeploymentState::Withdrawn, None) => Ok(()),
            _ => Err("prompt profile deployment record state is invalid".to_string()),
        }
    }

}

impl TryFrom<&PromptDistillationCanaryLeaseV1> for PromptProfileDistillationLease {
    type Error = String;

    fn try_from(lease: &PromptDistillationCanaryLeaseV1) -> Result<Self, Self::Error> {
        let projected = Self {
            schema: lease.schema.clone(),
            candidate_profile_id: lease.candidate_profile_id.clone(),
            candidate_profile_sha256: lease.candidate_profile_sha256.clone(),
            stable_profile_id: lease.stable_profile_id.clone(),
            stable_profile_sha256: lease.stable_profile_sha256.clone(),
            cohort_sha256: lease.cohort_sha256.clone(),
            paired_evidence_sha256: lease.paired_evidence_sha256.clone(),
        };
        projected.validate()?;
        Ok(projected)
    }
}

impl PromptProfileDistillationLease {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1
            || self.candidate_profile_id.trim().is_empty()
            || self.stable_profile_id.trim().is_empty()
            || [
                self.candidate_profile_sha256.as_str(),
                self.stable_profile_sha256.as_str(),
                self.cohort_sha256.as_str(),
                self.paired_evidence_sha256.as_str(),
            ]
            .iter()
            .any(|digest| !is_sha256(digest))
        {
            return Err("prompt profile distillation lease is invalid".to_string());
        }
        Ok(())
    }
}

fn validate_distillation_lease(deployment: &PromptProfileDeployment) -> Result<(), String> {
    let Some(lease) = deployment.distillation_lease.as_ref() else {
        return Ok(());
    };
    lease.validate()?;
    let canary = deployment
        .canary
        .as_ref()
        .ok_or_else(|| "prompt profile distillation lease has no canary".to_string())?;
    if lease.candidate_profile_id != canary.genome.id
        || lease.candidate_profile_sha256 != canary.sha256
        || lease.stable_profile_id != deployment.stable.genome.id
        || lease.stable_profile_sha256 != deployment.stable.sha256
    {
        return Err("prompt profile distillation lease does not match deployment".to_string());
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn validate_profile(genome: &ConductorPromptGenome) -> Result<(), String> {
    genome.validate()?;
    if genome.id.len() > MAX_PROFILE_ID_BYTES {
        return Err("prompt profile id exceeds 128 bytes".to_string());
    }
    Ok(())
}

pub(super) fn new_receipt(
    effort: &str,
    source: PromptProfileAssignmentSource,
    fallback: Option<PromptProfileFallback>,
    profile_id: String,
    profile_sha256: String,
) -> PromptProfileAssignmentReceipt {
    PromptProfileAssignmentReceipt {
        schema: PROMPT_PROFILE_ASSIGNMENT_SCHEMA.to_string(),
        effort: effort.to_string(),
        source,
        fallback,
        scope_sha256: None,
        run_identity_sha256: None,
        profile_id,
        stable_profile_sha256: profile_sha256.clone(),
        canary_profile_sha256: None,
        profile_sha256,
        rollout_status: None,
        canary_percent: 0,
        source_revision: None,
        deployment_generation: None,
        distillation_lease: None,
        operations: PromptProfileOperationCounts {
            read_model_loads: 0,
            profile_validations: 1,
            observation_rows_scanned: 0,
            history_rows_scanned: 0,
            writes: 0,
        },
    }
}
