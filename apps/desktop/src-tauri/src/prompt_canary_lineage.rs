use crate::{
    prompt_profile_serving::PromptProfileDeploymentLineage,
    view_models::PromptDistillationCanaryLeaseV1,
};
use orchestrator::{sha256_hex, PromptEvolutionObservation};

pub(crate) fn prompt_live_observation_matches_lineage(
    observation: &PromptEvolutionObservation,
    lineage: &PromptProfileDeploymentLineage,
) -> bool {
    observation
        .provenance
        .live_assignment
        .as_ref()
        .is_some_and(|assignment| {
            assignment.scope_sha256 == lineage.scope_sha256
                && assignment.source_revision == lineage.source_revision
                && assignment.deployment_generation == lineage.deployment_generation
        })
}

pub(crate) fn fresh_prompt_live_observations_for_lineage<'a>(
    observations: &[&'a PromptEvolutionObservation],
    checkpoint: usize,
    lineage: &PromptProfileDeploymentLineage,
) -> Vec<&'a PromptEvolutionObservation> {
    observations[checkpoint..]
        .iter()
        .copied()
        .filter(|observation| prompt_live_observation_matches_lineage(observation, lineage))
        .collect()
}

pub(crate) fn prompt_live_observation_matches_distillation_assignment(
    observation: &PromptEvolutionObservation,
    lineage: &PromptProfileDeploymentLineage,
    assignment_source: &str,
    profile_id: &str,
    profile_sha256: &str,
    lease_sha256: &str,
) -> bool {
    prompt_live_observation_matches_lineage(observation, lineage)
        && observation
            .provenance
            .live_assignment
            .as_ref()
            .is_some_and(|assignment| {
                assignment.assignment_source == assignment_source
                    && assignment.profile_id == profile_id
                    && assignment.profile_sha256 == profile_sha256
                    && assignment.distillation_lease_sha256.as_deref() == Some(lease_sha256)
            })
}

pub(crate) fn prompt_distillation_lease_sha256(
    lease: &PromptDistillationCanaryLeaseV1,
) -> Result<String, String> {
    serde_json::to_vec(lease)
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("Auto distillation canary lease serialization failed: {error}"))
}
