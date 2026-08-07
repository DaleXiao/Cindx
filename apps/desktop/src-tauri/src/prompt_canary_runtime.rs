use crate::{
    runtime_constants::PROMPT_ROLLOUT_MAX_QUARANTINED_PROFILES,
    view_models::{PromptEvolutionReadModel, PromptRolloutState},
};
use orchestrator::{ConductorPromptGenome, PromptEvaluationMode, PromptEvolutionObservation};
use std::collections::BTreeSet;

pub(crate) fn default_prompt_rollout(effort: &str) -> PromptRolloutState {
    PromptRolloutState {
        stable_profile_id: ConductorPromptGenome::seed_for_effort(effort).id,
        canary_profile_id: None,
        canary_percent: 0,
        evidence_checkpoint: 0,
        live_checkpoint: 0,
        stable_live_checkpoint: 0,
        quarantined_profile_ids: Vec::new(),
        distillation_lease: None,
        rollback_count: 0,
        status: "stable".to_string(),
        last_reason: None,
        promotion_confidence: None,
        frozen_profile: None,
    }
}

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
                && observation.is_trusted_live_assignment()
        })
        .map(|(_, observation)| observation)
        .filter(|observation| seen.insert(observation.evidence_identity()))
        .collect()
}

pub(crate) fn prompt_canary_degraded(
    model: &PromptEvolutionReadModel,
    effort: &str,
    stable_profile_id: &str,
    canary_profile_id: &str,
) -> Option<String> {
    let canary = collect_prompt_live_observations(model, effort, canary_profile_id);
    let stable = collect_prompt_live_observations(model, effort, stable_profile_id);
    prompt_canary_observations_degraded(&canary, &stable)
}

pub(crate) fn prompt_canary_observations_degraded(
    canary: &[&PromptEvolutionObservation],
    stable: &[&PromptEvolutionObservation],
) -> Option<String> {
    if canary
        .iter()
        .rev()
        .take(4)
        .any(|observation| observation.safety_violations > 0 || !observation.format_valid)
    {
        return Some("canary_safety_regression".to_string());
    }
    let recent_canary = canary.iter().rev().take(4).copied().collect::<Vec<_>>();
    if recent_canary.len() >= 2 {
        let success_rate = recent_canary
            .iter()
            .filter(|observation| observation.succeeded)
            .count() as f64
            / recent_canary.len() as f64;
        if success_rate < 0.5 {
            return Some("canary_success_regression".to_string());
        }
    }
    let recent_stable = stable.iter().rev().take(4).copied().collect::<Vec<_>>();
    if recent_canary.len() >= 2 && recent_stable.len() >= 2 {
        let average = |entries: &[&PromptEvolutionObservation]| {
            entries.iter().map(|entry| entry.reward()).sum::<f64>() / entries.len() as f64
        };
        if average(&recent_canary) + 0.08 < average(&recent_stable) {
            return Some("canary_reward_regression".to_string());
        }
    }
    None
}

pub(crate) fn next_prompt_canary_stage(current: u8) -> u8 {
    match current {
        0..=9 => 10,
        10..=24 => 25,
        _ => 50,
    }
}
