use crate::result_frontier::{BestKnownResult, ResultFrontier, ResultQuality};
use crate::run_budget::{RunBudget, RunStageClass};
use crate::{AgentLoopState, AgentRuntimeConfig};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PARTIAL_OUTPUT_MAX_CHARS: usize = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStopReason {
    UserCancelled,
    DeadlineExceeded,
    ModelCallBudgetExceeded,
    ToolCallBudgetExceeded,
    TurnBudgetExhausted,
    ProviderUnavailable,
    NoProgress,
    RepeatedAction,
    StageBudgetExhausted,
    RepairBudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunContinuationDirective {
    Continue,
    CommitTerminalResult,
    Stop(RunStopReason),
}

impl RunStopReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::UserCancelled => "user_cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ModelCallBudgetExceeded => "model_call_budget_exceeded",
            Self::ToolCallBudgetExceeded => "tool_call_budget_exceeded",
            Self::TurnBudgetExhausted => "turn_budget_exhausted",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::NoProgress => "no_progress",
            Self::RepeatedAction => "repeated_action",
            Self::StageBudgetExhausted => "stage_budget_exhausted",
            Self::RepairBudgetExhausted => "repair_budget_exhausted",
        }
    }

    pub fn is_user_cancelled(self) -> bool {
        self == Self::UserCancelled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunStageUsageSnapshot {
    pub model_calls: usize,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSteer {
    pub queue_id: String,
}

#[derive(Debug, Clone)]
pub struct RunControlSnapshot {
    budget: RunBudget,
    elapsed_active: Duration,
    model_calls: usize,
    tool_calls: usize,
    agent_turns: usize,
    repair_attempts: usize,
    partial_output: String,
    action_history: BTreeMap<String, (u64, usize)>,
    recent_actions: BTreeMap<String, VecDeque<u64>>,
    observation_fingerprints: BTreeSet<u64>,
    observation_count: usize,
    checkpoint_fingerprints: BTreeSet<u64>,
    checkpoint_count: usize,
    model_extension_checkpoint: usize,
    tool_extension_checkpoint: usize,
    agent_turn_extension_checkpoint: usize,
    model_call_limit: usize,
    tool_call_limit: usize,
    agent_turn_limit: usize,
    budget_extensions: usize,
    stop_reason: Option<RunStopReason>,
    pending_steers: VecDeque<RunSteer>,
    stage_usage: BTreeMap<RunStageClass, RunStageUsageSnapshot>,
    best_known_result: Option<BestKnownResult>,
    result_frontier: Vec<BestKnownResult>,
}

#[derive(Debug, Clone)]
pub struct RunProgressSnapshot {
    pub stage: String,
    pub detail: String,
    pub elapsed: Duration,
    pub remaining: Duration,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub agent_turns: usize,
    pub repair_attempts: usize,
    pub model_call_limit: usize,
    pub tool_call_limit: usize,
    pub agent_turn_limit: usize,
    pub observations: usize,
    pub checkpoints: usize,
    pub budget_extensions: usize,
    pub stage_usage: BTreeMap<RunStageClass, RunStageUsageSnapshot>,
    pub best_known_result: Option<BestKnownResult>,
}

#[derive(Debug, Clone)]
struct RunStageUsage {
    started_at: Instant,
    model_calls: usize,
}

#[derive(Debug)]
struct RunMutableState {
    started_at: Instant,
    last_progress_at: Instant,
    stage: String,
    detail: String,
    partial_output: String,
    action_history: BTreeMap<String, (u64, usize)>,
    recent_actions: BTreeMap<String, VecDeque<u64>>,
    observation_fingerprints: BTreeSet<u64>,
    observation_count: usize,
    checkpoint_fingerprints: BTreeSet<u64>,
    checkpoint_count: usize,
    model_extension_checkpoint: usize,
    tool_extension_checkpoint: usize,
    agent_turn_extension_checkpoint: usize,
    model_call_limit: usize,
    tool_call_limit: usize,
    agent_turn_limit: usize,
    budget_extensions: usize,
    stop_reason: Option<RunStopReason>,
    active_model_calls: usize,
    active_tool_calls: usize,
    pending_steers: VecDeque<RunSteer>,
    stage_usage: BTreeMap<RunStageClass, RunStageUsage>,
    results: ResultFrontier,
}

#[derive(Debug)]
pub struct AgentRunControl {
    budget: RunBudget,
    user_cancelled: AtomicBool,
    model_calls: AtomicUsize,
    tool_calls: AtomicUsize,
    agent_turns: AtomicUsize,
    repair_attempts: AtomicUsize,
    state: Mutex<RunMutableState>,
}

impl AgentRunControl {
    pub fn new(effort: &str) -> Self {
        Self::with_budget(RunBudget::for_effort(effort))
    }

    pub fn with_budget(budget: RunBudget) -> Self {
        let now = Instant::now();
        Self {
            budget,
            user_cancelled: AtomicBool::new(false),
            model_calls: AtomicUsize::new(0),
            tool_calls: AtomicUsize::new(0),
            agent_turns: AtomicUsize::new(0),
            repair_attempts: AtomicUsize::new(0),
            state: Mutex::new(RunMutableState {
                started_at: now,
                last_progress_at: now,
                stage: "starting".to_string(),
                detail: String::new(),
                partial_output: String::new(),
                action_history: BTreeMap::new(),
                recent_actions: BTreeMap::new(),
                observation_fingerprints: BTreeSet::new(),
                observation_count: 0,
                checkpoint_fingerprints: BTreeSet::new(),
                checkpoint_count: 0,
                model_extension_checkpoint: 0,
                tool_extension_checkpoint: 0,
                agent_turn_extension_checkpoint: 0,
                model_call_limit: budget
                    .initial_model_calls
                    .min(budget.max_model_calls)
                    .max(1),
                tool_call_limit: budget.initial_tool_calls.min(budget.max_tool_calls).max(1),
                agent_turn_limit: budget
                    .initial_agent_turns
                    .min(budget.max_agent_turns)
                    .max(1),
                budget_extensions: 0,
                stop_reason: None,
                active_model_calls: 0,
                active_tool_calls: 0,
                pending_steers: VecDeque::new(),
                stage_usage: BTreeMap::new(),
                results: ResultFrontier::default(),
            }),
        }
    }

    pub fn from_snapshot(snapshot: RunControlSnapshot) -> Self {
        let now = Instant::now();
        let started_at = now.checked_sub(snapshot.elapsed_active).unwrap_or(now);
        Self {
            budget: snapshot.budget,
            user_cancelled: AtomicBool::new(false),
            model_calls: AtomicUsize::new(snapshot.model_calls),
            tool_calls: AtomicUsize::new(snapshot.tool_calls),
            agent_turns: AtomicUsize::new(snapshot.agent_turns),
            repair_attempts: AtomicUsize::new(snapshot.repair_attempts),
            state: Mutex::new(RunMutableState {
                started_at,
                last_progress_at: now,
                stage: "resuming".to_string(),
                detail: String::new(),
                partial_output: snapshot.partial_output,
                action_history: snapshot.action_history,
                recent_actions: snapshot.recent_actions,
                observation_fingerprints: snapshot.observation_fingerprints,
                observation_count: snapshot.observation_count,
                checkpoint_fingerprints: snapshot.checkpoint_fingerprints,
                checkpoint_count: snapshot.checkpoint_count,
                model_extension_checkpoint: snapshot.model_extension_checkpoint,
                tool_extension_checkpoint: snapshot.tool_extension_checkpoint,
                agent_turn_extension_checkpoint: snapshot.agent_turn_extension_checkpoint,
                model_call_limit: snapshot.model_call_limit,
                tool_call_limit: snapshot.tool_call_limit,
                agent_turn_limit: snapshot.agent_turn_limit,
                budget_extensions: snapshot.budget_extensions,
                stop_reason: snapshot.stop_reason,
                active_model_calls: 0,
                active_tool_calls: 0,
                pending_steers: snapshot.pending_steers,
                stage_usage: restore_stage_usage(snapshot.stage_usage, now),
                results: ResultFrontier::restore(
                    snapshot.best_known_result,
                    snapshot.result_frontier,
                ),
            }),
        }
    }

    pub fn from_snapshot_for_continuation(
        snapshot: RunControlSnapshot,
    ) -> Result<Self, RunStopReason> {
        match snapshot.stop_reason {
            Some(RunStopReason::UserCancelled) => Err(RunStopReason::UserCancelled),
            None => Ok(Self::from_snapshot(snapshot)),
            Some(_) => {
                let now = Instant::now();
                let budget = snapshot.budget;
                Ok(Self {
                    budget,
                    user_cancelled: AtomicBool::new(false),
                    model_calls: AtomicUsize::new(0),
                    tool_calls: AtomicUsize::new(0),
                    agent_turns: AtomicUsize::new(0),
                    repair_attempts: AtomicUsize::new(0),
                    state: Mutex::new(RunMutableState {
                        started_at: now,
                        last_progress_at: now,
                        stage: "continuing".to_string(),
                        detail: String::new(),
                        partial_output: snapshot.partial_output,
                        action_history: snapshot.action_history,
                        recent_actions: snapshot.recent_actions,
                        observation_fingerprints: snapshot.observation_fingerprints,
                        observation_count: snapshot.observation_count,
                        checkpoint_fingerprints: snapshot.checkpoint_fingerprints,
                        checkpoint_count: snapshot.checkpoint_count,
                        model_extension_checkpoint: snapshot.checkpoint_count,
                        tool_extension_checkpoint: snapshot.checkpoint_count,
                        agent_turn_extension_checkpoint: snapshot.checkpoint_count,
                        model_call_limit: budget
                            .initial_model_calls
                            .min(budget.max_model_calls)
                            .max(1),
                        tool_call_limit: budget
                            .initial_tool_calls
                            .min(budget.max_tool_calls)
                            .max(1),
                        agent_turn_limit: budget
                            .initial_agent_turns
                            .min(budget.max_agent_turns)
                            .max(1),
                        budget_extensions: snapshot.budget_extensions,
                        stop_reason: None,
                        active_model_calls: 0,
                        active_tool_calls: 0,
                        pending_steers: snapshot.pending_steers,
                        stage_usage: BTreeMap::new(),
                        results: ResultFrontier::restore(
                            snapshot.best_known_result,
                            snapshot.result_frontier,
                        ),
                    }),
                })
            }
        }
    }

    pub fn snapshot(&self) -> RunControlSnapshot {
        let state = self.state.lock().expect("run control state poisoned");
        RunControlSnapshot {
            budget: self.budget,
            elapsed_active: state.started_at.elapsed().min(self.budget.max_duration),
            model_calls: self.model_calls.load(Ordering::SeqCst),
            tool_calls: self.tool_calls.load(Ordering::SeqCst),
            agent_turns: self.agent_turns.load(Ordering::SeqCst),
            repair_attempts: self.repair_attempts.load(Ordering::SeqCst),
            partial_output: state.partial_output.clone(),
            action_history: state.action_history.clone(),
            recent_actions: state.recent_actions.clone(),
            observation_fingerprints: state.observation_fingerprints.clone(),
            observation_count: state.observation_count,
            checkpoint_fingerprints: state.checkpoint_fingerprints.clone(),
            checkpoint_count: state.checkpoint_count,
            model_extension_checkpoint: state.model_extension_checkpoint,
            tool_extension_checkpoint: state.tool_extension_checkpoint,
            agent_turn_extension_checkpoint: state.agent_turn_extension_checkpoint,
            model_call_limit: state.model_call_limit,
            tool_call_limit: state.tool_call_limit,
            agent_turn_limit: state.agent_turn_limit,
            budget_extensions: state.budget_extensions,
            stop_reason: state.stop_reason,
            pending_steers: state.pending_steers.clone(),
            stage_usage: snapshot_stage_usage(&state.stage_usage),
            best_known_result: state.results.best_known(),
            result_frontier: state.results.candidates(),
        }
    }

    pub fn request_cancel(&self) {
        self.user_cancelled.store(true, Ordering::SeqCst);
        self.set_stop_reason(RunStopReason::UserCancelled);
    }

    pub fn request_stop(&self, reason: RunStopReason) {
        self.set_stop_reason(reason);
    }

    pub fn stop_reason(&self) -> Option<RunStopReason> {
        if self.user_cancelled.load(Ordering::SeqCst) {
            self.set_stop_reason(RunStopReason::UserCancelled);
        }
        let now = Instant::now();
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.stop_reason.is_none() {
            if now.duration_since(state.started_at) >= self.budget.max_duration {
                state.stop_reason = Some(RunStopReason::DeadlineExceeded);
            } else if now.duration_since(state.last_progress_at)
                >= if state.active_model_calls > 0 {
                    self.budget.model_call_timeout
                } else if state.active_tool_calls > 0 {
                    self.budget.tool_call_timeout
                } else {
                    self.budget.no_progress_timeout
                }
            {
                state.stop_reason = Some(RunStopReason::NoProgress);
            }
        }
        state.stop_reason
    }

    pub fn should_stop(&self) -> bool {
        self.stop_reason().is_some()
    }

    pub fn continuation_directive(&self) -> RunContinuationDirective {
        if let Some(reason) = self.stop_reason() {
            return RunContinuationDirective::Stop(reason);
        }
        let state = self.state.lock().expect("run control state poisoned");
        let model_calls = self.model_calls.load(Ordering::SeqCst);
        let agent_turns = self.agent_turns.load(Ordering::SeqCst);
        let remaining_calls = state.model_call_limit.saturating_sub(model_calls);
        let remaining_turns = state.agent_turn_limit.saturating_sub(agent_turns);
        let turn_extension_available = state.agent_turn_limit < self.budget.max_agent_turns
            && state.checkpoint_count > state.agent_turn_extension_checkpoint;
        let remaining_time = self
            .budget
            .max_duration
            .saturating_sub(state.started_at.elapsed());
        if remaining_calls <= self.budget.terminal_model_call_reserve
            || (remaining_turns <= 1 && !turn_extension_available)
            || remaining_time <= self.budget.terminal_time_reserve
        {
            RunContinuationDirective::CommitTerminalResult
        } else {
            RunContinuationDirective::Continue
        }
    }

    pub fn begin_model_call(&self, stage: &str) -> Result<usize, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let call = self.model_calls.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut state = self.state.lock().expect("run control state poisoned");
            if call > state.model_call_limit
                && !extend_model_budget_if_progressed(&self.budget, &mut state, call)
            {
                self.model_calls.fetch_sub(1, Ordering::SeqCst);
                state.stop_reason = Some(RunStopReason::ModelCallBudgetExceeded);
                return Err(RunStopReason::ModelCallBudgetExceeded);
            }
            state.active_model_calls = state.active_model_calls.saturating_add(1);
            state.stage = stage.to_string();
            state.detail = "model request started".to_string();
            state.last_progress_at = Instant::now();
        }
        Ok(call)
    }

    /// Starts a model call owned by a bounded collaboration stage. Exhausting
    /// this local allocation never stops the parent run, so a slow candidate or
    /// repair branch cannot consume the final reviewer/synthesizer reserve.
    pub fn begin_stage_model_call(
        &self,
        stage: &str,
        class: RunStageClass,
    ) -> Result<usize, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let stage_budget = self.budget.stage_budget(class);
        {
            let now = Instant::now();
            let mut state = self.state.lock().expect("run control state poisoned");
            let remaining = self
                .budget
                .max_duration
                .saturating_sub(now.duration_since(state.started_at));
            let protected_time = self.budget.protected_time_reserve(class);
            let protected_calls = self.budget.protected_model_call_reserve(class);
            if (protected_time > Duration::ZERO && remaining <= protected_time)
                || (protected_calls > 0
                    && self.model_calls.load(Ordering::SeqCst)
                        >= state
                            .model_call_limit
                            .saturating_sub(protected_calls)
                            .max(1))
            {
                return Err(RunStopReason::StageBudgetExhausted);
            }
            let usage = state.stage_usage.entry(class).or_insert(RunStageUsage {
                started_at: now,
                model_calls: 0,
            });
            if usage.model_calls >= stage_budget.max_model_calls
                || now.duration_since(usage.started_at) >= stage_budget.max_duration
            {
                return Err(RunStopReason::StageBudgetExhausted);
            }
            usage.model_calls = usage.model_calls.saturating_add(1);
        }

        match self.begin_model_call(stage) {
            Ok(call) => Ok(call),
            Err(reason) => {
                let mut state = self.state.lock().expect("run control state poisoned");
                if let Some(usage) = state.stage_usage.get_mut(&class) {
                    usage.model_calls = usage.model_calls.saturating_sub(1);
                }
                Err(reason)
            }
        }
    }

    pub fn stage_should_stop(&self, class: RunStageClass) -> bool {
        if self.should_stop() {
            return true;
        }
        let stage_budget = self.budget.stage_budget(class);
        let state = self.state.lock().expect("run control state poisoned");
        let remaining = self
            .budget
            .max_duration
            .saturating_sub(state.started_at.elapsed());
        let protected_time = self.budget.protected_time_reserve(class);
        (protected_time > Duration::ZERO && remaining <= protected_time)
            || state
                .stage_usage
                .get(&class)
                .is_some_and(|usage| usage.started_at.elapsed() >= stage_budget.max_duration)
    }

    pub fn record_agent_turn(&self, stage: &str) -> Result<usize, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let turn = self.agent_turns.fetch_add(1, Ordering::SeqCst) + 1;
        let mut state = self.state.lock().expect("run control state poisoned");
        if turn > state.agent_turn_limit
            && !extend_agent_turn_budget_if_progressed(&self.budget, &mut state, turn)
        {
            self.agent_turns.fetch_sub(1, Ordering::SeqCst);
            state.stop_reason = Some(RunStopReason::TurnBudgetExhausted);
            return Err(RunStopReason::TurnBudgetExhausted);
        }
        state.stage = stage.to_string();
        state.detail = "agent turn completed".to_string();
        state.last_progress_at = Instant::now();
        Ok(turn)
    }

    /// Counts a bounded repair attempt without poisoning the whole run when a
    /// local recovery strategy has exhausted its own allowance.
    pub fn begin_repair_attempt(&self, stage: &str) -> Result<usize, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let attempt = self.repair_attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt > self.budget.max_repair_attempts {
            self.repair_attempts.fetch_sub(1, Ordering::SeqCst);
            return Err(RunStopReason::RepairBudgetExhausted);
        }
        let mut state = self.state.lock().expect("run control state poisoned");
        state.stage = stage.to_string();
        state.detail = "repair attempt started".to_string();
        state.last_progress_at = Instant::now();
        Ok(attempt)
    }

    pub fn finish_model_call(&self) {
        let mut state = self.state.lock().expect("run control state poisoned");
        state.active_model_calls = state.active_model_calls.saturating_sub(1);
        state.last_progress_at = Instant::now();
    }

    pub fn begin_tool_call(
        &self,
        scope: &str,
        tool_name: &str,
        input: &str,
    ) -> Result<usize, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let call = self.tool_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let signature = fingerprint(&(tool_name, input));
        let mut state = self.state.lock().expect("run control state poisoned");
        if call > state.tool_call_limit
            && !extend_tool_budget_if_progressed(&self.budget, &mut state, call)
        {
            self.tool_calls.fetch_sub(1, Ordering::SeqCst);
            state.stop_reason = Some(RunStopReason::ToolCallBudgetExceeded);
            return Err(RunStopReason::ToolCallBudgetExceeded);
        }
        let history = state
            .action_history
            .entry(scope.to_string())
            .or_insert((signature, 0));
        if history.0 == signature {
            history.1 += 1;
        } else {
            *history = (signature, 1);
        }
        if history.1 > self.budget.max_identical_actions {
            self.tool_calls.fetch_sub(1, Ordering::SeqCst);
            state.stop_reason = Some(RunStopReason::RepeatedAction);
            return Err(RunStopReason::RepeatedAction);
        }
        let recent = state.recent_actions.entry(scope.to_string()).or_default();
        recent.push_back(signature);
        while recent.len() > 24 {
            recent.pop_front();
        }
        if has_repeated_action_cycle(recent, self.budget.max_identical_actions + 1) {
            self.tool_calls.fetch_sub(1, Ordering::SeqCst);
            state.stop_reason = Some(RunStopReason::RepeatedAction);
            return Err(RunStopReason::RepeatedAction);
        }
        state.active_tool_calls = state.active_tool_calls.saturating_add(1);
        state.stage = "tool".to_string();
        state.detail = tool_name.to_string();
        state.last_progress_at = Instant::now();
        Ok(call)
    }

    pub fn finish_tool_call(&self) {
        let mut state = self.state.lock().expect("run control state poisoned");
        state.active_tool_calls = state.active_tool_calls.saturating_sub(1);
        state.last_progress_at = Instant::now();
    }

    pub fn request_steer(&self, queue_id: impl Into<String>) -> Result<bool, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let queue_id = queue_id.into();
        let mut state = self.state.lock().expect("run control state poisoned");
        if state
            .pending_steers
            .iter()
            .any(|pending| pending.queue_id == queue_id)
        {
            return Ok(false);
        }
        if state.pending_steers.len() >= 16 {
            state.pending_steers.pop_front();
        }
        state.pending_steers.push_back(RunSteer { queue_id });
        state.stage = "steering".to_string();
        state.detail = "Applying user guidance".to_string();
        state.last_progress_at = Instant::now();
        Ok(true)
    }

    pub fn has_pending_steer(&self) -> bool {
        !self
            .state
            .lock()
            .expect("run control state poisoned")
            .pending_steers
            .is_empty()
    }

    pub fn take_pending_steers(&self) -> Vec<RunSteer> {
        self.state
            .lock()
            .expect("run control state poisoned")
            .pending_steers
            .drain(..)
            .collect()
    }

    pub fn mark_progress(&self, stage: &str, detail: &str) {
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.stop_reason.is_some() {
            return;
        }
        state.stage = stage.to_string();
        state.detail = detail.to_string();
        state.last_progress_at = Instant::now();
    }

    pub fn record_partial_output(&self, output: &str) {
        let output = output.trim();
        if output.is_empty() {
            return;
        }
        let partial_output = output
            .char_indices()
            .rev()
            .nth(PARTIAL_OUTPUT_MAX_CHARS.saturating_sub(1))
            .map(|(start, _)| output[start..].to_string())
            .unwrap_or_else(|| output.to_string());
        let mut state = self.state.lock().expect("run control state poisoned");
        state.partial_output = partial_output;
        state.last_progress_at = Instant::now();
    }

    pub fn record_best_known_result(
        &self,
        stage: &str,
        content: &str,
        quality: ResultQuality,
        evidence_count: usize,
        verified: bool,
        deliverable: bool,
    ) -> bool {
        let mut state = self.state.lock().expect("run control state poisoned");
        let should_replace = state.results.record(
            stage,
            content,
            quality,
            evidence_count,
            verified,
            deliverable,
        );
        if should_replace {
            state.last_progress_at = Instant::now();
        }
        should_replace
    }

    pub fn best_known_result(&self) -> Option<BestKnownResult> {
        self.state
            .lock()
            .expect("run control state poisoned")
            .results
            .best_known()
    }

    pub fn best_guidance_result(&self) -> Option<BestKnownResult> {
        self.state
            .lock()
            .expect("run control state poisoned")
            .results
            .best_guidance()
    }

    pub fn result_frontier(&self) -> Vec<BestKnownResult> {
        self.state
            .lock()
            .expect("run control state poisoned")
            .results
            .candidates()
    }

    /// Records distinct model or protocol output for liveness and diagnostics.
    /// Observations deliberately do not unlock more run budget; only verified
    /// material checkpoints may extend a bounded segment.
    pub fn record_observation(&self, stage: &str, detail: &str, evidence: &str) -> bool {
        let evidence = fingerprint(&(stage, evidence));
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.stop_reason.is_some() || !state.observation_fingerprints.insert(evidence) {
            return false;
        }
        state.observation_count = state.observation_count.saturating_add(1);
        state.stage = stage.to_string();
        state.detail = detail.to_string();
        state.last_progress_at = Instant::now();
        true
    }

    /// Records objective progress such as applied user steering, acquired tool
    /// evidence, workspace mutation, verification, or a completed evaluation.
    pub fn record_checkpoint(&self, stage: &str, detail: &str, evidence: &str) -> bool {
        let evidence = fingerprint(&(stage, evidence));
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.stop_reason.is_some() || !state.checkpoint_fingerprints.insert(evidence) {
            return false;
        }
        state.checkpoint_count = state.checkpoint_count.saturating_add(1);
        state.stage = stage.to_string();
        state.detail = detail.to_string();
        state.last_progress_at = Instant::now();
        true
    }

    pub fn partial_output(&self) -> String {
        self.state
            .lock()
            .expect("run control state poisoned")
            .partial_output
            .clone()
    }

    pub fn timeout_seconds(&self, cap_seconds: u64) -> u64 {
        let state = self.state.lock().expect("run control state poisoned");
        let remaining = self
            .budget
            .max_duration
            .saturating_sub(state.started_at.elapsed());
        remaining.as_secs().max(1).min(cap_seconds.max(1))
    }

    pub fn model_call_timeout_seconds(&self) -> u64 {
        self.timeout_seconds(self.budget.model_call_timeout.as_secs().max(1))
    }

    pub fn stage_model_call_timeout(&self, class: RunStageClass) -> Duration {
        let now = Instant::now();
        let state = self.state.lock().expect("run control state poisoned");
        let global_remaining = self
            .budget
            .max_duration
            .saturating_sub(now.duration_since(state.started_at));
        let stage_budget = self.budget.stage_budget(class);
        let stage_remaining = state
            .stage_usage
            .get(&class)
            .map(|usage| {
                stage_budget
                    .max_duration
                    .saturating_sub(now.duration_since(usage.started_at))
            })
            .unwrap_or(stage_budget.max_duration);
        let global_stage_allowance =
            global_remaining.saturating_sub(self.budget.protected_time_reserve(class));

        self.budget
            .model_call_timeout
            .min(stage_remaining)
            .min(global_stage_allowance)
    }

    pub fn stage_model_call_timeout_seconds(&self, class: RunStageClass) -> u64 {
        self.stage_model_call_timeout(class).as_secs().max(1)
    }

    pub fn stage_model_call_timeout_with_recovery(
        &self,
        class: RunStageClass,
        recovery_windows: usize,
        minimum_recovery_window: Duration,
    ) -> Duration {
        let available = self.stage_model_call_timeout(class);
        if recovery_windows == 0 || minimum_recovery_window.is_zero() {
            return available;
        }
        let reserved = minimum_recovery_window
            .checked_mul(u32::try_from(recovery_windows).unwrap_or(u32::MAX))
            .unwrap_or(available)
            .min(available.saturating_sub(Duration::from_secs(1)));
        available.saturating_sub(reserved)
    }

    pub fn progress(&self) -> RunProgressSnapshot {
        let state = self.state.lock().expect("run control state poisoned");
        let elapsed = state.started_at.elapsed().min(self.budget.max_duration);
        RunProgressSnapshot {
            stage: state.stage.clone(),
            detail: state.detail.clone(),
            elapsed,
            remaining: self.budget.max_duration.saturating_sub(elapsed),
            model_calls: self.model_calls.load(Ordering::SeqCst),
            tool_calls: self.tool_calls.load(Ordering::SeqCst),
            agent_turns: self.agent_turns.load(Ordering::SeqCst),
            repair_attempts: self.repair_attempts.load(Ordering::SeqCst),
            model_call_limit: state.model_call_limit,
            tool_call_limit: state.tool_call_limit,
            agent_turn_limit: state.agent_turn_limit,
            observations: state.observation_count,
            checkpoints: state.checkpoint_count,
            budget_extensions: state.budget_extensions,
            stage_usage: snapshot_stage_usage(&state.stage_usage),
            best_known_result: state.results.best_known(),
        }
    }

    pub fn budget(&self) -> RunBudget {
        self.budget
    }

    pub fn runtime_config(&self) -> AgentRuntimeConfig {
        AgentRuntimeConfig {
            max_turns: self.budget.max_agent_turns.max(1),
        }
    }

    pub fn extend_runtime_budget(&self, runtime: &mut AgentLoopState) {
        runtime.max_turns = runtime
            .turn
            .saturating_add(self.budget.max_agent_turns.max(1));
    }

    fn set_stop_reason(&self, reason: RunStopReason) {
        let mut state = self.state.lock().expect("run control state poisoned");
        state.stop_reason.get_or_insert(reason);
    }
}

fn fingerprint<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn extend_model_budget_if_progressed(
    budget: &RunBudget,
    state: &mut RunMutableState,
    requested_call: usize,
) -> bool {
    if requested_call > budget.max_model_calls
        || state.checkpoint_count <= state.model_extension_checkpoint
    {
        return false;
    }
    state.model_call_limit = state
        .model_call_limit
        .saturating_add(budget.model_calls_per_extension.max(1))
        .min(budget.max_model_calls);
    state.model_extension_checkpoint = state.checkpoint_count;
    state.budget_extensions = state.budget_extensions.saturating_add(1);
    requested_call <= state.model_call_limit
}

fn extend_tool_budget_if_progressed(
    budget: &RunBudget,
    state: &mut RunMutableState,
    requested_call: usize,
) -> bool {
    if requested_call > budget.max_tool_calls
        || state.checkpoint_count <= state.tool_extension_checkpoint
    {
        return false;
    }
    state.tool_call_limit = state
        .tool_call_limit
        .saturating_add(budget.tool_calls_per_extension.max(1))
        .min(budget.max_tool_calls);
    state.tool_extension_checkpoint = state.checkpoint_count;
    state.budget_extensions = state.budget_extensions.saturating_add(1);
    requested_call <= state.tool_call_limit
}

fn extend_agent_turn_budget_if_progressed(
    budget: &RunBudget,
    state: &mut RunMutableState,
    requested_turn: usize,
) -> bool {
    if requested_turn > budget.max_agent_turns
        || state.checkpoint_count <= state.agent_turn_extension_checkpoint
    {
        return false;
    }
    state.agent_turn_limit = state
        .agent_turn_limit
        .saturating_add(budget.agent_turns_per_extension.max(1))
        .min(budget.max_agent_turns);
    state.agent_turn_extension_checkpoint = state.checkpoint_count;
    state.budget_extensions = state.budget_extensions.saturating_add(1);
    requested_turn <= state.agent_turn_limit
}

fn snapshot_stage_usage(
    usage: &BTreeMap<RunStageClass, RunStageUsage>,
) -> BTreeMap<RunStageClass, RunStageUsageSnapshot> {
    usage
        .iter()
        .map(|(class, usage)| {
            (
                *class,
                RunStageUsageSnapshot {
                    model_calls: usage.model_calls,
                    elapsed: usage.started_at.elapsed(),
                },
            )
        })
        .collect()
}

fn restore_stage_usage(
    usage: BTreeMap<RunStageClass, RunStageUsageSnapshot>,
    now: Instant,
) -> BTreeMap<RunStageClass, RunStageUsage> {
    usage
        .into_iter()
        .map(|(class, usage)| {
            (
                class,
                RunStageUsage {
                    started_at: now.checked_sub(usage.elapsed).unwrap_or(now),
                    model_calls: usage.model_calls,
                },
            )
        })
        .collect()
}

fn has_repeated_action_cycle(actions: &VecDeque<u64>, repetitions: usize) -> bool {
    let repetitions = repetitions.max(2);
    (1..=3).any(|period| {
        let required = period * repetitions;
        if actions.len() < required {
            return false;
        }
        let start = actions.len() - required;
        (period..required)
            .all(|offset| actions.get(start + offset) == actions.get(start + (offset % period)))
    })
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
