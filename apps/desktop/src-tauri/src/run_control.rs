use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PARTIAL_OUTPUT_MAX_CHARS: usize = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunStopReason {
    UserCancelled,
    DeadlineExceeded,
    ModelCallBudgetExceeded,
    ToolCallBudgetExceeded,
    NoProgress,
    RepeatedAction,
}

impl RunStopReason {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::UserCancelled => "user_cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ModelCallBudgetExceeded => "model_call_budget_exceeded",
            Self::ToolCallBudgetExceeded => "tool_call_budget_exceeded",
            Self::NoProgress => "no_progress",
            Self::RepeatedAction => "repeated_action",
        }
    }

    pub(crate) fn is_user_cancelled(self) -> bool {
        self == Self::UserCancelled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunBudget {
    pub(crate) max_duration: Duration,
    pub(crate) max_model_calls: usize,
    pub(crate) max_tool_calls: usize,
    pub(crate) no_progress_timeout: Duration,
    pub(crate) max_identical_actions: usize,
}

impl RunBudget {
    pub(crate) fn for_effort(effort: &str) -> Self {
        match effort {
            "fast" => Self {
                max_duration: Duration::from_secs(3 * 60),
                max_model_calls: 6,
                max_tool_calls: 12,
                no_progress_timeout: Duration::from_secs(90),
                max_identical_actions: 3,
            },
            "pro" => Self {
                max_duration: Duration::from_secs(60 * 60),
                max_model_calls: 96,
                max_tool_calls: 180,
                no_progress_timeout: Duration::from_secs(5 * 60),
                max_identical_actions: 4,
            },
            _ => Self {
                max_duration: Duration::from_secs(8 * 60),
                max_model_calls: 18,
                max_tool_calls: 36,
                no_progress_timeout: Duration::from_secs(2 * 60),
                max_identical_actions: 3,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RunControlSnapshot {
    budget: RunBudget,
    elapsed_active: Duration,
    model_calls: usize,
    tool_calls: usize,
    partial_output: String,
    action_history: BTreeMap<String, (String, usize)>,
}

#[derive(Debug, Clone)]
pub(crate) struct RunProgressSnapshot {
    pub(crate) stage: String,
    pub(crate) detail: String,
    pub(crate) elapsed: Duration,
    pub(crate) remaining: Duration,
    pub(crate) model_calls: usize,
    pub(crate) tool_calls: usize,
}

#[derive(Debug)]
struct RunMutableState {
    started_at: Instant,
    last_progress_at: Instant,
    stage: String,
    detail: String,
    partial_output: String,
    action_history: BTreeMap<String, (String, usize)>,
    stop_reason: Option<RunStopReason>,
}

#[derive(Debug)]
pub(crate) struct AgentRunControl {
    budget: RunBudget,
    user_cancelled: AtomicBool,
    model_calls: AtomicUsize,
    tool_calls: AtomicUsize,
    state: Mutex<RunMutableState>,
}

impl AgentRunControl {
    pub(crate) fn new(effort: &str) -> Self {
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
                stop_reason: None,
            }),
        }
    }

    pub(crate) fn from_snapshot(snapshot: RunControlSnapshot) -> Self {
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
                stop_reason: None,
            }),
        }
    }

    pub(crate) fn snapshot(&self) -> RunControlSnapshot {
        let state = self.state.lock().expect("run control state poisoned");
        RunControlSnapshot {
            budget: self.budget,
            elapsed_active: state.started_at.elapsed().min(self.budget.max_duration),
            model_calls: self.model_calls.load(Ordering::SeqCst),
            tool_calls: self.tool_calls.load(Ordering::SeqCst),
            partial_output: state.partial_output.clone(),
            action_history: state.action_history.clone(),
        }
    }

    pub(crate) fn request_cancel(&self) {
        self.user_cancelled.store(true, Ordering::SeqCst);
        self.set_stop_reason(RunStopReason::UserCancelled);
    }

    pub(crate) fn stop_reason(&self) -> Option<RunStopReason> {
        if self.user_cancelled.load(Ordering::SeqCst) {
            self.set_stop_reason(RunStopReason::UserCancelled);
        }
        let now = Instant::now();
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.stop_reason.is_none() {
            if now.duration_since(state.started_at) >= self.budget.max_duration {
                state.stop_reason = Some(RunStopReason::DeadlineExceeded);
            } else if now.duration_since(state.last_progress_at) >= self.budget.no_progress_timeout {
                state.stop_reason = Some(RunStopReason::NoProgress);
            }
        }
        state.stop_reason
    }

    pub(crate) fn should_stop(&self) -> bool {
        self.stop_reason().is_some()
    }

    pub(crate) fn begin_model_call(&self, stage: &str) -> Result<usize, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let call = self.model_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call > self.budget.max_model_calls {
            self.set_stop_reason(RunStopReason::ModelCallBudgetExceeded);
            return Err(RunStopReason::ModelCallBudgetExceeded);
        }
        self.mark_progress(stage, "model request started");
        Ok(call)
    }

    pub(crate) fn begin_tool_call(
        &self,
        scope: &str,
        tool_name: &str,
        input: &str,
    ) -> Result<usize, RunStopReason> {
        if let Some(reason) = self.stop_reason() {
            return Err(reason);
        }
        let call = self.tool_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call > self.budget.max_tool_calls {
            self.set_stop_reason(RunStopReason::ToolCallBudgetExceeded);
            return Err(RunStopReason::ToolCallBudgetExceeded);
        }

        let signature = format!("{tool_name}\n{input}");
        let mut state = self.state.lock().expect("run control state poisoned");
        let history = state
            .action_history
            .entry(scope.to_string())
            .or_insert_with(|| (signature.clone(), 0));
        if history.0 == signature {
            history.1 += 1;
        } else {
            *history = (signature, 1);
        }
        if history.1 > self.budget.max_identical_actions {
            state.stop_reason = Some(RunStopReason::RepeatedAction);
            return Err(RunStopReason::RepeatedAction);
        }
        state.stage = "tool".to_string();
        state.detail = tool_name.to_string();
        state.last_progress_at = Instant::now();
        Ok(call)
    }

    pub(crate) fn mark_progress(&self, stage: &str, detail: &str) {
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.stop_reason.is_some() {
            return;
        }
        state.stage = stage.to_string();
        state.detail = detail.to_string();
        state.last_progress_at = Instant::now();
    }

    pub(crate) fn record_partial_output(&self, output: &str) {
        let output = output.trim();
        if output.is_empty() {
            return;
        }
        let partial_output = if output.chars().count() > PARTIAL_OUTPUT_MAX_CHARS {
            output
                .chars()
                .take(PARTIAL_OUTPUT_MAX_CHARS)
                .collect::<String>()
        } else {
            output.to_string()
        };
        let mut state = self.state.lock().expect("run control state poisoned");
        state.partial_output = partial_output;
        state.last_progress_at = Instant::now();
    }

    pub(crate) fn partial_output(&self) -> String {
        self.state
            .lock()
            .expect("run control state poisoned")
            .partial_output
            .clone()
    }

    pub(crate) fn timeout_seconds(&self, cap_seconds: u64) -> u64 {
        let state = self.state.lock().expect("run control state poisoned");
        let remaining = self
            .budget
            .max_duration
            .saturating_sub(state.started_at.elapsed());
        remaining.as_secs().max(1).min(cap_seconds.max(1))
    }

    pub(crate) fn progress(&self) -> RunProgressSnapshot {
        let state = self.state.lock().expect("run control state poisoned");
        let elapsed = state.started_at.elapsed().min(self.budget.max_duration);
        RunProgressSnapshot {
            stage: state.stage.clone(),
            detail: state.detail.clone(),
            elapsed,
            remaining: self.budget.max_duration.saturating_sub(elapsed),
            model_calls: self.model_calls.load(Ordering::SeqCst),
            tool_calls: self.tool_calls.load(Ordering::SeqCst),
        }
    }

    pub(crate) fn budget(&self) -> RunBudget {
        self.budget
    }

    fn set_stop_reason(&self, reason: RunStopReason) {
        let mut state = self.state.lock().expect("run control state poisoned");
        state.stop_reason.get_or_insert(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn test_budget() -> RunBudget {
        RunBudget {
            max_duration: Duration::from_millis(60),
            max_model_calls: 2,
            max_tool_calls: 2,
            no_progress_timeout: Duration::from_millis(25),
            max_identical_actions: 2,
        }
    }

    #[test]
    fn pro_budget_allows_a_long_running_segment() {
        let budget = RunBudget::for_effort("pro");
        assert_eq!(budget.max_duration, Duration::from_secs(60 * 60));
        assert_eq!(budget.max_model_calls, 96);
        assert_eq!(budget.max_tool_calls, 180);
        assert_eq!(budget.no_progress_timeout, Duration::from_secs(5 * 60));
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
        budget.max_tool_calls = 10;
        let control = AgentRunControl::with_budget(budget);
        assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
        assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
        assert!(control.begin_tool_call("main", "file.read", "b").is_ok());
        assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
        assert_eq!(control.stop_reason(), None);
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
        let snapshot = control.snapshot();
        thread::sleep(Duration::from_millis(35));
        let resumed = AgentRunControl::from_snapshot(snapshot);
        assert_eq!(resumed.stop_reason(), None);
        assert_eq!(resumed.partial_output(), "verified work");
        assert_eq!(resumed.progress().model_calls, 1);
    }
}
