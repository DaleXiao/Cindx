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
        }
    }

    pub fn is_user_cancelled(self) -> bool {
        self == Self::UserCancelled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBudget {
    pub max_duration: Duration,
    pub model_call_timeout: Duration,
    pub initial_model_calls: usize,
    pub max_model_calls: usize,
    pub model_calls_per_extension: usize,
    pub initial_tool_calls: usize,
    pub max_tool_calls: usize,
    pub tool_calls_per_extension: usize,
    pub no_progress_timeout: Duration,
    pub max_identical_actions: usize,
}

impl RunBudget {
    pub fn for_effort(effort: &str) -> Self {
        match effort {
            "fast" => Self {
                max_duration: Duration::from_secs(5 * 60),
                model_call_timeout: Duration::from_secs(3 * 60),
                initial_model_calls: 6,
                max_model_calls: 12,
                model_calls_per_extension: 3,
                initial_tool_calls: 12,
                max_tool_calls: 24,
                tool_calls_per_extension: 6,
                no_progress_timeout: Duration::from_secs(90),
                max_identical_actions: 3,
            },
            "pro" => Self {
                max_duration: Duration::from_secs(4 * 60 * 60),
                model_call_timeout: Duration::from_secs(15 * 60),
                initial_model_calls: 48,
                max_model_calls: 384,
                model_calls_per_extension: 48,
                initial_tool_calls: 96,
                max_tool_calls: 768,
                tool_calls_per_extension: 96,
                no_progress_timeout: Duration::from_secs(5 * 60),
                max_identical_actions: 4,
            },
            _ => Self {
                max_duration: Duration::from_secs(45 * 60),
                model_call_timeout: Duration::from_secs(5 * 60),
                initial_model_calls: 18,
                max_model_calls: 72,
                model_calls_per_extension: 18,
                initial_tool_calls: 36,
                max_tool_calls: 144,
                tool_calls_per_extension: 36,
                no_progress_timeout: Duration::from_secs(2 * 60),
                max_identical_actions: 3,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunControlSnapshot {
    budget: RunBudget,
    elapsed_active: Duration,
    model_calls: usize,
    tool_calls: usize,
    partial_output: String,
    action_history: BTreeMap<String, (u64, usize)>,
    recent_actions: BTreeMap<String, VecDeque<u64>>,
    checkpoint_fingerprints: BTreeSet<u64>,
    checkpoint_count: usize,
    model_extension_checkpoint: usize,
    tool_extension_checkpoint: usize,
    model_call_limit: usize,
    tool_call_limit: usize,
    budget_extensions: usize,
}

#[derive(Debug, Clone)]
pub struct RunProgressSnapshot {
    pub stage: String,
    pub detail: String,
    pub elapsed: Duration,
    pub remaining: Duration,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub model_call_limit: usize,
    pub tool_call_limit: usize,
    pub checkpoints: usize,
    pub budget_extensions: usize,
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
    checkpoint_fingerprints: BTreeSet<u64>,
    checkpoint_count: usize,
    model_extension_checkpoint: usize,
    tool_extension_checkpoint: usize,
    model_call_limit: usize,
    tool_call_limit: usize,
    budget_extensions: usize,
    stop_reason: Option<RunStopReason>,
    active_model_calls: usize,
}

#[derive(Debug)]
pub struct AgentRunControl {
    budget: RunBudget,
    user_cancelled: AtomicBool,
    model_calls: AtomicUsize,
    tool_calls: AtomicUsize,
    state: Mutex<RunMutableState>,
}

impl AgentRunControl {
    pub fn new(effort: &str) -> Self {
        Self::with_budget(RunBudget::for_effort(effort))
    }

    fn with_budget(budget: RunBudget) -> Self {
        let now = Instant::now();
        Self {
            budget,
            user_cancelled: AtomicBool::new(false),
            model_calls: AtomicUsize::new(0),
            tool_calls: AtomicUsize::new(0),
            state: Mutex::new(RunMutableState {
                started_at: now,
                last_progress_at: now,
                stage: "starting".to_string(),
                detail: String::new(),
                partial_output: String::new(),
                action_history: BTreeMap::new(),
                recent_actions: BTreeMap::new(),
                checkpoint_fingerprints: BTreeSet::new(),
                checkpoint_count: 0,
                model_extension_checkpoint: 0,
                tool_extension_checkpoint: 0,
                model_call_limit: budget.initial_model_calls.min(budget.max_model_calls).max(1),
                tool_call_limit: budget.initial_tool_calls.min(budget.max_tool_calls).max(1),
                budget_extensions: 0,
                stop_reason: None,
                active_model_calls: 0,
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
            state: Mutex::new(RunMutableState {
                started_at,
                last_progress_at: now,
                stage: "resuming".to_string(),
                detail: String::new(),
                partial_output: snapshot.partial_output,
                action_history: snapshot.action_history,
                recent_actions: snapshot.recent_actions,
                checkpoint_fingerprints: snapshot.checkpoint_fingerprints,
                checkpoint_count: snapshot.checkpoint_count,
                model_extension_checkpoint: snapshot.model_extension_checkpoint,
                tool_extension_checkpoint: snapshot.tool_extension_checkpoint,
                model_call_limit: snapshot.model_call_limit,
                tool_call_limit: snapshot.tool_call_limit,
                budget_extensions: snapshot.budget_extensions,
                stop_reason: None,
                active_model_calls: 0,
            }),
        }
    }

    pub fn snapshot(&self) -> RunControlSnapshot {
        let state = self.state.lock().expect("run control state poisoned");
        RunControlSnapshot {
            budget: self.budget,
            elapsed_active: state.started_at.elapsed().min(self.budget.max_duration),
            model_calls: self.model_calls.load(Ordering::SeqCst),
            tool_calls: self.tool_calls.load(Ordering::SeqCst),
            partial_output: state.partial_output.clone(),
            action_history: state.action_history.clone(),
            recent_actions: state.recent_actions.clone(),
            checkpoint_fingerprints: state.checkpoint_fingerprints.clone(),
            checkpoint_count: state.checkpoint_count,
            model_extension_checkpoint: state.model_extension_checkpoint,
            tool_extension_checkpoint: state.tool_extension_checkpoint,
            model_call_limit: state.model_call_limit,
            tool_call_limit: state.tool_call_limit,
            budget_extensions: state.budget_extensions,
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
                    self.budget
                        .model_call_timeout
                        .max(self.budget.no_progress_timeout)
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
            state.stop_reason = Some(RunStopReason::RepeatedAction);
            return Err(RunStopReason::RepeatedAction);
        }
        let recent = state.recent_actions.entry(scope.to_string()).or_default();
        recent.push_back(signature);
        while recent.len() > 24 {
            recent.pop_front();
        }
        if has_repeated_action_cycle(recent, self.budget.max_identical_actions + 1) {
            state.stop_reason = Some(RunStopReason::RepeatedAction);
            return Err(RunStopReason::RepeatedAction);
        }
        state.stage = "tool".to_string();
        state.detail = tool_name.to_string();
        state.last_progress_at = Instant::now();
        Ok(call)
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
            .nth(PARTIAL_OUTPUT_MAX_CHARS)
            .map(|(end, _)| output[..end].to_string())
            .unwrap_or_else(|| output.to_string());
        let mut state = self.state.lock().expect("run control state poisoned");
        state.partial_output = partial_output;
        state.last_progress_at = Instant::now();
    }

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
            model_call_limit: state.model_call_limit,
            tool_call_limit: state.tool_call_limit,
            checkpoints: state.checkpoint_count,
            budget_extensions: state.budget_extensions,
        }
    }

    pub fn budget(&self) -> RunBudget {
        self.budget
    }

    pub fn runtime_config(&self) -> AgentRuntimeConfig {
        AgentRuntimeConfig {
            max_turns: self.budget.max_model_calls.max(1),
        }
    }

    pub fn extend_runtime_budget(&self, runtime: &mut AgentLoopState) {
        runtime.max_turns = runtime
            .turn
            .saturating_add(self.budget.max_model_calls.max(1));
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

fn has_repeated_action_cycle(actions: &VecDeque<u64>, repetitions: usize) -> bool {
    let repetitions = repetitions.max(2);
    (1..=3).any(|period| {
        let required = period * repetitions;
        if actions.len() < required {
            return false;
        }
        let start = actions.len() - required;
        (period..required).all(|offset| {
            actions.get(start + offset) == actions.get(start + (offset % period))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn test_budget() -> RunBudget {
        RunBudget {
            max_duration: Duration::from_millis(60),
            model_call_timeout: Duration::from_millis(40),
            initial_model_calls: 2,
            max_model_calls: 2,
            model_calls_per_extension: 1,
            initial_tool_calls: 2,
            max_tool_calls: 2,
            tool_calls_per_extension: 1,
            no_progress_timeout: Duration::from_millis(25),
            max_identical_actions: 2,
        }
    }

    #[test]
    fn pro_budget_allows_a_long_running_segment() {
        let budget = RunBudget::for_effort("pro");
        assert_eq!(budget.max_duration, Duration::from_secs(4 * 60 * 60));
        assert_eq!(budget.model_call_timeout, Duration::from_secs(15 * 60));
        assert_eq!(budget.initial_model_calls, 48);
        assert_eq!(budget.max_model_calls, 384);
        assert_eq!(budget.initial_tool_calls, 96);
        assert_eq!(budget.max_tool_calls, 768);
        assert_eq!(budget.no_progress_timeout, Duration::from_secs(5 * 60));
    }

    #[test]
    fn runtime_turn_budget_comes_from_the_same_control_budget() {
        let control = AgentRunControl::new("pro");
        assert_eq!(control.runtime_config().max_turns, 384);

        let mut runtime = crate::start_agent_loop(
            agent_core::TaskId("resume".to_string()),
            "continue",
            AgentRuntimeConfig { max_turns: 6 },
        );
        runtime.turn = 23;
        control.extend_runtime_budget(&mut runtime);
        assert_eq!(runtime.max_turns, 407);
    }

    #[test]
    fn unique_checkpoints_extend_a_segment_but_chatter_does_not() {
        let mut budget = test_budget();
        budget.initial_model_calls = 1;
        budget.max_model_calls = 3;
        let control = AgentRunControl::with_budget(budget);

        assert_eq!(control.begin_model_call("one"), Ok(1));
        assert!(control.record_checkpoint("model", "answer", "evidence-a"));
        assert_eq!(control.begin_model_call("two"), Ok(2));
        assert!(!control.record_checkpoint("model", "answer", "evidence-a"));
        assert_eq!(
            control.begin_model_call("three"),
            Err(RunStopReason::ModelCallBudgetExceeded)
        );
        assert_eq!(control.progress().budget_extensions, 1);
    }

    #[test]
    fn enforces_model_and_tool_call_budgets() {
        let model_control = AgentRunControl::with_budget(test_budget());
        assert_eq!(model_control.begin_model_call("one"), Ok(1));
        assert_eq!(model_control.begin_model_call("two"), Ok(2));
        assert_eq!(
            model_control.begin_model_call("three"),
            Err(RunStopReason::ModelCallBudgetExceeded)
        );
        assert_eq!(model_control.progress().model_calls, 2);

        let tool_control = AgentRunControl::with_budget(test_budget());
        assert_eq!(tool_control.begin_tool_call("main", "file.read", "a"), Ok(1));
        assert_eq!(tool_control.begin_tool_call("main", "file.read", "b"), Ok(2));
        assert_eq!(
            tool_control.begin_tool_call("main", "file.read", "c"),
            Err(RunStopReason::ToolCallBudgetExceeded)
        );
    }

    #[test]
    fn detects_repeated_actions_within_a_scope() {
        let mut budget = test_budget();
        budget.initial_tool_calls = 10;
        budget.max_tool_calls = 10;
        let control = AgentRunControl::with_budget(budget);
        assert!(control.begin_tool_call("worker-1", "file.read", "a").is_ok());
        assert!(control.begin_tool_call("worker-1", "file.read", "a").is_ok());
        assert_eq!(
            control.begin_tool_call("worker-1", "file.read", "a"),
            Err(RunStopReason::RepeatedAction)
        );
    }

    #[test]
    fn a_different_action_resets_the_repeat_guard() {
        let mut budget = test_budget();
        budget.initial_tool_calls = 10;
        budget.max_tool_calls = 10;
        let control = AgentRunControl::with_budget(budget);
        assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
        assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
        assert!(control.begin_tool_call("main", "file.read", "b").is_ok());
        assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
        assert_eq!(control.stop_reason(), None);
    }

    #[test]
    fn detects_short_alternating_action_cycles() {
        let mut budget = test_budget();
        budget.initial_tool_calls = 12;
        budget.max_tool_calls = 12;
        let control = AgentRunControl::with_budget(budget);
        for input in ["a", "b", "a", "b", "a"] {
            assert!(control
                .begin_tool_call("main", "file.read", input)
                .is_ok());
        }
        assert_eq!(
            control.begin_tool_call("main", "file.read", "b"),
            Err(RunStopReason::RepeatedAction)
        );
    }

    #[test]
    fn stops_after_no_progress() {
        let control = AgentRunControl::with_budget(test_budget());
        thread::sleep(Duration::from_millis(35));
        assert_eq!(control.stop_reason(), Some(RunStopReason::NoProgress));
    }

    #[test]
    fn hard_deadline_wins_before_the_idle_limit() {
        let mut budget = test_budget();
        budget.max_duration = Duration::from_millis(20);
        budget.no_progress_timeout = Duration::from_secs(1);
        let control = AgentRunControl::with_budget(budget);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            control.stop_reason(),
            Some(RunStopReason::DeadlineExceeded)
        );
    }

    #[test]
    fn user_cancellation_is_sticky() {
        let control = AgentRunControl::with_budget(test_budget());
        control.request_cancel();
        control.mark_progress("model", "late delta");
        assert_eq!(
            control.stop_reason(),
            Some(RunStopReason::UserCancelled)
        );
    }

    #[test]
    fn snapshot_excludes_permission_wait_time() {
        let control = AgentRunControl::with_budget(test_budget());
        control.begin_model_call("planning").expect("model call should start");
        control.record_partial_output("verified work");
        control.finish_model_call();
        let snapshot = control.snapshot();
        thread::sleep(Duration::from_millis(35));
        let resumed = AgentRunControl::from_snapshot(snapshot);
        assert_eq!(resumed.stop_reason(), None);
        assert_eq!(resumed.partial_output(), "verified work");
        assert_eq!(resumed.progress().model_calls, 1);
    }

    #[test]
    fn active_model_call_uses_the_model_timeout_before_no_progress() {
        let mut budget = test_budget();
        budget.max_duration = Duration::from_secs(1);
        budget.no_progress_timeout = Duration::from_millis(20);
        budget.model_call_timeout = Duration::from_millis(80);
        let control = AgentRunControl::with_budget(budget);

        control
            .begin_model_call("executor")
            .expect("model call should start");
        thread::sleep(Duration::from_millis(35));
        assert_eq!(control.stop_reason(), None);

        control.finish_model_call();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(control.stop_reason(), Some(RunStopReason::NoProgress));
    }
}
