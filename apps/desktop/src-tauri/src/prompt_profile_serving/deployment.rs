use super::model::{validate_profile, DeployedPromptProfile, PromptProfileDistillationLease};
use super::{scope_sha256, valid_scope, PromptProfileDeployment, PROMPT_PROFILE_DEPLOYMENT_SCHEMA};
use crate::prompt_evolution_hot_state::{
    prompt_model_genome_conflict_keys, prompt_rollout_key, PROMPT_EVIDENCE_SCOPE_SEPARATOR,
};
use crate::view_models::{PromptEvolutionReadModel, PromptGenomeRecord, PromptRolloutState};
use orchestrator::{
    prompt_genome_sha256, sha256_hex, ConductorPromptGenome, PromptEvolutionMethod,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

type ProfileKey = (String, String, String);
type ScopeEffort = (String, String);

pub(super) struct PromptProfileDeploymentCatalog<'a> {
    model: &'a PromptEvolutionReadModel,
    profiles: BTreeMap<ProfileKey, &'a PromptGenomeRecord>,
    conflicts: BTreeSet<ProfileKey>,
    scope_efforts: BTreeSet<ScopeEffort>,
}

#[derive(Serialize)]
struct CanonicalPromptProfileSnapshot {
    schema: &'static str,
    scope_sha256: String,
    effort: String,
    active_deployment: Option<String>,
    genomes: Vec<String>,
    genome_identity_fingerprints: BTreeMap<String, String>,
    rollout: Option<String>,
}

struct ResolvedProfile {
    genome: ConductorPromptGenome,
    evolution_method: Option<PromptEvolutionMethod>,
}

impl<'a> PromptProfileDeploymentCatalog<'a> {
    pub(super) fn new(model: &'a PromptEvolutionReadModel) -> Self {
        let conflicts = prompt_model_genome_conflict_keys(model);
        let mut profiles = BTreeMap::new();
        let mut scope_efforts = BTreeSet::new();
        for record in &model.genomes {
            if valid_scope(&record.scope) && matches!(record.effort.as_str(), "auto" | "pro") {
                let pair = (record.scope.clone(), record.effort.clone());
                scope_efforts.insert(pair);
                let key = (
                    record.scope.clone(),
                    record.effort.clone(),
                    record.genome.id.clone(),
                );
                if !conflicts.contains(&key) {
                    profiles.entry(key).or_insert(record);
                }
            }
        }
        for identity in model.genome_identity_fingerprints.keys() {
            if let Ok((scope, effort, _)) = serde_json::from_str::<ProfileKey>(identity) {
                if valid_scope(&scope) && matches!(effort.as_str(), "auto" | "pro") {
                    scope_efforts.insert((scope, effort));
                }
            }
        }
        for key in model.rollouts.keys() {
            if let Some(pair) = rollout_scope_effort(key) {
                scope_efforts.insert(pair);
            }
        }
        Self {
            model,
            profiles,
            conflicts,
            scope_efforts,
        }
    }

    pub(super) fn scope_efforts(&self) -> impl Iterator<Item = &ScopeEffort> {
        self.scope_efforts.iter()
    }

    pub(super) fn build(
        &self,
        scope: &str,
        effort: &str,
        source_revision: u64,
    ) -> Result<PromptProfileDeployment, String> {
        let scope = scope.trim();
        if !valid_scope(scope)
            || !matches!(effort, "auto" | "pro")
            || source_revision != self.model.revision
        {
            return Err("prompt profile deployment source is invalid".to_string());
        }
        let rollout = self
            .rollout(scope, effort)
            .ok_or_else(|| format!("missing {effort} prompt rollout"))?;
        if self.rollout_references_conflict(scope, effort, rollout) {
            return Err("prompt rollout references a conflicted profile".to_string());
        }
        let stable = self.resolve_stable(scope, effort, rollout)?;
        let canary = rollout
            .canary_profile_id
            .as_deref()
            .map(|id| self.resolve_record(scope, effort, id))
            .transpose()?;
        let distillation_lease = distillation_lease(rollout, effort, &stable, canary.as_ref())?;
        let deployment = PromptProfileDeployment {
            schema: PROMPT_PROFILE_DEPLOYMENT_SCHEMA.to_string(),
            scope_sha256: scope_sha256(scope),
            effort: effort.to_string(),
            stable: deployed(stable.genome)?,
            canary: canary.map(|profile| deployed(profile.genome)).transpose()?,
            rollout_status: rollout.status.clone(),
            canary_percent: rollout.canary_percent,
            source_revision,
            distillation_lease,
        };
        deployment.validate_for(scope, effort)?;
        Ok(deployment)
    }

    fn rollout(&self, scope: &str, effort: &str) -> Option<&'a PromptRolloutState> {
        self.model
            .rollouts
            .get(&prompt_rollout_key(scope, effort))
            .or_else(|| {
                ((scope == "global" || self.is_projection_for(scope))
                    && self
                        .model
                        .rollouts
                        .keys()
                        .all(|key| matches!(key.as_str(), "fast" | "auto" | "pro")))
                .then(|| self.model.rollouts.get(effort))
                .flatten()
            })
    }

    fn is_projection_for(&self, scope: &str) -> bool {
        self.model
            .genomes
            .iter()
            .all(|record| record.scope == scope)
            && self
                .model
                .failure_curricula
                .iter()
                .all(|record| record.scope == scope)
            && self
                .model
                .datasets
                .values()
                .all(|dataset| dataset.project_id == scope)
    }

    fn rollout_references_conflict(
        &self,
        scope: &str,
        effort: &str,
        rollout: &PromptRolloutState,
    ) -> bool {
        let mut ids = BTreeSet::from([rollout.stable_profile_id.as_str()]);
        ids.extend(rollout.canary_profile_id.as_deref());
        ids.into_iter().any(|id| {
            self.conflicts
                .contains(&(scope.to_string(), effort.to_string(), id.to_string()))
        })
    }

    fn resolve_stable(
        &self,
        scope: &str,
        effort: &str,
        rollout: &PromptRolloutState,
    ) -> Result<ResolvedProfile, String> {
        let seed = ConductorPromptGenome::seed_for_effort(effort);
        if rollout.stable_profile_id == seed.id {
            return Ok(ResolvedProfile {
                genome: seed,
                evolution_method: None,
            });
        }
        if let Some(snapshot) = &rollout.frozen_profile {
            snapshot.validate()?;
            if snapshot.effort != effort || snapshot.genome.id != rollout.stable_profile_id {
                return Err("frozen stable prompt profile does not match rollout".to_string());
            }
            return Ok(ResolvedProfile {
                genome: snapshot.genome.clone(),
                evolution_method: Some(snapshot.evolution_method),
            });
        }
        self.resolve_record(scope, effort, &rollout.stable_profile_id)
    }

    fn resolve_record(
        &self,
        scope: &str,
        effort: &str,
        profile_id: &str,
    ) -> Result<ResolvedProfile, String> {
        let key = (
            scope.to_string(),
            effort.to_string(),
            profile_id.to_string(),
        );
        if self.conflicts.contains(&key) {
            return Err(format!("conflicted prompt profile {profile_id}"));
        }
        let record = self
            .profiles
            .get(&key)
            .ok_or_else(|| format!("missing prompt profile {profile_id}"))?;
        Ok(ResolvedProfile {
            genome: record.genome.clone(),
            evolution_method: record.evolution_method,
        })
    }
}

pub(super) fn canonical_prompt_profile_snapshot_sha256(
    model: &PromptEvolutionReadModel,
    scope: &str,
    effort: &str,
) -> Result<String, String> {
    let scope = scope.trim();
    if !valid_scope(scope) || !matches!(effort, "auto" | "pro") {
        return Err("prompt profile canonical snapshot identity is invalid".to_string());
    }
    let catalog = PromptProfileDeploymentCatalog::new(model);
    if let Ok(mut deployment) = catalog.build(scope, effort, model.revision) {
        deployment.source_revision = 0;
        let snapshot = CanonicalPromptProfileSnapshot {
            schema: "cindx.prompt-profile-canonical-snapshot.v1",
            scope_sha256: scope_sha256(scope),
            effort: effort.to_string(),
            active_deployment: Some(serde_json::to_string(&deployment).map_err(|error| {
                format!("prompt profile deployment serialization failed: {error}")
            })?),
            genomes: Vec::new(),
            genome_identity_fingerprints: BTreeMap::new(),
            rollout: None,
        };
        let payload = serde_json::to_vec(&snapshot)
            .map_err(|error| format!("prompt profile snapshot serialization failed: {error}"))?;
        return Ok(sha256_hex(&payload));
    }
    let mut genomes = model
        .genomes
        .iter()
        .filter(|record| record.scope == scope && record.effort == effort)
        .map(|record| {
            serde_json::to_string(record)
                .map_err(|error| format!("prompt profile genome serialization failed: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    genomes.sort();
    let genome_identity_fingerprints = model
        .genome_identity_fingerprints
        .iter()
        .filter_map(|(identity, fingerprint)| {
            serde_json::from_str::<ProfileKey>(identity)
                .ok()
                .filter(|(identity_scope, identity_effort, _)| {
                    identity_scope == scope && identity_effort == effort
                })
                .map(|_| (identity.clone(), fingerprint.clone()))
        })
        .collect();
    let rollout = catalog
        .rollout(scope, effort)
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("prompt rollout serialization failed: {error}"))?;
    let snapshot = CanonicalPromptProfileSnapshot {
        schema: "cindx.prompt-profile-canonical-snapshot.v1",
        scope_sha256: scope_sha256(scope),
        effort: effort.to_string(),
        active_deployment: None,
        genomes,
        genome_identity_fingerprints,
        rollout,
    };
    let payload = serde_json::to_vec(&snapshot)
        .map_err(|error| format!("prompt profile snapshot serialization failed: {error}"))?;
    Ok(sha256_hex(&payload))
}

fn rollout_scope_effort(key: &str) -> Option<ScopeEffort> {
    let (scope, effort) = key
        .split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        .map(|(scope, effort)| (scope.to_string(), effort.to_string()))
        .unwrap_or_else(|| ("global".to_string(), key.to_string()));
    (valid_scope(&scope) && matches!(effort.as_str(), "auto" | "pro")).then_some((scope, effort))
}

fn distillation_lease(
    rollout: &PromptRolloutState,
    effort: &str,
    stable: &ResolvedProfile,
    canary: Option<&ResolvedProfile>,
) -> Result<Option<PromptProfileDistillationLease>, String> {
    let candidate_is_distillation = canary.is_some_and(|profile| {
        profile.evolution_method == Some(PromptEvolutionMethod::ProToAutoDistillation)
    });
    match (
        candidate_is_distillation,
        rollout.distillation_lease.as_ref(),
    ) {
        (true, Some(lease)) if effort == "auto" => {
            let projected = PromptProfileDistillationLease::try_from(lease)?;
            let canary = canary.expect("distillation candidate was checked");
            let stable_sha256 = prompt_genome_sha256(&stable.genome)?;
            let canary_sha256 = prompt_genome_sha256(&canary.genome)?;
            if projected.stable_profile_id != stable.genome.id
                || projected.stable_profile_sha256 != stable_sha256
                || projected.candidate_profile_id != canary.genome.id
                || projected.candidate_profile_sha256 != canary_sha256
            {
                return Err("prompt distillation lease drifted from its profiles".to_string());
            }
            Ok(Some(projected))
        }
        (true, _) => Err("distilled Auto canary requires a matched lease".to_string()),
        (false, Some(_)) => Err("prompt distillation lease has no distilled canary".to_string()),
        (false, None) => Ok(None),
    }
}

fn deployed(genome: ConductorPromptGenome) -> Result<DeployedPromptProfile, String> {
    validate_profile(&genome)?;
    Ok(DeployedPromptProfile {
        sha256: prompt_genome_sha256(&genome)?,
        genome,
    })
}
