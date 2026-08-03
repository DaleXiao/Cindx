use crate::run_budget::{RunBudget, RunStageClass};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_RESOURCE_LEDGER_MODELS: usize = 32;
pub const MAX_PENDING_RESOURCE_ATTEMPTS: usize = 64;
pub const MAX_RESOURCE_MODEL_KEY_BYTES: usize = 128;
pub const MAX_PERSISTED_RESOURCE_SNAPSHOT_BYTES: usize = 65_536;

const OTHER_MODELS_KEY: &str = "__other_models__";
const UNKNOWN_MODEL_KEY: &str = "__unknown_model__";
static NEXT_LEDGER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelUsageSource {
    Provider,
    ProviderPartial,
    Estimated,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsageSourceCounts {
    pub provider: u64,
    pub provider_partial: u64,
    pub estimated: u64,
    pub unknown: u64,
}

impl ModelUsageSourceCounts {
    pub fn total(self) -> u64 {
        self.provider
            .saturating_add(self.provider_partial)
            .saturating_add(self.estimated)
            .saturating_add(self.unknown)
    }

    pub fn least_complete(self) -> Option<ModelUsageSource> {
        if self.unknown > 0 {
            Some(ModelUsageSource::Unknown)
        } else if self.estimated > 0 {
            Some(ModelUsageSource::Estimated)
        } else if self.provider_partial > 0 {
            Some(ModelUsageSource::ProviderPartial)
        } else if self.provider > 0 {
            Some(ModelUsageSource::Provider)
        } else {
            None
        }
    }

    fn record(&mut self, source: ModelUsageSource) {
        let counter = match source {
            ModelUsageSource::Provider => &mut self.provider,
            ModelUsageSource::ProviderPartial => &mut self.provider_partial,
            ModelUsageSource::Estimated => &mut self.estimated,
            ModelUsageSource::Unknown => &mut self.unknown,
        };
        *counter = counter.saturating_add(1);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelResourceUsage {
    pub physical_attempts: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub reserved_tokens: u64,
    pub usage_sources: ModelUsageSourceCounts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResourceUsage {
    pub physical_attempts: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub reserved_tokens: u64,
    pub usage_sources: ModelUsageSourceCounts,
    pub models: BTreeMap<String, ModelResourceUsage>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAttemptUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub source: ModelUsageSource,
}

impl ModelAttemptUsage {
    pub fn new(
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: u64,
        source: ModelUsageSource,
    ) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            source,
        }
    }

    fn normalized(self) -> Self {
        Self {
            total_tokens: self
                .total_tokens
                .max(self.prompt_tokens.saturating_add(self.completion_tokens)),
            ..self
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
#[must_use = "a physical model attempt must be settled after provider completion"]
pub struct PhysicalModelAttempt {
    ledger_id: u64,
    attempt_id: u64,
    model: String,
    reserved_tokens: u64,
}

impl PhysicalModelAttempt {
    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn reserved_tokens(&self) -> u64 {
        self.reserved_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PendingModelAttempt {
    model: String,
    reserved_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResourceSnapshot {
    pub segment: RunResourceUsage,
    pub lineage: RunResourceUsage,
    #[serde(default)]
    mutation_revision: u64,
    #[serde(default)]
    ledger_id: u64,
    #[serde(default)]
    next_attempt_id: u64,
    #[serde(default)]
    pending_attempts: BTreeMap<u64, PendingModelAttempt>,
}

impl RunResourceSnapshot {
    pub fn mutation_revision(&self) -> u64 {
        self.mutation_revision
    }

    pub fn is_within_persistence_bounds(&self) -> bool {
        self.pending_attempts.len() <= MAX_PENDING_RESOURCE_ATTEMPTS
            && [&self.segment, &self.lineage].into_iter().all(|usage| {
                usage.models.len() <= MAX_RESOURCE_LEDGER_MODELS
                    && usage
                        .models
                        .keys()
                        .all(|model| model.len() <= MAX_RESOURCE_MODEL_KEY_BYTES)
            })
            && self
                .pending_attempts
                .values()
                .all(|attempt| attempt.model.len() <= MAX_RESOURCE_MODEL_KEY_BYTES)
            && serde_json::to_vec(self)
                .map(|encoded| encoded.len() <= MAX_PERSISTED_RESOURCE_SNAPSHOT_BYTES)
                .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceAdmissionError {
    ProtectedReserve,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResourceSettlement {
    pub(crate) settled: bool,
    pub(crate) exhausted: bool,
}

#[derive(Debug)]
pub(crate) struct RunResourceLedger {
    mutation_revision: u64,
    ledger_id: u64,
    next_attempt_id: u64,
    segment: RunResourceUsage,
    lineage: RunResourceUsage,
    pending_attempts: BTreeMap<u64, PendingModelAttempt>,
}

impl RunResourceLedger {
    pub(crate) fn new() -> Self {
        Self {
            mutation_revision: 0,
            ledger_id: next_ledger_id(),
            next_attempt_id: 1,
            segment: RunResourceUsage::default(),
            lineage: RunResourceUsage::default(),
            pending_attempts: BTreeMap::new(),
        }
    }

    pub(crate) fn from_snapshot(snapshot: RunResourceSnapshot) -> Self {
        let pending_attempts = snapshot
            .pending_attempts
            .into_iter()
            .map(|(attempt_id, attempt)| {
                (
                    attempt_id,
                    PendingModelAttempt {
                        model: bounded_model_identifier(&attempt.model),
                        reserved_tokens: attempt.reserved_tokens,
                    },
                )
            })
            .collect();
        Self {
            mutation_revision: snapshot.mutation_revision,
            ledger_id: if snapshot.ledger_id == 0 {
                next_ledger_id()
            } else {
                snapshot.ledger_id
            },
            next_attempt_id: snapshot.next_attempt_id.max(1),
            segment: bounded_usage(snapshot.segment),
            lineage: bounded_usage(snapshot.lineage),
            pending_attempts,
        }
    }

    pub(crate) fn from_persisted_snapshot(snapshot: RunResourceSnapshot) -> Self {
        let mut ledger = Self::from_snapshot(snapshot);
        ledger.settle_all_pending_as_unknown();
        ledger
    }

    pub(crate) fn snapshot(&self) -> RunResourceSnapshot {
        RunResourceSnapshot {
            segment: self.segment.clone(),
            lineage: self.lineage.clone(),
            mutation_revision: self.mutation_revision,
            ledger_id: self.ledger_id,
            next_attempt_id: self.next_attempt_id,
            pending_attempts: self.pending_attempts.clone(),
        }
    }

    pub(crate) fn start_new_segment(&mut self) {
        self.settle_all_pending_as_unknown();
        self.segment = RunResourceUsage::default();
        self.bump_mutation_revision();
    }

    pub(crate) fn absorb_completed_segment(&mut self, snapshot: RunResourceSnapshot) {
        let settled = RunResourceLedger::from_persisted_snapshot(snapshot).segment;
        merge_run_usage(&mut self.segment, settled.clone());
        merge_run_usage(&mut self.lineage, settled);
        self.bump_mutation_revision();
    }

    pub(crate) fn reserve(
        &mut self,
        budget: RunBudget,
        class: RunStageClass,
        model: &str,
        estimated_prompt_tokens: u64,
        max_completion_tokens: u64,
    ) -> Result<PhysicalModelAttempt, ResourceAdmissionError> {
        let reserved_tokens = estimated_prompt_tokens.saturating_add(max_completion_tokens);
        let next_attempt = self.segment.physical_attempts.saturating_add(1);
        let next_tokens = self
            .segment
            .total_tokens
            .saturating_add(self.segment.reserved_tokens)
            .saturating_add(reserved_tokens);
        let global_attempt_limit =
            u64::try_from(budget.max_physical_model_attempts).unwrap_or(u64::MAX);
        let protected_attempts =
            u64::try_from(budget.protected_physical_model_attempt_reserve(class))
                .unwrap_or(u64::MAX);
        let allowed_attempts = global_attempt_limit.saturating_sub(protected_attempts);
        let allowed_tokens = budget
            .max_total_tokens
            .saturating_sub(budget.protected_token_reserve(class));
        if (protected_attempts > 0 && next_attempt > allowed_attempts)
            || (budget.protected_token_reserve(class) > 0 && next_tokens > allowed_tokens)
        {
            return Err(ResourceAdmissionError::ProtectedReserve);
        }
        if next_attempt > global_attempt_limit || next_tokens > budget.max_total_tokens {
            return Err(ResourceAdmissionError::Exhausted);
        }
        if self.pending_attempts.len() >= MAX_PENDING_RESOURCE_ATTEMPTS {
            return Err(ResourceAdmissionError::Exhausted);
        }

        let attempt_id = self.next_attempt_id;
        if self.pending_attempts.contains_key(&attempt_id) {
            return Err(ResourceAdmissionError::Exhausted);
        }
        self.next_attempt_id = self.next_attempt_id.saturating_add(1);
        let model = bounded_model_key(&self.lineage.models, model);
        self.pending_attempts.insert(
            attempt_id,
            PendingModelAttempt {
                model: model.clone(),
                reserved_tokens,
            },
        );
        reserve_usage(&mut self.segment, &model, reserved_tokens);
        reserve_usage(&mut self.lineage, &model, reserved_tokens);
        self.bump_mutation_revision();
        Ok(PhysicalModelAttempt {
            ledger_id: self.ledger_id,
            attempt_id,
            model,
            reserved_tokens,
        })
    }

    pub(crate) fn settle(
        &mut self,
        budget: RunBudget,
        attempt: PhysicalModelAttempt,
        usage: Option<ModelAttemptUsage>,
    ) -> ResourceSettlement {
        if attempt.ledger_id != self.ledger_id {
            return ResourceSettlement {
                settled: false,
                exhausted: false,
            };
        }
        let Some(pending) = self.pending_attempts.remove(&attempt.attempt_id) else {
            return ResourceSettlement {
                settled: false,
                exhausted: false,
            };
        };
        if pending.model != attempt.model || pending.reserved_tokens != attempt.reserved_tokens {
            self.pending_attempts.insert(attempt.attempt_id, pending);
            return ResourceSettlement {
                settled: false,
                exhausted: false,
            };
        }

        let usage = usage
            .map(ModelAttemptUsage::normalized)
            .unwrap_or(ModelAttemptUsage {
                total_tokens: pending.reserved_tokens,
                source: ModelUsageSource::Unknown,
                ..ModelAttemptUsage::default()
            });
        settle_usage(
            &mut self.segment,
            &pending.model,
            pending.reserved_tokens,
            usage,
        );
        settle_usage(
            &mut self.lineage,
            &pending.model,
            pending.reserved_tokens,
            usage,
        );
        self.bump_mutation_revision();
        ResourceSettlement {
            settled: true,
            exhausted: self
                .segment
                .total_tokens
                .saturating_add(self.segment.reserved_tokens)
                > budget.max_total_tokens,
        }
    }

    fn settle_all_pending_as_unknown(&mut self) {
        let pending = std::mem::take(&mut self.pending_attempts);
        if pending.is_empty() {
            return;
        }
        for (_, attempt) in pending {
            let usage = ModelAttemptUsage {
                total_tokens: attempt.reserved_tokens,
                source: ModelUsageSource::Unknown,
                ..ModelAttemptUsage::default()
            };
            settle_usage(
                &mut self.segment,
                &attempt.model,
                attempt.reserved_tokens,
                usage,
            );
            settle_usage(
                &mut self.lineage,
                &attempt.model,
                attempt.reserved_tokens,
                usage,
            );
        }
        self.bump_mutation_revision();
    }

    fn bump_mutation_revision(&mut self) {
        self.mutation_revision = self.mutation_revision.saturating_add(1);
    }
}

fn next_ledger_id() -> u64 {
    NEXT_LEDGER_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_add(1))
        })
        .unwrap_or(u64::MAX)
        .max(1)
}

fn bounded_model_key(models: &BTreeMap<String, ModelResourceUsage>, model: &str) -> String {
    let model = bounded_model_identifier(model);
    if models.contains_key(&model) {
        return model;
    }
    if model == OTHER_MODELS_KEY || models.len() >= MAX_RESOURCE_LEDGER_MODELS.saturating_sub(1) {
        OTHER_MODELS_KEY.to_string()
    } else {
        model
    }
}

fn bounded_model_identifier(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        return UNKNOWN_MODEL_KEY.to_string();
    }
    if model.len() <= MAX_RESOURCE_MODEL_KEY_BYTES {
        return model.to_string();
    }
    let digest = Sha256::digest(model.as_bytes());
    let suffix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let prefix_budget = MAX_RESOURCE_MODEL_KEY_BYTES.saturating_sub(suffix.len() + 1);
    let mut prefix = String::new();
    for character in model.chars() {
        if prefix.len().saturating_add(character.len_utf8()) > prefix_budget {
            break;
        }
        prefix.push(character);
    }
    format!("{prefix}#{suffix}")
}

fn reserve_usage(usage: &mut RunResourceUsage, model: &str, reserved_tokens: u64) {
    usage.physical_attempts = usage.physical_attempts.saturating_add(1);
    usage.reserved_tokens = usage.reserved_tokens.saturating_add(reserved_tokens);
    let model_usage = usage.models.entry(model.to_string()).or_default();
    model_usage.physical_attempts = model_usage.physical_attempts.saturating_add(1);
    model_usage.reserved_tokens = model_usage.reserved_tokens.saturating_add(reserved_tokens);
}

fn settle_usage(
    usage: &mut RunResourceUsage,
    model: &str,
    reserved_tokens: u64,
    settled: ModelAttemptUsage,
) {
    usage.reserved_tokens = usage.reserved_tokens.saturating_sub(reserved_tokens);
    usage.prompt_tokens = usage.prompt_tokens.saturating_add(settled.prompt_tokens);
    usage.completion_tokens = usage
        .completion_tokens
        .saturating_add(settled.completion_tokens);
    usage.total_tokens = usage.total_tokens.saturating_add(settled.total_tokens);
    usage.usage_sources.record(settled.source);

    let model = bounded_model_key(&usage.models, model);
    let model_usage = usage.models.entry(model).or_default();
    model_usage.reserved_tokens = model_usage.reserved_tokens.saturating_sub(reserved_tokens);
    model_usage.prompt_tokens = model_usage
        .prompt_tokens
        .saturating_add(settled.prompt_tokens);
    model_usage.completion_tokens = model_usage
        .completion_tokens
        .saturating_add(settled.completion_tokens);
    model_usage.total_tokens = model_usage
        .total_tokens
        .saturating_add(settled.total_tokens);
    model_usage.usage_sources.record(settled.source);
}

fn bounded_usage(mut usage: RunResourceUsage) -> RunResourceUsage {
    let mut bounded = BTreeMap::new();
    let mut overflow = ModelResourceUsage::default();
    let mut overflow_used = false;
    for (model, model_usage) in std::mem::take(&mut usage.models) {
        let model = bounded_model_identifier(&model);
        if model == OTHER_MODELS_KEY {
            merge_model_usage(&mut overflow, model_usage);
            overflow_used = true;
        } else if let Some(existing) = bounded.get_mut(&model) {
            merge_model_usage(existing, model_usage);
        } else if bounded.len() < MAX_RESOURCE_LEDGER_MODELS.saturating_sub(1) {
            bounded.insert(model, model_usage);
        } else {
            merge_model_usage(&mut overflow, model_usage);
            overflow_used = true;
        }
    }
    if overflow_used {
        bounded.insert(OTHER_MODELS_KEY.to_string(), overflow);
    }
    usage.models = bounded;
    usage
}

fn merge_model_usage(target: &mut ModelResourceUsage, source: ModelResourceUsage) {
    target.physical_attempts = target
        .physical_attempts
        .saturating_add(source.physical_attempts);
    target.prompt_tokens = target.prompt_tokens.saturating_add(source.prompt_tokens);
    target.completion_tokens = target
        .completion_tokens
        .saturating_add(source.completion_tokens);
    target.total_tokens = target.total_tokens.saturating_add(source.total_tokens);
    target.reserved_tokens = target
        .reserved_tokens
        .saturating_add(source.reserved_tokens);
    target.usage_sources.provider = target
        .usage_sources
        .provider
        .saturating_add(source.usage_sources.provider);
    target.usage_sources.provider_partial = target
        .usage_sources
        .provider_partial
        .saturating_add(source.usage_sources.provider_partial);
    target.usage_sources.estimated = target
        .usage_sources
        .estimated
        .saturating_add(source.usage_sources.estimated);
    target.usage_sources.unknown = target
        .usage_sources
        .unknown
        .saturating_add(source.usage_sources.unknown);
}

fn merge_run_usage(target: &mut RunResourceUsage, source: RunResourceUsage) {
    target.physical_attempts = target
        .physical_attempts
        .saturating_add(source.physical_attempts);
    target.prompt_tokens = target.prompt_tokens.saturating_add(source.prompt_tokens);
    target.completion_tokens = target
        .completion_tokens
        .saturating_add(source.completion_tokens);
    target.total_tokens = target.total_tokens.saturating_add(source.total_tokens);
    target.reserved_tokens = target
        .reserved_tokens
        .saturating_add(source.reserved_tokens);
    target.usage_sources.provider = target
        .usage_sources
        .provider
        .saturating_add(source.usage_sources.provider);
    target.usage_sources.provider_partial = target
        .usage_sources
        .provider_partial
        .saturating_add(source.usage_sources.provider_partial);
    target.usage_sources.estimated = target
        .usage_sources
        .estimated
        .saturating_add(source.usage_sources.estimated);
    target.usage_sources.unknown = target
        .usage_sources
        .unknown
        .saturating_add(source.usage_sources.unknown);
    for (model, usage) in source.models {
        let key = bounded_model_key(&target.models, &model);
        merge_model_usage(target.models.entry(key).or_default(), usage);
    }
    *target = bounded_usage(std::mem::take(target));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_model_maps_reserve_the_overflow_slot() {
        let models = (0..MAX_RESOURCE_LEDGER_MODELS)
            .map(|index| {
                (
                    format!("model-{index:02}"),
                    ModelResourceUsage {
                        physical_attempts: 1,
                        total_tokens: 1,
                        ..ModelResourceUsage::default()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let usage = RunResourceUsage {
            physical_attempts: MAX_RESOURCE_LEDGER_MODELS as u64,
            total_tokens: MAX_RESOURCE_LEDGER_MODELS as u64,
            models,
            ..RunResourceUsage::default()
        };
        let mut ledger = RunResourceLedger::from_snapshot(RunResourceSnapshot {
            segment: usage.clone(),
            lineage: usage,
            ..RunResourceSnapshot::default()
        });
        let mut budget = RunBudget::for_effort("pro");
        budget.terminal_token_reserve = 0;
        budget.terminal_physical_model_attempt_reserve = 0;

        let attempt = ledger
            .reserve(budget, RunStageClass::Finalizer, "model-new", 1, 0)
            .expect("restored ledger should retain admission capacity");
        assert!(ledger.settle(budget, attempt, None).settled);
        let snapshot = ledger.snapshot();
        assert_eq!(snapshot.segment.models.len(), MAX_RESOURCE_LEDGER_MODELS);
        assert_eq!(snapshot.lineage.models.len(), MAX_RESOURCE_LEDGER_MODELS);
        assert!(snapshot.segment.models.contains_key(OTHER_MODELS_KEY));
        assert!(snapshot.lineage.models.contains_key(OTHER_MODELS_KEY));
    }

    #[test]
    fn resource_snapshot_round_trips_pending_reservations() {
        let mut budget = RunBudget::for_effort("fast");
        budget.terminal_token_reserve = 0;
        budget.terminal_physical_model_attempt_reserve = 0;
        let mut ledger = RunResourceLedger::new();
        let _attempt = ledger
            .reserve(budget, RunStageClass::Worker, "model-a", 12, 8)
            .expect("attempt should reserve");
        let encoded = serde_json::to_string(&ledger.snapshot()).expect("snapshot should encode");
        let decoded =
            serde_json::from_str::<RunResourceSnapshot>(&encoded).expect("snapshot should decode");

        assert_eq!(decoded.segment.physical_attempts, 1);
        assert_eq!(decoded.segment.reserved_tokens, 20);
        assert_eq!(decoded.lineage, decoded.segment);
        assert_eq!(decoded.pending_attempts.len(), 1);
        assert!(decoded.is_within_persistence_bounds());
    }

    #[test]
    fn resource_snapshot_bounds_model_keys_and_concurrent_pending_attempts() {
        let mut budget = RunBudget::for_effort("pro");
        budget.terminal_token_reserve = 0;
        budget.terminal_physical_model_attempt_reserve = 0;
        let mut ledger = RunResourceLedger::new();
        let long_model = "模型".repeat(MAX_RESOURCE_MODEL_KEY_BYTES);
        for _ in 0..MAX_PENDING_RESOURCE_ATTEMPTS {
            let _ = ledger
                .reserve(budget, RunStageClass::Finalizer, &long_model, 0, 0)
                .expect("bounded pending attempt should reserve");
        }
        assert_eq!(
            ledger.reserve(budget, RunStageClass::Finalizer, &long_model, 0, 0),
            Err(ResourceAdmissionError::Exhausted)
        );

        let snapshot = ledger.snapshot();
        assert!(snapshot
            .segment
            .models
            .keys()
            .all(|model| model.len() <= MAX_RESOURCE_MODEL_KEY_BYTES));
        assert!(snapshot.is_within_persistence_bounds());
        assert!(
            serde_json::to_vec(&snapshot).unwrap().len() <= MAX_PERSISTED_RESOURCE_SNAPSHOT_BYTES
        );
    }
}
