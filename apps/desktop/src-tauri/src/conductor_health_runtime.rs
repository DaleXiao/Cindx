use crate::collaboration_stage_runtime::CollaborationStageError;
use agent_core::Metadata;
use orchestrator::{sha256_hex, AgentRunDecision};
use std::collections::{BTreeMap, VecDeque};
#[cfg(test)]
use std::mem::size_of;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(crate) const CONDUCTOR_HEALTH_MAX_KEYS: usize = 32;
pub(crate) const CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY: usize = 32;
const CONDUCTOR_HEALTH_HALF_LIFE: Duration = Duration::from_secs(30 * 60);
const CONDUCTOR_HEALTH_ENTRY_TTL: Duration = Duration::from_secs(4 * 60 * 60);
const CONDUCTOR_HEALTH_MIN_RAW_SAMPLES: usize = 4;
const CONDUCTOR_HEALTH_MIN_DECAYED_MASS: f64 = 2.0;
const CONDUCTOR_HEALTH_MIN_KISH_SAMPLES: f64 = 4.0;
const CONDUCTOR_HEALTH_MAX_AUDIT_ATTEMPTS: usize = 6;
const WILSON_Z_95: f64 = 1.959_963_984_540_054;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConductorHealthOutcome {
    ValidDecision,
    RetryableFailure,
    RejectedDecision,
    Censored,
}

impl ConductorHealthOutcome {
    fn is_success(self) -> bool {
        self == Self::ValidDecision
    }

    fn label(self) -> &'static str {
        match self {
            Self::ValidDecision => "valid_decision",
            Self::RetryableFailure => "retryable_failure",
            Self::RejectedDecision => "rejected_decision",
            Self::Censored => "censored",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ConductorHealthKey {
    provider_scope: String,
    model: String,
}

#[derive(Debug, Clone, Copy)]
struct ConductorHealthObservation {
    outcome: ConductorHealthOutcome,
    model_calls: u8,
    observed_at: Instant,
}

#[derive(Debug)]
struct ConductorHealthEntry {
    observations: VecDeque<ConductorHealthObservation>,
    last_observed_at: Instant,
}

#[derive(Debug, Clone, Copy)]
struct ConductorHealthEstimate {
    raw_samples: usize,
    decayed_mass: f64,
    kish_samples: f64,
    lower_bound: f64,
    upper_bound: f64,
    single_call_only: bool,
}

impl ConductorHealthEstimate {
    fn eligible(self) -> bool {
        self.raw_samples >= CONDUCTOR_HEALTH_MIN_RAW_SAMPLES
            && self.decayed_mass >= CONDUCTOR_HEALTH_MIN_DECAYED_MASS
            && self.kish_samples >= CONDUCTOR_HEALTH_MIN_KISH_SAMPLES
    }

    fn strictly_dominates(self, incumbent: Self) -> bool {
        self.single_call_only
            && self.eligible()
            && incumbent.eligible()
            && self.lower_bound > incumbent.upper_bound
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConductorHealthRouting {
    pub(crate) models: Vec<String>,
    pub(crate) source: &'static str,
    pub(crate) candidate_models: Vec<String>,
    pub(crate) retained_observations: usize,
    pub(crate) hedge_enabled: bool,
}

impl ConductorHealthRouting {
    fn configured(models: &[String], retained_observations: usize) -> Self {
        Self {
            models: models.to_vec(),
            source: "configured_order",
            candidate_models: Vec::new(),
            retained_observations,
            hedge_enabled: false,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ConductorHealthLedger {
    entries: BTreeMap<ConductorHealthKey, ConductorHealthEntry>,
    generation: u64,
}

pub(crate) fn conductor_provider_scope(base_url: &str) -> String {
    let normalized = base_url.trim().trim_end_matches('/');
    sha256_hex(normalized.as_bytes())
}

fn conductor_model_scope(model: &str) -> String {
    sha256_hex(model.trim().as_bytes())
}

fn conductor_ledger_scope(provider_scope: &str) -> String {
    sha256_hex(provider_scope.trim().as_bytes())
}

pub(crate) fn record_conductor_fast_bypass(run_context: &mut Metadata) {
    run_context.insert(
        "conductor_health_routing".to_string(),
        "fast_bypass".to_string(),
    );
    run_context.insert("conductor_hedge_enabled".to_string(), "false".to_string());
}

pub(crate) fn conductor_health_outcome(
    result: &Result<AgentRunDecision, CollaborationStageError>,
) -> ConductorHealthOutcome {
    match result {
        Ok(_) => ConductorHealthOutcome::ValidDecision,
        Err(CollaborationStageError::AttemptDeadline) => ConductorHealthOutcome::RetryableFailure,
        Err(CollaborationStageError::ModelFailure(failure))
            if failure.class == agent_runtime::AgentFailureClass::ProviderTransient
                && failure.retryable =>
        {
            ConductorHealthOutcome::RetryableFailure
        }
        Err(CollaborationStageError::ModelFailure(failure))
            if failure.class == agent_runtime::AgentFailureClass::ModelOutput =>
        {
            ConductorHealthOutcome::RejectedDecision
        }
        Err(CollaborationStageError::DecisionRejected(_)) => {
            ConductorHealthOutcome::RejectedDecision
        }
        Err(_) => ConductorHealthOutcome::Censored,
    }
}

pub(crate) fn route_conductor_models(
    ledger: &Mutex<ConductorHealthLedger>,
    base_url: &str,
    configured_models: &[String],
    run_context: &mut Metadata,
) -> (String, u64, Vec<String>) {
    let provider_scope = conductor_provider_scope(base_url);
    let (routing, generation) = {
        let mut ledger = ledger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let routing = ledger.route(&provider_scope, configured_models);
        (routing, ledger.generation)
    };
    run_context.insert(
        "conductor_configured_models".to_string(),
        configured_models.join(","),
    );
    run_context.insert(
        "conductor_health_routing".to_string(),
        routing.source.to_string(),
    );
    run_context.insert(
        "conductor_health_candidate_models".to_string(),
        routing.candidate_models.join(","),
    );
    run_context.insert(
        "conductor_health_observations".to_string(),
        routing.retained_observations.to_string(),
    );
    run_context.insert(
        "conductor_hedge_enabled".to_string(),
        routing.hedge_enabled.to_string(),
    );
    (provider_scope, generation, routing.models)
}

pub(crate) fn record_conductor_attempt(
    ledger: &Mutex<ConductorHealthLedger>,
    provider_scope: &str,
    generation: u64,
    model: &str,
    model_calls: usize,
    result: &Result<AgentRunDecision, CollaborationStageError>,
    run_context: &mut Metadata,
) {
    let outcome = conductor_health_outcome(result);
    ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record_if_current(provider_scope, generation, model, outcome, model_calls);
    let mut attempts = run_context
        .get("conductor_health_attempts")
        .filter(|encoded| encoded.len() <= 2_048)
        .and_then(|encoded| serde_json::from_str::<Vec<(String, String, u8)>>(encoded).ok())
        .unwrap_or_default();
    if attempts.len() < CONDUCTOR_HEALTH_MAX_AUDIT_ATTEMPTS {
        attempts.push((
            model.chars().take(160).collect(),
            outcome.label().to_string(),
            u8::try_from(model_calls.max(1)).unwrap_or(u8::MAX),
        ));
        if let Ok(encoded) = serde_json::to_string(&attempts) {
            run_context.insert("conductor_health_attempts".to_string(), encoded);
        }
    }
}

impl ConductorHealthLedger {
    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
    }

    fn record_if_current(
        &mut self,
        provider_scope: &str,
        generation: u64,
        model: &str,
        outcome: ConductorHealthOutcome,
        model_calls: usize,
    ) {
        if generation == self.generation {
            self.record_at_with_calls(provider_scope, model, outcome, model_calls, Instant::now());
        }
    }

    #[cfg(test)]
    pub(crate) fn record(
        &mut self,
        provider_scope: &str,
        model: &str,
        outcome: ConductorHealthOutcome,
    ) {
        self.record_at(provider_scope, model, outcome, Instant::now());
    }

    #[cfg(test)]
    fn record_at(
        &mut self,
        provider_scope: &str,
        model: &str,
        outcome: ConductorHealthOutcome,
        now: Instant,
    ) {
        self.record_at_with_calls(provider_scope, model, outcome, 1, now);
    }

    fn record_at_with_calls(
        &mut self,
        provider_scope: &str,
        model: &str,
        outcome: ConductorHealthOutcome,
        model_calls: usize,
        now: Instant,
    ) {
        if outcome == ConductorHealthOutcome::Censored
            || provider_scope.trim().is_empty()
            || model.trim().is_empty()
        {
            return;
        }
        self.prune_expired(now);
        let key = ConductorHealthKey {
            provider_scope: conductor_ledger_scope(provider_scope),
            model: conductor_model_scope(model),
        };
        if !self.entries.contains_key(&key) && self.entries.len() >= CONDUCTOR_HEALTH_MAX_KEYS {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_observed_at)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let entry = self
            .entries
            .entry(key)
            .or_insert_with(|| ConductorHealthEntry {
                observations: VecDeque::with_capacity(CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY),
                last_observed_at: now,
            });
        if entry.observations.len() >= CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY {
            entry.observations.pop_front();
        }
        entry.observations.push_back(ConductorHealthObservation {
            outcome,
            model_calls: u8::try_from(model_calls.max(1)).unwrap_or(u8::MAX),
            observed_at: now,
        });
        entry.last_observed_at = now;
    }

    pub(crate) fn route(
        &mut self,
        provider_scope: &str,
        configured_models: &[String],
    ) -> ConductorHealthRouting {
        self.route_at(provider_scope, configured_models, Instant::now())
    }

    fn route_at(
        &mut self,
        provider_scope: &str,
        configured_models: &[String],
        now: Instant,
    ) -> ConductorHealthRouting {
        self.prune_expired(now);
        let retained_observations = self.retained_observations();
        if retained_observations == 0
            || configured_models.len() < 4
            || provider_scope.trim().is_empty()
        {
            return ConductorHealthRouting::configured(configured_models, retained_observations);
        }

        let provider_scope = conductor_ledger_scope(provider_scope);
        let terminal_index = configured_models.len() - 1;
        let mut scored_models = configured_models
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let estimate = (index > 0 && index < terminal_index)
                    .then(|| conductor_model_scope(model))
                    .and_then(|model_scope| {
                        self.estimate_scoped(&provider_scope, &model_scope, now)
                    });
                (model.clone(), estimate)
            })
            .collect::<Vec<_>>();
        for candidate_index in 2..terminal_index {
            let mut position = candidate_index;
            while position > 1 {
                let Some(candidate) = scored_models[position].1 else {
                    break;
                };
                let Some(incumbent) = scored_models[position - 1].1 else {
                    break;
                };
                if !candidate.strictly_dominates(incumbent) {
                    break;
                }
                scored_models.swap(position - 1, position);
                position -= 1;
            }
        }
        let confidence_order = scored_models
            .into_iter()
            .map(|(model, _)| model)
            .collect::<Vec<_>>();

        if confidence_order == configured_models {
            return ConductorHealthRouting::configured(configured_models, retained_observations);
        }
        let candidate_models = confidence_order
            .iter()
            .enumerate()
            .filter_map(|(index, model)| {
                configured_models
                    .iter()
                    .position(|configured| configured == model)
                    .filter(|configured_index| index < *configured_index)
                    .map(|_| model.clone())
            })
            .collect();
        ConductorHealthRouting {
            models: configured_models.to_vec(),
            source: "quality_evidence_required",
            candidate_models,
            retained_observations,
            hedge_enabled: false,
        }
    }

    pub(crate) fn retained_observations(&self) -> usize {
        self.entries
            .values()
            .map(|entry| entry.observations.len())
            .sum()
    }

    #[cfg(test)]
    pub(crate) fn estimated_heap_bytes(&self) -> usize {
        size_of::<Self>()
            + self
                .entries
                .iter()
                .map(|(key, entry)| {
                    64 + key.provider_scope.capacity()
                        + key.model.capacity()
                        + size_of::<ConductorHealthEntry>()
                        + entry.observations.capacity() * size_of::<ConductorHealthObservation>()
                })
                .sum::<usize>()
    }

    fn prune_expired(&mut self, now: Instant) {
        self.entries.retain(|_, entry| {
            now.saturating_duration_since(entry.last_observed_at) <= CONDUCTOR_HEALTH_ENTRY_TTL
        });
    }

    #[cfg(test)]
    fn estimate(
        &self,
        provider_scope: &str,
        model: &str,
        now: Instant,
    ) -> Option<ConductorHealthEstimate> {
        self.estimate_scoped(
            &conductor_ledger_scope(provider_scope),
            &conductor_model_scope(model),
            now,
        )
    }

    fn estimate_scoped(
        &self,
        provider_scope: &str,
        model_scope: &str,
        now: Instant,
    ) -> Option<ConductorHealthEstimate> {
        let key = ConductorHealthKey {
            provider_scope: provider_scope.to_string(),
            model: model_scope.to_string(),
        };
        let entry = self.entries.get(&key)?;
        let half_life_seconds = CONDUCTOR_HEALTH_HALF_LIFE.as_secs_f64();
        let mut sum_weights = 0.0;
        let mut sum_squared_weights = 0.0;
        let mut weighted_successes = 0.0;
        let mut single_call_only = true;
        for observation in &entry.observations {
            let age_seconds = now
                .saturating_duration_since(observation.observed_at)
                .as_secs();
            let weight = if age_seconds == 0 {
                1.0
            } else {
                2.0_f64.powf(-(age_seconds as f64) / half_life_seconds)
            };
            sum_weights += weight;
            sum_squared_weights += weight * weight;
            if observation.outcome.is_success() {
                weighted_successes += weight;
            }
            single_call_only &= observation.model_calls == 1;
        }
        if sum_weights <= f64::EPSILON || sum_squared_weights <= f64::EPSILON {
            return None;
        }
        let success_rate = (weighted_successes / sum_weights).clamp(0.0, 1.0);
        let kish_samples = sum_weights * sum_weights / sum_squared_weights;
        let wilson_samples = kish_samples.max(f64::EPSILON);
        let (lower_bound, upper_bound) = wilson_interval(success_rate, wilson_samples);
        Some(ConductorHealthEstimate {
            raw_samples: entry.observations.len(),
            decayed_mass: sum_weights,
            kish_samples,
            lower_bound,
            upper_bound,
            single_call_only,
        })
    }
}

fn wilson_interval(success_rate: f64, samples: f64) -> (f64, f64) {
    let z_squared = WILSON_Z_95 * WILSON_Z_95;
    let denominator = 1.0 + z_squared / samples;
    let center = (success_rate + z_squared / (2.0 * samples)) / denominator;
    let margin = WILSON_Z_95
        * ((success_rate * (1.0 - success_rate) + z_squared / (4.0 * samples)) / samples).sqrt()
        / denominator;
    (
        (center - margin).clamp(0.0, 1.0),
        (center + margin).clamp(0.0, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<String> {
        ["primary", "alternate", "reserve", "terminal"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn record_outcomes(
        ledger: &mut ConductorHealthLedger,
        scope: &str,
        model: &str,
        outcome: ConductorHealthOutcome,
        count: usize,
        now: Instant,
    ) {
        for _ in 0..count {
            ledger.record_at(scope, model, outcome, now);
        }
    }

    #[test]
    fn goal3_conductor_health_cold_weak_and_censored_evidence_preserve_order() {
        let now = Instant::now();
        let configured = models();
        let mut ledger = ConductorHealthLedger::default();
        assert_eq!(
            ledger.route_at("scope", &configured, now).models,
            configured
        );
        record_outcomes(
            &mut ledger,
            "scope",
            "alternate",
            ConductorHealthOutcome::RetryableFailure,
            3,
            now,
        );
        record_outcomes(
            &mut ledger,
            "scope",
            "reserve",
            ConductorHealthOutcome::ValidDecision,
            3,
            now,
        );
        ledger.record_at("scope", "reserve", ConductorHealthOutcome::Censored, now);
        let route = ledger.route_at("scope", &configured, now);
        assert_eq!(route.models, configured);
        assert_eq!(route.source, "configured_order");
        assert!(!route.hedge_enabled);
        assert_eq!(ledger.retained_observations(), 6);
    }

    #[test]
    fn goal3_conductor_health_identifies_reliability_candidate_but_preserves_order() {
        let now = Instant::now();
        let configured = models();
        let mut ledger = ConductorHealthLedger::default();
        record_outcomes(
            &mut ledger,
            "scope",
            "alternate",
            ConductorHealthOutcome::RetryableFailure,
            4,
            now,
        );
        record_outcomes(
            &mut ledger,
            "scope",
            "reserve",
            ConductorHealthOutcome::ValidDecision,
            4,
            now,
        );
        let route = ledger.route_at("scope", &configured, now);
        assert_eq!(route.models, configured);
        assert_eq!(route.source, "quality_evidence_required");
        assert_eq!(route.candidate_models, vec!["reserve"]);
        assert!(!route.hedge_enabled);
    }

    #[test]
    fn goal3_conductor_health_never_moves_configured_primary_or_terminal_model() {
        let now = Instant::now();
        let configured = models();
        let mut ledger = ConductorHealthLedger::default();
        for (model, outcome) in [
            ("primary", ConductorHealthOutcome::RetryableFailure),
            ("alternate", ConductorHealthOutcome::RetryableFailure),
            ("reserve", ConductorHealthOutcome::ValidDecision),
            ("terminal", ConductorHealthOutcome::ValidDecision),
        ] {
            record_outcomes(&mut ledger, "scope", model, outcome, 8, now);
        }

        let route = ledger.route_at("scope", &configured, now);
        assert_eq!(route.models.first(), configured.first());
        assert_eq!(route.models.last(), configured.last());
        assert_eq!(route.models, configured);
        assert_eq!(route.candidate_models, vec!["reserve"]);
    }

    #[test]
    fn goal3_conductor_health_never_promotes_a_repair_dependent_backup() {
        let now = Instant::now();
        let configured = models();
        let mut ledger = ConductorHealthLedger::default();
        record_outcomes(
            &mut ledger,
            "scope",
            "alternate",
            ConductorHealthOutcome::RetryableFailure,
            5,
            now,
        );
        record_outcomes(
            &mut ledger,
            "scope",
            "reserve",
            ConductorHealthOutcome::ValidDecision,
            4,
            now,
        );
        ledger.record_at_with_calls(
            "scope",
            "reserve",
            ConductorHealthOutcome::ValidDecision,
            2,
            now,
        );

        assert_eq!(
            ledger.route_at("scope", &configured, now).models,
            configured
        );
    }

    #[test]
    fn goal3_conductor_health_requires_four_effective_samples() {
        let now = Instant::now();
        let old = now - Duration::from_secs(423);
        let configured = models();
        let mut ledger = ConductorHealthLedger::default();
        for (model, outcome) in [
            ("alternate", ConductorHealthOutcome::RetryableFailure),
            ("reserve", ConductorHealthOutcome::ValidDecision),
        ] {
            record_outcomes(&mut ledger, "scope", model, outcome, 1, old);
            record_outcomes(&mut ledger, "scope", model, outcome, 3, now);
        }

        let alternate = ledger
            .estimate("scope", "alternate", now)
            .expect("alternate estimate");
        assert!(alternate.kish_samples < 4.0);
        assert_eq!(
            ledger.route_at("scope", &configured, now).models,
            configured
        );
    }

    #[test]
    fn goal3_conductor_health_overlapping_confidence_intervals_preserve_order() {
        let now = Instant::now();
        let configured = models();
        let mut ledger = ConductorHealthLedger::default();
        record_outcomes(
            &mut ledger,
            "scope",
            "alternate",
            ConductorHealthOutcome::ValidDecision,
            3,
            now,
        );
        record_outcomes(
            &mut ledger,
            "scope",
            "alternate",
            ConductorHealthOutcome::RetryableFailure,
            1,
            now,
        );
        record_outcomes(
            &mut ledger,
            "scope",
            "reserve",
            ConductorHealthOutcome::ValidDecision,
            4,
            now,
        );

        assert_eq!(
            ledger.route_at("scope", &configured, now).models,
            configured
        );
    }

    #[test]
    fn goal3_conductor_health_provider_scope_is_normalized_and_opaque() {
        let scoped = conductor_provider_scope("  https://gateway.example/v1///  ");
        assert_eq!(
            scoped,
            conductor_provider_scope("https://gateway.example/v1")
        );
        assert_eq!(scoped.len(), 64);
        assert!(!scoped.contains("gateway.example"));
    }

    #[test]
    fn goal3_conductor_health_decay_and_provider_scope_restore_configured_order() {
        let now = Instant::now();
        let configured = models();
        let mut ledger = ConductorHealthLedger::default();
        record_outcomes(
            &mut ledger,
            "scope-a",
            "alternate",
            ConductorHealthOutcome::RejectedDecision,
            4,
            now,
        );
        record_outcomes(
            &mut ledger,
            "scope-a",
            "reserve",
            ConductorHealthOutcome::ValidDecision,
            4,
            now,
        );
        assert_eq!(
            ledger.route_at("scope-b", &configured, now).models,
            configured
        );
        let eligible = ledger.route_at("scope-a", &configured, now + Duration::from_secs(2 * 60));
        assert_eq!(eligible.models, configured);
        assert_eq!(eligible.source, "quality_evidence_required");
        assert_eq!(eligible.candidate_models, vec!["reserve"]);
        assert_eq!(
            ledger
                .route_at(
                    "scope-a",
                    &configured,
                    now + CONDUCTOR_HEALTH_HALF_LIFE + Duration::from_secs(1),
                )
                .models,
            configured
        );
    }

    #[test]
    fn goal3_conductor_health_rejects_observations_from_an_invalidated_generation() {
        let mut ledger = ConductorHealthLedger::default();
        let stale_generation = ledger.generation;
        ledger.clear();
        ledger.record_if_current(
            "scope",
            stale_generation,
            "alternate",
            ConductorHealthOutcome::ValidDecision,
            1,
        );
        assert_eq!(ledger.retained_observations(), 0);

        let current_generation = ledger.generation;
        ledger.record_if_current(
            "scope",
            current_generation,
            "alternate",
            ConductorHealthOutcome::ValidDecision,
            1,
        );
        assert_eq!(ledger.retained_observations(), 1);
    }

    #[test]
    fn goal3_conductor_health_storage_is_strictly_bounded() {
        let now = Instant::now();
        let mut ledger = ConductorHealthLedger::default();
        for model_index in 0..(CONDUCTOR_HEALTH_MAX_KEYS + 8) {
            for _ in 0..(CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY + 8) {
                ledger.record_at(
                    "scope",
                    &format!("model-{model_index}"),
                    ConductorHealthOutcome::ValidDecision,
                    now + Duration::from_millis(model_index as u64),
                );
            }
        }
        assert_eq!(ledger.entries.len(), CONDUCTOR_HEALTH_MAX_KEYS);
        assert_eq!(
            ledger.retained_observations(),
            CONDUCTOR_HEALTH_MAX_KEYS * CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY
        );
        assert!(ledger.estimated_heap_bytes() <= 65_536);
    }

    #[test]
    fn goal3_conductor_health_hashes_unbounded_external_key_material() {
        let mut ledger = ConductorHealthLedger::default();
        ledger.record(
            &"endpoint".repeat(128 * 1024),
            &"model".repeat(256 * 1024),
            ConductorHealthOutcome::ValidDecision,
        );
        let key = ledger.entries.keys().next().expect("health key");
        assert_eq!(key.provider_scope.len(), 64);
        assert_eq!(key.model.len(), 64);
        assert!(ledger.estimated_heap_bytes() <= 65_536);
    }

    #[test]
    fn goal3_conductor_health_persists_bounded_attempt_audit_metadata() {
        let ledger = Mutex::new(ConductorHealthLedger::default());
        let mut run_context = Metadata::new();
        let result = Err(CollaborationStageError::DecisionRejected(
            "invalid decision".to_string(),
        ));
        record_conductor_attempt(
            &ledger,
            "scope",
            0,
            "alternate",
            2,
            &result,
            &mut run_context,
        );

        let attempts = serde_json::from_str::<Vec<(String, String, u8)>>(
            run_context
                .get("conductor_health_attempts")
                .expect("attempt audit metadata"),
        )
        .expect("valid attempt audit JSON");
        assert_eq!(
            attempts,
            vec![("alternate".to_string(), "rejected_decision".to_string(), 2)]
        );
        assert_eq!(
            ledger
                .lock()
                .expect("health ledger")
                .retained_observations(),
            1
        );
    }

    #[test]
    #[ignore = "deterministic performance diagnostic"]
    fn conductor_health_scaling_diagnostic() {
        fn percentile(samples: &mut [u128], percentile: usize) -> u128 {
            samples.sort_unstable();
            let index = samples.len().saturating_sub(1).saturating_mul(percentile) / 100;
            samples.get(index).copied().unwrap_or_default()
        }

        let now = Instant::now();
        let configured = (0..6)
            .map(|index| format!("model-{index}"))
            .collect::<Vec<_>>();
        let mut baseline_samples = Vec::new();
        for iteration in 0..121 {
            let started = Instant::now();
            let route = std::hint::black_box(configured.clone());
            assert_eq!(route, configured);
            if iteration >= 20 {
                baseline_samples.push(started.elapsed().as_nanos());
            }
        }
        let mut cold = ConductorHealthLedger::default();
        let mut cold_samples = Vec::new();
        for iteration in 0..121 {
            let started = Instant::now();
            let route = cold.route_at("scope", &configured, now);
            assert_eq!(route.models, configured);
            if iteration >= 20 {
                cold_samples.push(started.elapsed().as_nanos());
            }
        }

        let mut warm = ConductorHealthLedger::default();
        for key in 0..CONDUCTOR_HEALTH_MAX_KEYS {
            for sample in 0..CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY {
                warm.record_at(
                    &format!("scope-{}", key / 6),
                    &format!("model-{}", key % 6),
                    if (key + sample) % 3 == 0 {
                        ConductorHealthOutcome::RetryableFailure
                    } else {
                        ConductorHealthOutcome::ValidDecision
                    },
                    now,
                );
            }
        }
        let mut warm_samples = Vec::new();
        for iteration in 0..121 {
            let started = Instant::now();
            let _ = warm.route_at("scope-0", &configured, now);
            if iteration >= 20 {
                warm_samples.push(started.elapsed().as_nanos());
            }
        }
        let baseline_order_p95_nanos = percentile(&mut baseline_samples, 95);
        let cold_path_p95_nanos = percentile(&mut cold_samples, 95);
        let warm_evidence_p95_nanos = percentile(&mut warm_samples, 95);
        let allowed_nanos = baseline_order_p95_nanos
            .saturating_mul(105)
            .saturating_div(100)
            .max(baseline_order_p95_nanos.saturating_add(50_000));
        let retained_observations = warm.retained_observations();
        let estimated_heap_bytes = warm.estimated_heap_bytes();
        assert!(cold_path_p95_nanos <= allowed_nanos);
        assert!(warm_evidence_p95_nanos <= allowed_nanos);
        assert!(
            retained_observations
                <= CONDUCTOR_HEALTH_MAX_KEYS * CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY
        );
        assert!(estimated_heap_bytes <= 65_536);
        let baseline_order_p95_micros = baseline_order_p95_nanos as f64 / 1_000.0;
        let cold_path_p95_micros = cold_path_p95_nanos as f64 / 1_000.0;
        let warm_evidence_p95_micros = warm_evidence_p95_nanos as f64 / 1_000.0;
        println!(
            "{{\"schema\":\"cindx.conductor-health-diagnostic.v1\",\"models\":6,\"max_health_keys\":{},\"max_observations_per_key\":{},\"warmup_rounds\":20,\"sample_count\":101,\"hedge\":false,\"baseline_order_p95_micros\":{baseline_order_p95_micros},\"cold_path_p95_micros\":{cold_path_p95_micros},\"warm_evidence_p95_micros\":{warm_evidence_p95_micros},\"retained_observations\":{retained_observations},\"estimated_heap_bytes\":{estimated_heap_bytes}}}",
            CONDUCTOR_HEALTH_MAX_KEYS,
            CONDUCTOR_HEALTH_MAX_OBSERVATIONS_PER_KEY,
        );
    }
}
