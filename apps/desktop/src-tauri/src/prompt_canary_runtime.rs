use crate::{
    runtime_constants::PROMPT_ROLLOUT_MAX_QUARANTINED_PROFILES,
    view_models::{PromptEvolutionReadModel, PromptRolloutState},
};
use orchestrator::{PromptEvaluationMode, PromptEvolutionObservation};
use std::collections::BTreeSet;

pub(crate) fn rollback_prompt_canary(
    rollout: &mut PromptRolloutState,
    candidate_id: &str,
    quarantine_candidate: bool,
    reason: String,
) {
    if quarantine_candidate
        && !rollout
            .quarantined_profile_ids
            .iter()
            .any(|profile_id| profile_id == candidate_id)
        && rollout.quarantined_profile_ids.len() < PROMPT_ROLLOUT_MAX_QUARANTINED_PROFILES
    {
        rollout
            .quarantined_profile_ids
            .push(candidate_id.to_string());
    }
    rollout.canary_profile_id = None;
    rollout.canary_percent = 0;
    rollout.distillation_lease = None;
    rollout.rollback_count = rollout.rollback_count.saturating_add(1);
    rollout.status = "rolled_back".to_string();
    rollout.last_reason = Some(reason);
}

pub(crate) fn prompt_candidate_blocked_by_distillation_quarantine(
    rollout: &PromptRolloutState,
    candidate_id: &str,
    candidate_is_distillation: bool,
) -> bool {
    candidate_is_distillation
        && (rollout
            .quarantined_profile_ids
            .iter()
            .any(|profile_id| profile_id == candidate_id)
            || (rollout.canary_profile_id.is_none()
                && rollout.quarantined_profile_ids.len()
                    >= PROMPT_ROLLOUT_MAX_QUARANTINED_PROFILES))
}

pub(crate) fn collect_prompt_live_observations<'a>(
    model: &'a PromptEvolutionReadModel,
    effort: &str,
    profile_id: &str,
) -> Vec<&'a PromptEvolutionObservation> {
    let mut seen = BTreeSet::new();
    model
        .observations
        .iter()
        .filter(|(observed_effort, observation)| {
            observed_effort == effort
                && observation.profile_id == profile_id
                && observation.mode == PromptEvaluationMode::Live
        })
        .map(|(_, observation)| observation)
        .filter(|observation| seen.insert(observation.evidence_identity()))
        .collect()
}
