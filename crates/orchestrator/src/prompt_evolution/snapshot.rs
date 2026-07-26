use super::ConductorPromptGenome;
use crate::sha256_hex;
use serde::{Deserialize, Serialize};

pub const FROZEN_PROMPT_PROFILE_SCHEMA: &str = "cindx.prompt-profile-snapshot.v1";
pub const PROMPT_PROMOTION_GATE_PROTOCOL: &str = "paired-wilson-task-diversity-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvolutionMethod {
    GepaReflectivePaired,
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
        };
        snapshot.validate()?;
        Ok(snapshot)
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
        if self.evolution_method != PromptEvolutionMethod::GepaReflectivePaired {
            return Err("unsupported prompt evolution method".to_string());
        }
        if self.promotion_gate_protocol != PROMPT_PROMOTION_GATE_PROTOCOL {
            return Err(format!(
                "unsupported prompt promotion gate protocol: {}",
                self.promotion_gate_protocol
            ));
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

    fn evolved_genome() -> ConductorPromptGenome {
        ConductorPromptGenome::seed_for_effort("auto")
            .mutations()
            .into_iter()
            .next()
            .expect("seed should have mutations")
    }

    #[test]
    fn valid_gepa_snapshot_round_trips_with_stable_fingerprints() {
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "auto",
            evolved_genome(),
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
            evolved_genome(),
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
}
