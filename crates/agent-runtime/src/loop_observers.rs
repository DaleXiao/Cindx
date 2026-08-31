//! Advisory loop observers for the agent loop.
//!
//! Observers watch two kernel hook points — after a tool observation enters the
//! transcript and before a model turn is prepared — and propose `Intervention`s.
//! The kernel applies each intervention to the loop state: hints reuse the
//! advisory overlay injection, host quarantines and the forced-final-turn flag
//! live in loop state, and a doom-loop confirmation flag is consumed by the
//! desktop layer to pause the run for an explicit user decision.
//!
//! Fault isolation: every observer hook runs inside `catch_unwind`. An observer
//! that panics is logged exactly once, disabled, and never propagated; it cannot
//! abort the loop or disturb the remaining observers.

use crate::repetition_advisory::{
    repetition_advisory_content, REPETITION_ADVISORY_KIND, REPETITION_ADVISORY_THRESHOLDS,
};
use agent_core::{Message, MessageRole, Metadata, ToolOutcomeStatus};
use std::collections::{BTreeMap, BTreeSet};

/// Tools whose failed web targets are tracked for stuck-target quarantine.
pub const STUCK_TARGET_TOOLS: &[&str] = &["web.fetch", "web.search"];
/// Consecutive failures of the same host before it is quarantined.
pub const STUCK_TARGET_QUARANTINE_THRESHOLD: usize = 3;
/// Consecutive identical tool calls before a doom-loop confirmation is
/// requested. Sits above the advisory repetition thresholds (3/5).
pub const DOOM_LOOP_CONFIRMATION_THRESHOLD: usize = 4;
/// Consecutive failure/stuck signals before the adaptive reasoning observer
/// raises the next model turn's thinking budget.
pub const ADAPTIVE_REASONING_ESCALATION_THRESHOLD: usize = 2;
/// Repetition streak (identical consecutive calls) that counts as a stuck
/// signal for adaptive reasoning escalation. Reuses the first advisory
/// repetition threshold so the budget reacts at the same point the model is
/// first warned.
pub const ADAPTIVE_REASONING_STUCK_REPETITION: usize = REPETITION_ADVISORY_THRESHOLDS[0];
/// Metadata kind of the one-shot leaked tool-call retry instruction.
pub const LEAKED_TOOL_CALL_RETRY_KIND: &str = "leaked_tool_call_retry";
/// One-shot repair instruction injected when a model turn writes a tool call
/// as body text instead of using the structured tool interface.
pub const LEAKED_TOOL_CALL_RETRY_INSTRUCTION: &str = "Your previous reply wrote a tool call as plain text in the message body instead of using the tool interface. Re-issue the intended action now through the structured tool interface; do not output tool-call JSON in the message body.";
/// Metadata kind of generic observer hint overlays.
pub const LOOP_OBSERVER_HINT_KIND: &str = "loop_observer_hint";
/// Permission-request metadata kind identifying a doom-loop confirmation.
pub const DOOM_LOOP_CONFIRMATION_KIND: &str = "doom_loop_confirmation";
/// Denial code recorded when a quarantined stuck target is blocked.
pub const STUCK_TARGET_BLOCKED_CODE: &str = "stuck_target_quarantined";
/// Appended to the system prompt of a forced final turn.
pub const FORCE_FINAL_TURN_INSTRUCTION: &str = "This run has reached its terminal model-call reserve. Give your final answer now. Do not call any more tools; state any unresolved limitation explicitly.";

const DEFAULT_TERMINAL_MODEL_CALL_RESERVE: usize = 4;
const UNKNOWN_REMAINING_MODEL_CALLS: usize = usize::MAX;
const MAX_OBSERVED_HOSTS: usize = 8;
const MAX_HOST_CHARS: usize = 253;
const OBSERVATION_EXCERPT_MAX_CHARS: usize = 8_192;
/// Deterministic output markers that classify a succeeded web call as a
/// stuck-target failure (empty-result or transport-failure payloads). Bare
/// words like "error" are deliberately not markers: they appear in ordinary
/// page text and would quarantine healthy hosts.
const STUCK_TARGET_FAILURE_MARKERS: &[&str] =
    &["no results", "0 results", "timed out", "could not resolve"];

/// Consecutive successes on a host that lift a prior quarantine: a host that
/// starts answering again was quarantined on a transient or misread signal,
/// and the block must not outlive the condition that caused it.
const STUCK_TARGET_RELEASE_THRESHOLD: usize = 2;

/// One intervention proposed by an observer. The kernel applies it to the loop
/// state; observers never mutate the loop directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intervention {
    /// Advisory text injected into the next model turn as an overlay.
    Hint(String),
    /// Block future web.search/web.fetch calls targeting this host.
    QuarantineHost(String),
    /// Lift a prior quarantine for this host after consecutive successes.
    ReleaseHostQuarantine(String),
    /// The next model turn drops all tools and carries the finalization
    /// instruction so the run can close out inside its budget.
    ForceFinalTurn,
    /// Pause the run and ask the user whether the stuck loop may continue.
    RequestDoomLoopConfirmation { tool: String },
    /// Set the next model turn's thinking budget (tokens). Always bounded by
    /// the run's reasoning-effort tier; the kernel writes it into the request
    /// reasoning metadata.
    SetThinkingBudget(u32),
}

/// An intervention together with the observer that proposed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverEmission {
    pub observer: String,
    pub intervention: Intervention,
}

/// Snapshot of the loop facts observers may read at a hook point.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObserverContext {
    pub tool_name: String,
    /// Canonical fingerprint of the tool call (tool name + canonical arguments).
    pub input_fingerprint: String,
    /// Canonical argument string (drives bounded advisory previews).
    pub canonical_input: String,
    pub outcome_status: Option<ToolOutcomeStatus>,
    /// HTTP hosts parsed from the call arguments and observation output.
    pub http_hosts: Vec<String>,
    pub remaining_model_calls: usize,
    pub terminal_model_call_reserve: usize,
    pub consecutive_identical_calls: usize,
    /// Maximum thinking budget of the run's reasoning-effort tier (0 when the
    /// tier does not enable thinking). Adaptive reasoning never exceeds it.
    pub reasoning_tier_max_budget: u32,
    /// Bounded excerpt of the tool observation for output-marker detection.
    pub observation_excerpt: String,
}

/// An advisory observer over the agent loop's hook points. Observers must be
/// `Send` because the loop state (and any suspended run carrying it) moves
/// between threads.
pub trait LoopObserver: Send {
    fn name(&self) -> &'static str;
    fn after_tool_observation(&mut self, _ctx: &ObserverContext) -> Vec<Intervention> {
        Vec::new()
    }
    fn before_model_turn(&mut self, _ctx: &ObserverContext) -> Vec<Intervention> {
        Vec::new()
    }
    fn clone_box(&self) -> Box<dyn LoopObserver>;
}

/// Dispatches hook calls to every registered observer with panic isolation: a
/// panicking observer is logged once, disabled, and never propagated.
#[derive(Default)]
pub struct ObserverRegistry {
    observers: Vec<Box<dyn LoopObserver>>,
    failed_observers: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy)]
enum ObserverHook {
    AfterToolObservation,
    BeforeModelTurn,
}

impl ObserverRegistry {
    pub fn register(&mut self, observer: Box<dyn LoopObserver>) {
        self.observers.push(observer);
    }

    pub fn with_observer(mut self, observer: Box<dyn LoopObserver>) -> Self {
        self.register(observer);
        self
    }

    pub fn observer_names(&self) -> Vec<&'static str> {
        self.observers
            .iter()
            .map(|observer| observer.name())
            .collect()
    }

    /// Observers that panicked and were isolated from the dispatch path.
    pub fn failed_observers(&self) -> &BTreeSet<String> {
        &self.failed_observers
    }

    pub fn notify_after_tool_observation(
        &mut self,
        ctx: &ObserverContext,
    ) -> Vec<ObserverEmission> {
        self.notify(ctx, ObserverHook::AfterToolObservation)
    }

    pub fn notify_before_model_turn(&mut self, ctx: &ObserverContext) -> Vec<ObserverEmission> {
        self.notify(ctx, ObserverHook::BeforeModelTurn)
    }

    fn notify(&mut self, ctx: &ObserverContext, hook: ObserverHook) -> Vec<ObserverEmission> {
        let mut emissions = Vec::new();
        let mut newly_failed = Vec::new();
        for observer in self.observers.iter_mut() {
            let name = observer.name();
            if self.failed_observers.contains(name) {
                continue;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match hook {
                ObserverHook::AfterToolObservation => observer.after_tool_observation(ctx),
                ObserverHook::BeforeModelTurn => observer.before_model_turn(ctx),
            }));
            match result {
                Ok(interventions) => {
                    emissions.extend(interventions.into_iter().map(|intervention| {
                        ObserverEmission {
                            observer: name.to_string(),
                            intervention,
                        }
                    }))
                }
                Err(_) => newly_failed.push(name),
            }
        }
        for name in newly_failed {
            if self.failed_observers.insert(name.to_string()) {
                eprintln!("loop observer `{name}` panicked and was isolated from the agent loop");
            }
        }
        emissions
    }
}

impl Clone for ObserverRegistry {
    fn clone(&self) -> Self {
        Self {
            observers: self
                .observers
                .iter()
                .map(|observer| observer.clone_box())
                .collect(),
            failed_observers: self.failed_observers.clone(),
        }
    }
}

impl PartialEq for ObserverRegistry {
    fn eq(&self, other: &Self) -> bool {
        self.observer_names() == other.observer_names()
            && self.failed_observers == other.failed_observers
    }
}

impl Eq for ObserverRegistry {}

impl std::fmt::Debug for ObserverRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObserverRegistry")
            .field("observers", &self.observer_names())
            .field("failed_observers", &self.failed_observers)
            .finish()
    }
}

/// Advisory repetition notice as an observer: fires a hint exactly at the
/// existing advisory thresholds. The streak itself remains owned by
/// `RepetitionAdvisoryTracker` in loop state, so behavior is unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct RepetitionAdvisoryObserver;

impl LoopObserver for RepetitionAdvisoryObserver {
    fn name(&self) -> &'static str {
        REPETITION_ADVISORY_KIND
    }

    fn before_model_turn(&mut self, ctx: &ObserverContext) -> Vec<Intervention> {
        if REPETITION_ADVISORY_THRESHOLDS.contains(&ctx.consecutive_identical_calls) {
            vec![Intervention::Hint(repetition_advisory_content(
                &ctx.tool_name,
                ctx.consecutive_identical_calls,
                &ctx.canonical_input,
            ))]
        } else {
            Vec::new()
        }
    }

    fn clone_box(&self) -> Box<dyn LoopObserver> {
        Box::new(*self)
    }
}

/// Tracks failed web.search/web.fetch calls per host; after
/// `STUCK_TARGET_QUARANTINE_THRESHOLD` consecutive failures of the same host
/// the host is quarantined. One success for a host clears its failure count.
#[derive(Debug, Clone)]
pub struct StuckTargetObserver {
    failure_streaks: BTreeMap<String, usize>,
    success_streaks: BTreeMap<String, usize>,
    quarantine_threshold: usize,
}

impl Default for StuckTargetObserver {
    fn default() -> Self {
        Self {
            failure_streaks: BTreeMap::new(),
            success_streaks: BTreeMap::new(),
            quarantine_threshold: STUCK_TARGET_QUARANTINE_THRESHOLD,
        }
    }
}

impl StuckTargetObserver {
    pub fn with_threshold(quarantine_threshold: usize) -> Self {
        Self {
            quarantine_threshold: quarantine_threshold.max(1),
            ..Self::default()
        }
    }

    /// Consecutive failure count currently tracked for a host.
    pub fn failure_streak(&self, host: &str) -> usize {
        self.failure_streaks.get(host).copied().unwrap_or_default()
    }
}

impl LoopObserver for StuckTargetObserver {
    fn name(&self) -> &'static str {
        "stuck_target"
    }

    fn after_tool_observation(&mut self, ctx: &ObserverContext) -> Vec<Intervention> {
        if !STUCK_TARGET_TOOLS.contains(&ctx.tool_name.as_str()) || ctx.http_hosts.is_empty() {
            return Vec::new();
        }
        let failed = matches!(ctx.outcome_status, Some(ToolOutcomeStatus::Failed))
            || (matches!(ctx.outcome_status, Some(ToolOutcomeStatus::Succeeded))
                && observation_signals_stuck_failure(&ctx.observation_excerpt));
        let succeeded = matches!(ctx.outcome_status, Some(ToolOutcomeStatus::Succeeded)) && !failed;
        if !failed && !succeeded {
            return Vec::new();
        }
        let mut interventions = Vec::new();
        for host in &ctx.http_hosts {
            if failed {
                self.success_streaks.remove(host);
                let streak = self.failure_streaks.entry(host.clone()).or_default();
                *streak = streak.saturating_add(1);
                if *streak >= self.quarantine_threshold {
                    interventions.push(Intervention::QuarantineHost(host.clone()));
                }
            } else {
                self.failure_streaks.remove(host);
                let streak = self.success_streaks.entry(host.clone()).or_default();
                *streak = streak.saturating_add(1);
                if *streak >= STUCK_TARGET_RELEASE_THRESHOLD {
                    self.success_streaks.remove(host);
                    // Idempotent at the host: releasing a host that was never
                    // quarantined is a no-op.
                    interventions.push(Intervention::ReleaseHostQuarantine(host.clone()));
                }
            }
        }
        interventions
    }

    fn clone_box(&self) -> Box<dyn LoopObserver> {
        Box::new(self.clone())
    }
}

/// Forces the final turn once the remaining model-call budget reaches the
/// terminal reserve, so the loop can always close out inside its budget.
#[derive(Debug, Clone, Copy, Default)]
pub struct FinalizationObserver;

impl LoopObserver for FinalizationObserver {
    fn name(&self) -> &'static str {
        "finalization"
    }

    fn after_tool_observation(&mut self, ctx: &ObserverContext) -> Vec<Intervention> {
        finalization_interventions(ctx)
    }

    fn before_model_turn(&mut self, ctx: &ObserverContext) -> Vec<Intervention> {
        finalization_interventions(ctx)
    }

    fn clone_box(&self) -> Box<dyn LoopObserver> {
        Box::new(*self)
    }
}

fn finalization_interventions(ctx: &ObserverContext) -> Vec<Intervention> {
    if ctx.remaining_model_calls <= ctx.terminal_model_call_reserve {
        vec![Intervention::ForceFinalTurn]
    } else {
        Vec::new()
    }
}

/// Requests an explicit user confirmation when the identical-call streak
/// reaches the doom-loop threshold (default 4, above the advisory 3/5).
#[derive(Debug, Clone)]
pub struct DoomLoopObserver {
    threshold: usize,
}

impl Default for DoomLoopObserver {
    fn default() -> Self {
        Self {
            threshold: DOOM_LOOP_CONFIRMATION_THRESHOLD,
        }
    }
}

impl DoomLoopObserver {
    pub fn with_threshold(threshold: usize) -> Self {
        Self {
            threshold: threshold.max(2),
        }
    }
}

impl LoopObserver for DoomLoopObserver {
    fn name(&self) -> &'static str {
        "doom_loop"
    }

    fn after_tool_observation(&mut self, ctx: &ObserverContext) -> Vec<Intervention> {
        if ctx.consecutive_identical_calls == self.threshold {
            vec![Intervention::RequestDoomLoopConfirmation {
                tool: ctx.tool_name.clone(),
            }]
        } else {
            Vec::new()
        }
    }

    fn clone_box(&self) -> Box<dyn LoopObserver> {
        Box::new(self.clone())
    }
}

/// Maximum per-turn thinking budget of a reasoning-effort tier. Mirrors the
/// provider's tier budgets; `0` marks tiers that do not enable thinking
/// (fast/absent), where adaptive reasoning stays inactive.
pub fn reasoning_tier_max_thinking_budget(effort: Option<&str>) -> u32 {
    match effort {
        Some("default") => 4096,
        Some("high") => 8192,
        Some("xhigh") => 16384,
        _ => 0,
    }
}

/// Per-turn adaptive reasoning budget (prepareNextTurn-style). Watches tool
/// outcomes for consecutive failure/stuck signals — failed outcomes and
/// repetition streaks at the advisory threshold — and raises the next model
/// turn's thinking budget inside the run's effort tier; a succeeding tool
/// falls back to the tier's base budget. The ceiling is always the tier's own
/// maximum (`default` 4096 / `high` 8192 / `xhigh` 16384), never crossing
/// into a higher tier, and a tier without thinking (cap 0) never emits.
/// Without any signal the observer emits nothing, so the request keeps the
/// tier's default budget.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdaptiveReasoningObserver {
    failure_streak: usize,
    current_budget: u32,
    observed_cap: u32,
}

impl AdaptiveReasoningObserver {
    /// The economical base budget of a tier: half the tier ceiling, at least
    /// one token.
    pub fn base_budget(tier_max_budget: u32) -> u32 {
        // Half the tier ceiling so even steady turns get meaningful thinking
        // depth.
        (tier_max_budget / 2).max(1)
    }

    /// The budget the observer currently holds for the next turn.
    pub fn current_budget(&self) -> u32 {
        self.current_budget
    }

    /// The consecutive failure/stuck signal count.
    pub fn failure_streak(&self) -> usize {
        self.failure_streak
    }
}

impl LoopObserver for AdaptiveReasoningObserver {
    fn name(&self) -> &'static str {
        "adaptive_reasoning"
    }

    fn after_tool_observation(&mut self, ctx: &ObserverContext) -> Vec<Intervention> {
        let cap = ctx.reasoning_tier_max_budget;
        if cap == 0 {
            return Vec::new();
        }
        if self.observed_cap != cap {
            self.observed_cap = cap;
            self.current_budget = Self::base_budget(cap);
            self.failure_streak = 0;
        }
        let failed = matches!(ctx.outcome_status, Some(ToolOutcomeStatus::Failed));
        let stuck = ctx.consecutive_identical_calls >= ADAPTIVE_REASONING_STUCK_REPETITION;
        if failed || stuck {
            self.failure_streak = self.failure_streak.saturating_add(1);
            if self.failure_streak >= ADAPTIVE_REASONING_ESCALATION_THRESHOLD
                && self.current_budget < cap
            {
                self.current_budget = self.current_budget.saturating_mul(2).min(cap);
                return vec![Intervention::SetThinkingBudget(self.current_budget)];
            }
            return Vec::new();
        }
        if matches!(ctx.outcome_status, Some(ToolOutcomeStatus::Succeeded)) {
            self.failure_streak = 0;
            let base = Self::base_budget(cap);
            if self.current_budget > base {
                self.current_budget = base;
                return vec![Intervention::SetThinkingBudget(base)];
            }
        }
        Vec::new()
    }

    fn clone_box(&self) -> Box<dyn LoopObserver> {
        Box::new(*self)
    }
}

/// Runtime-only observer host carried by the loop state: the observer registry
/// plus the intervention effects (quarantine set, forced-final flag, pending
/// doom-loop confirmation, pending hint overlays, pending thinking budget, the
/// one-shot leaked-retry flag, the one-shot overflow-compaction flag, and the
/// published model-call budget). Not persisted: restored runs start with a
/// fresh host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopObservers {
    registry: ObserverRegistry,
    quarantined_hosts: BTreeSet<String>,
    force_final_turn: bool,
    pending_doom_loop_confirmation: Option<String>,
    hint_overlays: Vec<Message>,
    pending_thinking_budget: Option<u32>,
    reasoning_tier_max_budget: u32,
    leaked_retry_used: bool,
    overflow_compact_used: bool,
    remaining_model_calls: usize,
    terminal_model_call_reserve: usize,
}

impl Default for LoopObservers {
    fn default() -> Self {
        let registry = ObserverRegistry::default()
            .with_observer(Box::new(RepetitionAdvisoryObserver))
            .with_observer(Box::new(StuckTargetObserver::default()))
            .with_observer(Box::new(FinalizationObserver))
            .with_observer(Box::new(DoomLoopObserver::default()))
            .with_observer(Box::new(AdaptiveReasoningObserver::default()));
        Self {
            registry,
            quarantined_hosts: BTreeSet::new(),
            force_final_turn: false,
            pending_doom_loop_confirmation: None,
            hint_overlays: Vec::new(),
            pending_thinking_budget: None,
            reasoning_tier_max_budget: 0,
            leaked_retry_used: false,
            overflow_compact_used: false,
            remaining_model_calls: UNKNOWN_REMAINING_MODEL_CALLS,
            terminal_model_call_reserve: DEFAULT_TERMINAL_MODEL_CALL_RESERVE,
        }
    }
}

impl LoopObservers {
    pub fn with_registry(registry: ObserverRegistry) -> Self {
        Self {
            registry,
            ..Self::default()
        }
    }

    pub fn register(&mut self, observer: Box<dyn LoopObserver>) {
        self.registry.register(observer);
    }

    pub fn observer_names(&self) -> Vec<&'static str> {
        self.registry.observer_names()
    }

    pub fn failed_observers(&self) -> &BTreeSet<String> {
        self.registry.failed_observers()
    }

    pub fn notify_after_tool_observation(
        &mut self,
        ctx: &ObserverContext,
    ) -> Vec<ObserverEmission> {
        self.registry.notify_after_tool_observation(ctx)
    }

    pub fn notify_before_model_turn(&mut self, ctx: &ObserverContext) -> Vec<ObserverEmission> {
        self.registry.notify_before_model_turn(ctx)
    }

    /// Applies one emission to the loop-state effects. Hints become generic
    /// hint overlays; the kernel substitutes the byte-identical repetition
    /// advisory message for the repetition observer.
    pub fn apply_emission(&mut self, emission: &ObserverEmission) {
        match &emission.intervention {
            Intervention::Hint(text) => self
                .hint_overlays
                .push(generic_hint_message(&emission.observer, text)),
            Intervention::QuarantineHost(host) => self.quarantine_host(host),
            Intervention::ReleaseHostQuarantine(host) => self.release_host_quarantine(host),
            Intervention::ForceFinalTurn => self.arm_force_final_turn(),
            Intervention::RequestDoomLoopConfirmation { tool } => {
                self.request_doom_loop_confirmation(tool.clone());
            }
            Intervention::SetThinkingBudget(budget) => {
                self.pending_thinking_budget = Some(*budget);
            }
        }
    }

    pub fn quarantine_host(&mut self, host: &str) {
        let normalized = normalize_host(host);
        if !normalized.is_empty() {
            self.quarantined_hosts.insert(normalized);
        }
    }

    /// Lift a host's quarantine. Releasing a host that is not quarantined is a
    /// no-op, so observers may propose releases unconditionally after
    /// consecutive successes.
    pub fn release_host_quarantine(&mut self, host: &str) {
        let normalized = normalize_host(host);
        if !normalized.is_empty() {
            self.quarantined_hosts.remove(&normalized);
        }
    }

    pub fn quarantined_hosts(&self) -> &BTreeSet<String> {
        &self.quarantined_hosts
    }

    pub fn is_host_quarantined(&self, host: &str) -> bool {
        self.quarantined_hosts.contains(&normalize_host(host))
    }

    pub fn arm_force_final_turn(&mut self) {
        self.force_final_turn = true;
    }

    pub fn force_final_turn(&self) -> bool {
        self.force_final_turn
    }

    pub fn request_doom_loop_confirmation(&mut self, tool: String) {
        self.pending_doom_loop_confirmation.get_or_insert(tool);
    }

    pub fn pending_doom_loop_confirmation(&self) -> Option<&str> {
        self.pending_doom_loop_confirmation.as_deref()
    }

    pub fn take_pending_doom_loop_confirmation(&mut self) -> Option<String> {
        self.pending_doom_loop_confirmation.take()
    }

    /// Clears a pending doom-loop confirmation without resetting the streak.
    pub fn resolve_doom_loop_confirmation(&mut self) {
        self.pending_doom_loop_confirmation = None;
    }

    pub fn push_hint_overlay(&mut self, message: Message) {
        self.hint_overlays.push(message);
    }

    /// Drains the pending hint overlays for injection into the next turn.
    pub fn take_hint_overlays(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.hint_overlays)
    }

    /// The thinking budget the observers set for the next model request, if
    /// any. Persists across turns until the observers change it; the kernel
    /// writes it into the request reasoning metadata.
    pub fn pending_thinking_budget(&self) -> Option<u32> {
        self.pending_thinking_budget
    }

    /// Publishes the run's reasoning-effort tier ceiling so the adaptive
    /// reasoning observer knows the budget range it may move inside (0 when
    /// the tier does not enable thinking).
    pub fn publish_reasoning_effort_budget(&mut self, tier_max_budget: u32) {
        self.reasoning_tier_max_budget = tier_max_budget;
    }

    /// The published reasoning-effort tier ceiling.
    pub fn reasoning_tier_max_budget(&self) -> u32 {
        self.reasoning_tier_max_budget
    }

    /// True once the one-shot leaked tool-call retry has been consumed.
    pub fn leaked_retry_used(&self) -> bool {
        self.leaked_retry_used
    }

    /// Consumes the one-shot leaked tool-call retry allowance.
    pub fn mark_leaked_retry_used(&mut self) {
        self.leaked_retry_used = true;
    }

    /// True once the one-shot overflow compaction re-entry has been consumed.
    pub fn overflow_compact_used(&self) -> bool {
        self.overflow_compact_used
    }

    /// Consumes the one-shot overflow compaction allowance.
    pub fn mark_overflow_compact_used(&mut self) {
        self.overflow_compact_used = true;
    }

    pub fn remaining_model_calls(&self) -> usize {
        self.remaining_model_calls
    }

    pub fn terminal_model_call_reserve(&self) -> usize {
        self.terminal_model_call_reserve
    }

    /// Publishes the run's latest known model-call budget view so the
    /// finalization observer can decide when the terminal reserve starts.
    pub fn publish_model_call_budget(
        &mut self,
        remaining_model_calls: usize,
        terminal_reserve: usize,
    ) {
        self.remaining_model_calls = remaining_model_calls;
        self.terminal_model_call_reserve = terminal_reserve.max(1);
    }
}

/// Clears a pending doom-loop confirmation and resets the repetition streak so
/// a user-confirmed continuation starts from a fresh count ("continue once").
pub fn clear_doom_loop_confirmation(state: &mut crate::AgentLoopState) {
    state.repetition_advisory = crate::RepetitionAdvisoryTracker::default();
    state.loop_observers.resolve_doom_loop_confirmation();
}

/// The quarantined host blocking a web.search/web.fetch call, if any.
pub fn quarantined_web_target_host(
    state: &crate::AgentLoopState,
    tool_name: &str,
    input_json: &str,
) -> Option<String> {
    if !STUCK_TARGET_TOOLS.contains(&tool_name) {
        return None;
    }
    http_hosts_for_tool_io(tool_name, input_json, "")
        .into_iter()
        .find(|host| state.loop_observers.is_host_quarantined(host))
}

/// The model-facing observation for a quarantined stuck target (denied
/// semantics; the call never executes).
pub fn stuck_target_blocked_observation(host: &str) -> String {
    format!(
        "stuck target blocked: {host}\nThis host has failed repeatedly in this run and is quarantined for the rest of the run. Do not retry it; switch to a different source or report the blocker."
    )
}

/// The bounded observation excerpt carried to observers.
pub fn observation_excerpt(observation: &str) -> String {
    observation
        .chars()
        .take(OBSERVATION_EXCERPT_MAX_CHARS)
        .collect()
}

/// The overlay message for a generic observer hint.
pub fn generic_hint_message(observer: &str, text: &str) -> Message {
    Message {
        role: MessageRole::System,
        content: text.to_string(),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), LOOP_OBSERVER_HINT_KIND.to_string()),
            ("observer".to_string(), observer.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>(),
    }
}

/// Heuristic detection of a tool call that leaked into assistant body text:
/// an explicit `<tool_call>` marker, a ```json fence naming a `"name"` field,
/// or a `{"name": ..., "arguments": ...}` object literal.
pub fn assistant_text_leaks_tool_call(content: &str) -> bool {
    content.contains("<tool_call")
        || (content.contains("```json") && content.contains("\"name\""))
        || (content.contains("{\"name\"") && content.contains("\"arguments\""))
}

/// Append an internal (model-visible, non-surface) instruction message to the
/// loop state, tagged with a stable `kind` for attribution and dedupe.
pub fn append_internal_instruction(
    state: &mut crate::AgentLoopState,
    kind: &str,
    instruction: &str,
) {
    state.messages.push(Message {
        role: MessageRole::System,
        content: instruction.to_string(),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), kind.to_string()),
        ]
        .into_iter()
        .collect(),
    });
}

/// One-shot repair for a completed turn whose body text leaked a tool call.
/// When the answer carries a leaked call and the retry allowance is unused,
/// withdraws the assistant message added since `previous_message_count`,
/// records the allowance as used, injects the one-shot repair instruction,
/// and returns the retry instruction for the loop to re-enter the same turn.
/// Returns `None` (no retry) when there is no leak or the allowance is
/// already spent, so the loop never retries twice.
pub fn consume_leaked_tool_call_retry(
    state: &mut crate::AgentLoopState,
    answer: &str,
    previous_message_count: usize,
) -> Option<String> {
    if state.loop_observers.leaked_retry_used() || !assistant_text_leaks_tool_call(answer) {
        return None;
    }
    state
        .messages
        .truncate(previous_message_count.min(state.messages.len()));
    state.loop_observers.mark_leaked_retry_used();
    append_internal_instruction(
        state,
        LEAKED_TOOL_CALL_RETRY_KIND,
        LEAKED_TOOL_CALL_RETRY_INSTRUCTION,
    );
    Some(LEAKED_TOOL_CALL_RETRY_INSTRUCTION.to_string())
}

/// HTTP hosts attributed to one tool call: parsed from the call arguments
/// first, falling back to the observation output when the arguments name no
/// host (e.g. a web.search query).
pub fn http_hosts_for_tool_io(tool_name: &str, input_json: &str, observation: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    if tool_name == "web.search" {
        if let Some(query) = serde_json::from_str::<serde_json::Value>(input_json)
            .ok()
            .and_then(|value| {
                value
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
        {
            collect_site_token_hosts(&query, &mut hosts);
        }
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(input_json) {
        collect_url_hosts_from_json(&value, &mut hosts);
    }
    if hosts.is_empty() && !observation.trim().is_empty() {
        collect_url_hosts(observation, &mut hosts);
    }
    finalize_hosts(hosts)
}

fn observation_signals_stuck_failure(observation: &str) -> bool {
    let lower = observation.to_lowercase();
    STUCK_TARGET_FAILURE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

fn collect_url_hosts_from_json(value: &serde_json::Value, hosts: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => collect_url_hosts(text, hosts),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_url_hosts_from_json(item, hosts);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                collect_url_hosts_from_json(item, hosts);
            }
        }
        _ => {}
    }
}

fn collect_url_hosts(text: &str, hosts: &mut Vec<String>) {
    let lower = text.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("://") {
        let scheme_end = cursor + offset;
        let scheme_start = scheme_end.saturating_sub(5);
        let scheme = &lower[scheme_start..scheme_end];
        if scheme.ends_with("http") || scheme.ends_with("https") {
            let host_start = scheme_end + 3;
            let host_end = lower[host_start..]
                .find(|character| {
                    matches!(character, '/' | '?' | '#' | '"' | '\'' | '<' | '>' | ' ')
                })
                .map(|offset| host_start + offset)
                .unwrap_or(lower.len());
            let host = normalize_host(&text[host_start..host_end]);
            if !host.is_empty() {
                hosts.push(host);
            }
        }
        cursor = scheme_end + 3;
    }
}

fn collect_site_token_hosts(query: &str, hosts: &mut Vec<String>) {
    for token in query.split_whitespace() {
        let Some(site) = token
            .strip_prefix("site:")
            .or_else(|| token.strip_prefix("SITE:"))
        else {
            continue;
        };
        let host = normalize_host(site);
        if !host.is_empty() {
            hosts.push(host);
        }
    }
}

fn finalize_hosts(hosts: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for host in hosts {
        if host.chars().count() > MAX_HOST_CHARS || !seen.insert(host.clone()) {
            continue;
        }
        ordered.push(host);
        if ordered.len() >= MAX_OBSERVED_HOSTS {
            break;
        }
    }
    ordered
}

/// Canonical host form: lowercase, without userinfo or port.
pub fn normalize_host(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_userinfo = trimmed.rsplit_once('@').map_or(trimmed, |(_, host)| host);
    let host = if without_userinfo.starts_with('[') {
        without_userinfo
            .split_once(']')
            .map(|(host, _)| host.strip_prefix('[').unwrap_or(host))
            .unwrap_or(without_userinfo)
    } else {
        without_userinfo
            .split_once(':')
            .map(|(host, _)| host)
            .unwrap_or(without_userinfo)
    };
    host.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{start_agent_loop, tool_input_fingerprint, AgentRuntimeConfig};
    use agent_core::TaskId;

    struct PanickingObserver;

    impl LoopObserver for PanickingObserver {
        fn name(&self) -> &'static str {
            "panicking"
        }

        fn after_tool_observation(&mut self, _ctx: &ObserverContext) -> Vec<Intervention> {
            panic!("injected observer failure");
        }

        fn before_model_turn(&mut self, _ctx: &ObserverContext) -> Vec<Intervention> {
            panic!("injected observer failure");
        }

        fn clone_box(&self) -> Box<dyn LoopObserver> {
            Box::new(PanickingObserver)
        }
    }

    struct CountingObserver {
        name: &'static str,
    }

    impl LoopObserver for CountingObserver {
        fn name(&self) -> &'static str {
            self.name
        }

        fn after_tool_observation(&mut self, _ctx: &ObserverContext) -> Vec<Intervention> {
            vec![Intervention::Hint(format!("{} hint", self.name))]
        }

        fn clone_box(&self) -> Box<dyn LoopObserver> {
            Box::new(Self { name: self.name })
        }
    }

    fn tool_context(tool_name: &str, status: ToolOutcomeStatus, hosts: &[&str]) -> ObserverContext {
        ObserverContext {
            tool_name: tool_name.to_string(),
            input_fingerprint: tool_input_fingerprint(tool_name, "{}"),
            canonical_input: "{}".to_string(),
            outcome_status: Some(status),
            http_hosts: hosts.iter().map(|host| host.to_string()).collect(),
            remaining_model_calls: usize::MAX,
            terminal_model_call_reserve: 4,
            consecutive_identical_calls: 0,
            reasoning_tier_max_budget: 0,
            observation_excerpt: String::new(),
        }
    }

    #[test]
    fn a_panicking_observer_is_isolated_and_logged_once() {
        let mut registry = ObserverRegistry::default()
            .with_observer(Box::new(PanickingObserver))
            .with_observer(Box::new(CountingObserver { name: "healthy" }));
        let ctx = tool_context("web.fetch", ToolOutcomeStatus::Failed, &["example.com"]);

        for _ in 0..3 {
            let emissions = registry.notify_after_tool_observation(&ctx);
            assert_eq!(emissions.len(), 1, "only the healthy observer emits");
            assert_eq!(emissions[0].observer, "healthy");
        }
        assert_eq!(
            registry.failed_observers().iter().collect::<Vec<_>>(),
            vec!["panicking"],
            "the panicking observer is recorded exactly once"
        );
        assert_eq!(registry.observer_names().len(), 2);
    }

    #[test]
    fn a_panicking_observer_does_not_disturb_the_loop_state_host() {
        let mut state = start_agent_loop(
            TaskId("observer-panic".to_string()),
            "stay live",
            AgentRuntimeConfig::default(),
        );
        state.loop_observers.register(Box::new(PanickingObserver));

        let ctx = tool_context("web.fetch", ToolOutcomeStatus::Failed, &["example.com"]);
        let emissions = state.loop_observers.notify_after_tool_observation(&ctx);
        for emission in &emissions {
            state.loop_observers.apply_emission(emission);
        }
        assert!(emissions.is_empty());
        assert!(state
            .loop_observers
            .failed_observers()
            .contains("panicking"));
        assert!(
            state.loop_observers.quarantined_hosts().is_empty(),
            "no intervention leaked from the panicking observer"
        );
        assert_eq!(state.messages.len(), 1, "the transcript is untouched");
    }

    #[test]
    fn registry_clone_preserves_observers_and_failures() {
        let mut registry = ObserverRegistry::default()
            .with_observer(Box::new(PanickingObserver))
            .with_observer(Box::new(CountingObserver { name: "healthy" }));
        let ctx = tool_context("web.fetch", ToolOutcomeStatus::Failed, &[]);
        registry.notify_after_tool_observation(&ctx);

        let cloned = registry.clone();
        assert_eq!(cloned.observer_names(), registry.observer_names());
        assert_eq!(cloned.failed_observers(), registry.failed_observers());
    }

    #[test]
    fn stuck_target_observer_quarantines_after_three_consecutive_failures() {
        let mut observer = StuckTargetObserver::default();
        let failed = tool_context("web.fetch", ToolOutcomeStatus::Failed, &["example.com"]);

        let mut quarantined = Vec::new();
        for attempt in 0..3 {
            for intervention in observer.after_tool_observation(&failed) {
                if let Intervention::QuarantineHost(host) = intervention {
                    quarantined.push((attempt, host));
                }
            }
        }
        assert_eq!(observer.failure_streak("example.com"), 3);
        assert_eq!(quarantined, vec![(2, "example.com".to_string())]);
    }

    #[test]
    fn stuck_target_observer_clears_the_count_on_success() {
        let mut observer = StuckTargetObserver::default();
        let failed = tool_context("web.fetch", ToolOutcomeStatus::Failed, &["example.com"]);
        observer.after_tool_observation(&failed);
        observer.after_tool_observation(&failed);
        assert_eq!(observer.failure_streak("example.com"), 2);

        let succeeded = tool_context("web.fetch", ToolOutcomeStatus::Succeeded, &["example.com"]);
        assert!(observer.after_tool_observation(&succeeded).is_empty());
        assert_eq!(observer.failure_streak("example.com"), 0);

        // The streak restarts from zero after a success.
        observer.after_tool_observation(&failed);
        observer.after_tool_observation(&failed);
        assert!(observer
            .after_tool_observation(&failed)
            .iter()
            .any(|intervention| matches!(intervention, Intervention::QuarantineHost(host) if host == "example.com")));
    }

    #[test]
    fn stuck_target_observer_releases_quarantine_after_two_consecutive_successes() {
        let mut observer = StuckTargetObserver::default();
        let succeeded = tool_context("web.fetch", ToolOutcomeStatus::Succeeded, &["example.com"]);

        assert_eq!(
            observer.after_tool_observation(&succeeded),
            Vec::new(),
            "one success is not yet enough to release"
        );
        let interventions = observer.after_tool_observation(&succeeded);
        assert_eq!(
            interventions,
            vec![Intervention::ReleaseHostQuarantine(
                "example.com".to_string()
            )]
        );

        // The success streak restarts after a release, so steady successes do
        // not spam releases; a failure in between resets it entirely.
        assert_eq!(observer.after_tool_observation(&succeeded), Vec::new());
        let failed = tool_context("web.fetch", ToolOutcomeStatus::Failed, &["example.com"]);
        observer.after_tool_observation(&failed);
        assert_eq!(observer.after_tool_observation(&succeeded), Vec::new());
    }

    #[test]
    fn succeeded_pages_mentioning_error_are_not_stuck_failures() {
        let mut observer = StuckTargetObserver::default();
        let mut healthy = tool_context("web.fetch", ToolOutcomeStatus::Succeeded, &["example.com"]);
        healthy.observation_excerpt =
            "status=succeeded\noutput=\nHow to read an error page: a tutorial.".to_string();

        for _ in 0..4 {
            observer.after_tool_observation(&healthy);
        }
        assert_eq!(
            observer.failure_streak("example.com"),
            0,
            "the bare word 'error' in page text must not quarantine a healthy host"
        );
    }

    #[test]
    fn loop_observers_host_lifts_quarantine_through_release_emissions() {
        let mut host = LoopObservers::default();
        host.apply_emission(&ObserverEmission {
            observer: "stuck_target".to_string(),
            intervention: Intervention::QuarantineHost("example.com".to_string()),
        });
        assert!(host.is_host_quarantined("example.com"));

        host.apply_emission(&ObserverEmission {
            observer: "stuck_target".to_string(),
            intervention: Intervention::ReleaseHostQuarantine("example.com".to_string()),
        });
        assert!(!host.is_host_quarantined("example.com"));

        // Releasing an unquarantined host is a no-op.
        host.apply_emission(&ObserverEmission {
            observer: "stuck_target".to_string(),
            intervention: Intervention::ReleaseHostQuarantine("example.com".to_string()),
        });
        assert!(!host.is_host_quarantined("example.com"));
    }

    #[test]
    fn stuck_target_observer_treats_no_result_output_as_failure() {
        let mut observer = StuckTargetObserver::default();
        let mut no_result =
            tool_context("web.search", ToolOutcomeStatus::Succeeded, &["example.com"]);
        no_result.observation_excerpt = "status=succeeded\noutput=\nNo results found".to_string();

        for _ in 0..3 {
            observer.after_tool_observation(&no_result);
        }
        assert_eq!(observer.failure_streak("example.com"), 3);
    }

    #[test]
    fn stuck_target_observer_ignores_other_tools_and_denied_calls() {
        let mut observer = StuckTargetObserver::default();
        let denied = tool_context("web.fetch", ToolOutcomeStatus::Denied, &["example.com"]);
        for _ in 0..4 {
            assert!(observer.after_tool_observation(&denied).is_empty());
        }
        assert_eq!(observer.failure_streak("example.com"), 0);

        let other_tool = tool_context("file.read", ToolOutcomeStatus::Failed, &["example.com"]);
        assert!(observer.after_tool_observation(&other_tool).is_empty());
        assert_eq!(observer.failure_streak("example.com"), 0);
    }

    #[test]
    fn finalization_observer_forces_the_final_turn_inside_the_reserve() {
        let mut observer = FinalizationObserver;
        let mut ctx = tool_context("file.read", ToolOutcomeStatus::Succeeded, &[]);
        ctx.remaining_model_calls = 5;
        ctx.terminal_model_call_reserve = 4;
        assert!(observer.before_model_turn(&ctx).is_empty());
        assert!(observer.after_tool_observation(&ctx).is_empty());

        ctx.remaining_model_calls = 4;
        assert_eq!(
            observer.before_model_turn(&ctx),
            vec![Intervention::ForceFinalTurn]
        );
        ctx.remaining_model_calls = 0;
        assert_eq!(
            observer.after_tool_observation(&ctx),
            vec![Intervention::ForceFinalTurn]
        );
    }

    #[test]
    fn doom_loop_observer_requests_confirmation_exactly_at_the_threshold() {
        let mut observer = DoomLoopObserver::default();
        let mut ctx = tool_context("file.read", ToolOutcomeStatus::Succeeded, &[]);
        ctx.consecutive_identical_calls = 3;
        assert!(observer.after_tool_observation(&ctx).is_empty());

        ctx.consecutive_identical_calls = 4;
        assert_eq!(
            observer.after_tool_observation(&ctx),
            vec![Intervention::RequestDoomLoopConfirmation {
                tool: "file.read".to_string()
            }]
        );

        ctx.consecutive_identical_calls = 5;
        assert!(
            observer.after_tool_observation(&ctx).is_empty(),
            "the request fires once per streak, at the threshold"
        );
    }

    #[test]
    fn loop_observers_apply_each_intervention_kind_to_loop_state() {
        let mut host = LoopObservers::with_registry(ObserverRegistry::default());
        host.apply_emission(&ObserverEmission {
            observer: "test".to_string(),
            intervention: Intervention::QuarantineHost("HTTPS-Host.EXAMPLE:8080".to_string()),
        });
        host.apply_emission(&ObserverEmission {
            observer: "test".to_string(),
            intervention: Intervention::ForceFinalTurn,
        });
        host.apply_emission(&ObserverEmission {
            observer: "test".to_string(),
            intervention: Intervention::RequestDoomLoopConfirmation {
                tool: "web.fetch".to_string(),
            },
        });
        host.apply_emission(&ObserverEmission {
            observer: "test".to_string(),
            intervention: Intervention::Hint("change approach".to_string()),
        });

        assert!(host.is_host_quarantined("https-host.example"));
        assert!(host.force_final_turn());
        assert_eq!(host.pending_doom_loop_confirmation(), Some("web.fetch"));
        let overlays = host.take_hint_overlays();
        assert_eq!(overlays.len(), 1);
        assert_eq!(overlays[0].content, "change approach");
        assert_eq!(
            overlays[0].metadata.get("kind").map(String::as_str),
            Some(LOOP_OBSERVER_HINT_KIND)
        );
        assert!(host.take_hint_overlays().is_empty());
    }

    #[test]
    fn host_extraction_and_normalization_are_deterministic() {
        assert_eq!(normalize_host("Example.COM:8080"), "example.com");
        assert_eq!(normalize_host("user@sub.Example.com"), "sub.example.com");
        assert_eq!(normalize_host("[::1]:443"), "::1");

        let hosts = http_hosts_for_tool_io(
            "web.fetch",
            r#"{"url":"https://Example.com:443/a?b=1"}"#,
            "",
        );
        assert_eq!(hosts, vec!["example.com".to_string()]);

        let hosts = http_hosts_for_tool_io(
            "web.search",
            r#"{"query":"site:Docs.Example.org incident report"}"#,
            "",
        );
        assert_eq!(hosts, vec!["docs.example.org".to_string()]);

        let hosts = http_hosts_for_tool_io(
            "web.search",
            r#"{"query":"incident report"}"#,
            "see https://status.example.net/incidents and https://status.example.net/history",
        );
        assert_eq!(hosts, vec!["status.example.net".to_string()]);
    }

    #[test]
    fn quarantined_web_target_blocks_only_quarantined_hosts() {
        let mut state = start_agent_loop(
            TaskId("stuck-target-block".to_string()),
            "fetch pages",
            AgentRuntimeConfig::default(),
        );
        assert_eq!(
            quarantined_web_target_host(&state, "web.fetch", r#"{"url":"https://example.com"}"#),
            None
        );

        state.loop_observers.quarantine_host("example.com");
        assert_eq!(
            quarantined_web_target_host(&state, "web.fetch", r#"{"url":"https://example.com/x"}"#),
            Some("example.com".to_string())
        );
        assert_eq!(
            quarantined_web_target_host(&state, "web.fetch", r#"{"url":"https://other.org"}"#),
            None
        );
        assert_eq!(
            quarantined_web_target_host(&state, "file.read", r#"{"path":"https://example.com"}"#),
            None,
            "non-web tools are never blocked by the stuck-target quarantine"
        );
    }

    #[test]
    fn blocked_observation_names_the_quarantined_host() {
        let observation = stuck_target_blocked_observation("example.com");
        assert!(observation.starts_with("stuck target blocked: example.com"));
    }

    #[test]
    fn default_registry_registers_the_standard_observers() {
        let host = LoopObservers::default();
        assert_eq!(
            host.observer_names(),
            vec![
                REPETITION_ADVISORY_KIND,
                "stuck_target",
                "finalization",
                "doom_loop",
                "adaptive_reasoning"
            ]
        );
        assert_eq!(host.remaining_model_calls(), usize::MAX);
        assert_eq!(host.terminal_model_call_reserve(), 4);
    }

    #[test]
    fn clear_doom_loop_confirmation_resets_streak_and_flag() {
        let mut state = start_agent_loop(
            TaskId("doom-reset".to_string()),
            "browse",
            AgentRuntimeConfig::default(),
        );
        state
            .repetition_advisory
            .observe("web.fetch", r#"{"url":"https://example.com"}"#);
        state
            .repetition_advisory
            .observe("web.fetch", r#"{"url":"https://example.com"}"#);
        state
            .loop_observers
            .request_doom_loop_confirmation("web.fetch".to_string());
        assert_eq!(state.repetition_advisory.streak(), 2);
        assert!(state
            .loop_observers
            .pending_doom_loop_confirmation()
            .is_some());

        clear_doom_loop_confirmation(&mut state);
        assert_eq!(state.repetition_advisory.streak(), 0);
        assert!(state
            .loop_observers
            .pending_doom_loop_confirmation()
            .is_none());
    }

    fn reasoning_context(cap: u32, failed: bool, identical: usize) -> ObserverContext {
        ObserverContext {
            tool_name: "shell.run".to_string(),
            input_fingerprint: "fp".to_string(),
            canonical_input: "{}".to_string(),
            outcome_status: Some(if failed {
                ToolOutcomeStatus::Failed
            } else {
                ToolOutcomeStatus::Succeeded
            }),
            http_hosts: Vec::new(),
            remaining_model_calls: usize::MAX,
            terminal_model_call_reserve: 4,
            consecutive_identical_calls: identical,
            reasoning_tier_max_budget: cap,
            observation_excerpt: String::new(),
        }
    }

    fn escalated_budgets(observer: &mut AdaptiveReasoningObserver, cap: u32, n: usize) -> Vec<u32> {
        let mut seen = Vec::new();
        for _ in 0..n {
            for intervention in observer.after_tool_observation(&reasoning_context(cap, true, 0)) {
                if let Intervention::SetThinkingBudget(budget) = intervention {
                    seen.push(budget);
                }
            }
        }
        seen
    }

    #[test]
    fn adaptive_reasoning_escalates_on_failures_and_stays_capped() {
        let mut observer = AdaptiveReasoningObserver::default();
        let cap = 4096u32;
        let seen = escalated_budgets(&mut observer, cap, 8);
        assert!(!seen.is_empty(), "failures should escalate the budget");
        assert!(seen.iter().all(|budget| *budget <= cap));
        assert_eq!(
            seen[seen.len() - 1],
            cap,
            "escalation saturates at the tier cap"
        );
    }

    #[test]
    fn adaptive_reasoning_backs_off_to_base_on_success() {
        let mut observer = AdaptiveReasoningObserver::default();
        let cap = 4096u32;
        escalated_budgets(&mut observer, cap, 4);
        assert!(observer.current_budget() > AdaptiveReasoningObserver::base_budget(cap));
        observer.after_tool_observation(&reasoning_context(cap, false, 0));
        assert_eq!(
            observer.current_budget(),
            AdaptiveReasoningObserver::base_budget(cap)
        );
    }

    #[test]
    fn adaptive_reasoning_is_inert_without_a_thinking_tier() {
        let mut observer = AdaptiveReasoningObserver::default();
        let interventions = observer.after_tool_observation(&reasoning_context(0, true, 0));
        assert!(
            interventions.is_empty(),
            "cap 0 means the tier does not think"
        );
    }

    #[test]
    fn leaked_tool_call_retry_is_one_shot() {
        let mut state = start_agent_loop(
            TaskId("leak".to_string()),
            "do the thing",
            AgentRuntimeConfig::default(),
        );
        let before = state.messages.len();
        let leaked = "I will now call {\"name\": \"file.read\", \"arguments\": {}} inline";
        assert!(consume_leaked_tool_call_retry(&mut state, leaked, before).is_some());
        assert!(state.loop_observers.leaked_retry_used());
        assert_eq!(
            state.messages.len(),
            before + 1,
            "repair instruction appended"
        );
        let after = state.messages.len();
        assert!(
            consume_leaked_tool_call_retry(&mut state, leaked, after).is_none(),
            "the allowance is spent after one retry"
        );
    }

    #[test]
    fn plain_answer_does_not_trigger_a_leaked_retry() {
        let mut state = start_agent_loop(
            TaskId("noleak".to_string()),
            "do the thing",
            AgentRuntimeConfig::default(),
        );
        let len = state.messages.len();
        assert!(consume_leaked_tool_call_retry(&mut state, "A plain final answer.", len).is_none());
    }

    #[test]
    fn overflow_compaction_archives_a_long_head_and_skips_short_transcripts() {
        use agent_core::{Message, MessageRole};
        let mut messages = vec![Message {
            role: MessageRole::User,
            content: "start".to_string(),
            metadata: Default::default(),
        }];
        for i in 0..60 {
            messages.push(Message {
                role: MessageRole::User,
                content: format!("question {i} {}", "x".repeat(400)),
                metadata: Default::default(),
            });
            messages.push(Message {
                role: MessageRole::Assistant,
                content: format!("answer {i} {}", "y".repeat(400)),
                metadata: Default::default(),
            });
        }
        let compacted = crate::compact_messages_for_overflow(&messages, 2_000);
        assert!(compacted.is_some(), "a long overflowing head must archive");
        assert!(compacted.unwrap().len() < messages.len());
        assert!(
            crate::compact_messages_for_overflow(&messages[..1], 100_000).is_none(),
            "a short transcript has no archivable head"
        );
    }
}
