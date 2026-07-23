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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunStageClass {
    Conductor,
    Candidate,
    Worker,
    Reviewer,
    Synthesizer,
    Repair,
    Other,
}

impl RunStageClass {
    pub fn from_label(label: &str) -> Self {
        let label = label.to_ascii_lowercase();
        if label.contains("repair") || label.contains("recover") || label.contains("retry") {
            Self::Repair
        } else if label.contains("conductor") || label.contains("planner") {
            Self::Conductor
        } else if label.contains("candidate") {
            Self::Candidate
        } else if label.contains("review") || label.contains("critic") {
            Self::Reviewer
        } else if label.contains("synth") || label.contains("summar") || label.contains("final") {
            Self::Synthesizer
        } else if label.contains("worker") {
            Self::Worker
        } else {
            Self::Other
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Reviewer | Self::Synthesizer | Self::Repair)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunStageBudget {
    pub max_model_calls: usize,
    pub max_duration: Duration,
    pub terminal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultQuality {
    Draft,
    Substantive,
    Grounded,
    Verified,
    Synthesized,
}

impl ResultQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Substantive => "substantive",
            Self::Grounded => "grounded",
            Self::Verified => "verified",
            Self::Synthesized => "synthesized",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BestKnownResult {
    pub content: String,
    pub stage: String,
    pub quality: ResultQuality,
    pub evidence_count: usize,
    pub verified: bool,
    pub deliverable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunStageUsageSnapshot {
    pub model_calls: usize,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBudget {
    pub max_duration: Duration,
    pub model_call_timeout: Duration,
    pub tool_call_timeout: Duration,
    pub initial_model_calls: usize,
    pub max_model_calls: usize,
    pub model_calls_per_extension: usize,
    pub initial_tool_calls: usize,
    pub max_tool_calls: usize,
    pub tool_calls_per_extension: usize,
    pub no_progress_timeout: Duration,
    pub max_identical_actions: usize,
    pub initial_agent_turns: usize,
    pub max_agent_turns: usize,
    pub agent_turns_per_extension: usize,
    pub max_repair_attempts: usize,
    pub terminal_model_call_reserve: usize,
    pub terminal_time_reserve: Duration,
}

impl RunBudget {
    pub fn for_effort(effort: &str) -> Self {
        match effort {
            "fast" => Self {
                max_duration: Duration::from_secs(5 * 60),
                model_call_timeout: Duration::from_secs(3 * 60),
                tool_call_timeout: Duration::from_secs(5 * 60),
                initial_model_calls: 6,
                max_model_calls: 12,
                model_calls_per_extension: 3,
                initial_tool_calls: 12,
                max_tool_calls: 24,
                tool_calls_per_extension: 6,
                no_progress_timeout: Duration::from_secs(90),
                max_identical_actions: 3,
                initial_agent_turns: 6,
                max_agent_turns: 12,
                agent_turns_per_extension: 3,
                max_repair_attempts: 2,
                terminal_model_call_reserve: 2,
                terminal_time_reserve: Duration::from_secs(30),
            },
            "pro" => Self {
                max_duration: Duration::from_secs(4 * 60 * 60),
                model_call_timeout: Duration::from_secs(15 * 60),
                tool_call_timeout: Duration::from_secs(60 * 60),
                initial_model_calls: 48,
                max_model_calls: 384,
                model_calls_per_extension: 48,
                initial_tool_calls: 96,
                max_tool_calls: 768,
                tool_calls_per_extension: 96,
                no_progress_timeout: Duration::from_secs(5 * 60),
                max_identical_actions: 4,
                initial_agent_turns: 48,
                max_agent_turns: 384,
                agent_turns_per_extension: 48,
                max_repair_attempts: 8,
                terminal_model_call_reserve: 8,
                terminal_time_reserve: Duration::from_secs(10 * 60),
            },
            _ => Self {
                max_duration: Duration::from_secs(45 * 60),
                model_call_timeout: Duration::from_secs(5 * 60),
                tool_call_timeout: Duration::from_secs(15 * 60),
                initial_model_calls: 18,
                max_model_calls: 72,
                model_calls_per_extension: 18,
                initial_tool_calls: 36,
                max_tool_calls: 144,
                tool_calls_per_extension: 36,
                no_progress_timeout: Duration::from_secs(2 * 60),
                max_identical_actions: 3,
                initial_agent_turns: 18,
                max_agent_turns: 72,
                agent_turns_per_extension: 18,
                max_repair_attempts: 4,
                terminal_model_call_reserve: 4,
                terminal_time_reserve: Duration::from_secs(2 * 60),
            },
        }
    }

    pub fn stage_budget(self, class: RunStageClass) -> RunStageBudget {
        let (max_model_calls, duration_divisor) = match class {
            RunStageClass::Conductor => (self.max_repair_attempts.saturating_add(1), 5),
            RunStageClass::Candidate => (self.max_model_calls.saturating_div(3).max(2), 2),
            RunStageClass::Worker => (self.max_model_calls.saturating_div(2).max(2), 2),
            RunStageClass::Reviewer => (self.max_repair_attempts.saturating_add(2), 4),
            RunStageClass::Synthesizer => (self.max_repair_attempts.saturating_add(2), 3),
            RunStageClass::Repair => (self.max_repair_attempts, 4),
            RunStageClass::Other => (self.max_model_calls, 1),
        };
        RunStageBudget {
            max_model_calls: max_model_calls.min(self.max_model_calls).max(1),
            max_duration: self.max_duration / duration_divisor,
            terminal: class.is_terminal(),
        }
    }
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
    best_known_result: Option<BestKnownResult>,
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

    fn with_budget(budget: RunBudget) -> Self {
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
                best_known_result: None,
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
                best_known_result: snapshot.best_known_result,
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
                        best_known_result: snapshot.best_known_result,
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
            best_known_result: state.best_known_result.clone(),
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
            if !stage_budget.terminal
                && (remaining <= self.budget.terminal_time_reserve
                    || self.model_calls.load(Ordering::SeqCst)
                        >= state
                            .model_call_limit
                            .saturating_sub(self.budget.terminal_model_call_reserve)
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
        self.state
            .lock()
            .expect("run control state poisoned")
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
        let content = bounded_result_content(content);
        if content.is_empty() {
            return false;
        }
        let candidate = BestKnownResult {
            content,
            stage: stage.to_string(),
            quality,
            evidence_count,
            verified,
            deliverable,
        };
        let mut state = self.state.lock().expect("run control state poisoned");
        let should_replace = state
            .best_known_result
            .as_ref()
            .is_none_or(|current| result_rank(&candidate) > result_rank(current));
        if should_replace {
            state.best_known_result = Some(candidate);
            state.last_progress_at = Instant::now();
        }
        should_replace
    }

    pub fn best_known_result(&self) -> Option<BestKnownResult> {
        self.state
            .lock()
            .expect("run control state poisoned")
            .best_known_result
            .clone()
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
            best_known_result: state.best_known_result.clone(),
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

fn bounded_result_content(content: &str) -> String {
    let content = content.trim();
    content
        .char_indices()
        .rev()
        .nth(PARTIAL_OUTPUT_MAX_CHARS.saturating_sub(1))
        .map(|(start, _)| content[start..].to_string())
        .unwrap_or_else(|| content.to_string())
}

fn result_rank(result: &BestKnownResult) -> (bool, ResultQuality, bool, usize, usize) {
    (
        result.deliverable,
        result.quality,
        result.verified,
        result.evidence_count,
        result.content.chars().count(),
    )
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
mod tests {
    use super::*;
    use std::thread;

    fn test_budget() -> RunBudget {
        RunBudget {
            max_duration: Duration::from_millis(60),
            model_call_timeout: Duration::from_millis(40),
            tool_call_timeout: Duration::from_millis(45),
            initial_model_calls: 2,
            max_model_calls: 2,
            model_calls_per_extension: 1,
            initial_tool_calls: 2,
            max_tool_calls: 2,
            tool_calls_per_extension: 1,
            no_progress_timeout: Duration::from_millis(25),
            max_identical_actions: 2,
            initial_agent_turns: 2,
            max_agent_turns: 2,
            agent_turns_per_extension: 1,
            max_repair_attempts: 2,
            terminal_model_call_reserve: 1,
            terminal_time_reserve: Duration::from_millis(5),
        }
    }

    #[test]
    fn pro_budget_allows_a_long_running_segment() {
        let budget = RunBudget::for_effort("pro");
        assert_eq!(budget.max_duration, Duration::from_secs(4 * 60 * 60));
        assert_eq!(budget.model_call_timeout, Duration::from_secs(15 * 60));
        assert_eq!(budget.tool_call_timeout, Duration::from_secs(60 * 60));
        assert_eq!(budget.initial_model_calls, 48);
        assert_eq!(budget.max_model_calls, 384);
        assert_eq!(budget.initial_tool_calls, 96);
        assert_eq!(budget.max_tool_calls, 768);
        assert_eq!(budget.no_progress_timeout, Duration::from_secs(5 * 60));
        assert_eq!(budget.max_agent_turns, 384);
        assert_eq!(budget.max_repair_attempts, 8);
        assert_eq!(budget.terminal_model_call_reserve, 8);
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
    fn material_checkpoints_extend_a_segment_but_new_observations_do_not() {
        let mut budget = test_budget();
        budget.initial_model_calls = 1;
        budget.max_model_calls = 3;
        let control = AgentRunControl::with_budget(budget);

        assert_eq!(control.begin_model_call("one"), Ok(1));
        assert!(control.record_observation("model", "answer", "novel chatter"));
        assert_eq!(
            control.begin_model_call("two"),
            Err(RunStopReason::ModelCallBudgetExceeded)
        );

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
        assert_eq!(control.progress().observations, 0);
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
        assert_eq!(
            tool_control.begin_tool_call("main", "file.read", "a"),
            Ok(1)
        );
        assert_eq!(
            tool_control.begin_tool_call("main", "file.read", "b"),
            Ok(2)
        );
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
        assert!(control
            .begin_tool_call("worker-1", "file.read", "a")
            .is_ok());
        assert!(control
            .begin_tool_call("worker-1", "file.read", "a")
            .is_ok());
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
            assert!(control.begin_tool_call("main", "file.read", input).is_ok());
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
        assert_eq!(control.stop_reason(), Some(RunStopReason::DeadlineExceeded));
    }

    #[test]
    fn user_cancellation_is_sticky() {
        let control = AgentRunControl::with_budget(test_budget());
        control.request_cancel();
        control.mark_progress("model", "late delta");
        assert_eq!(control.stop_reason(), Some(RunStopReason::UserCancelled));
    }

    #[test]
    fn snapshot_excludes_permission_wait_time() {
        let control = AgentRunControl::with_budget(test_budget());
        control
            .begin_model_call("planning")
            .expect("model call should start");
        control.record_partial_output("verified work");
        assert!(control.record_observation("model_result", "planning", "draft-a"));
        assert!(control.record_checkpoint("tool_result", "file.read", "evidence-a"));
        control.finish_model_call();
        let snapshot = control.snapshot();
        thread::sleep(Duration::from_millis(35));
        let resumed = AgentRunControl::from_snapshot(snapshot);
        assert_eq!(resumed.stop_reason(), None);
        assert_eq!(resumed.partial_output(), "verified work");
        assert_eq!(resumed.progress().model_calls, 1);
        assert_eq!(resumed.progress().observations, 1);
        assert_eq!(resumed.progress().checkpoints, 1);
    }

    #[test]
    fn continuation_starts_a_fresh_bounded_segment_after_budget_exhaustion() {
        let control = AgentRunControl::with_budget(test_budget());
        control.record_partial_output("verified work");
        assert!(control.record_checkpoint("tool", "created file", "artifact-a"));
        assert_eq!(control.request_steer("queue-a"), Ok(true));
        assert_eq!(control.begin_model_call("one"), Ok(1));
        assert_eq!(control.begin_model_call("two"), Ok(2));
        assert_eq!(
            control.begin_model_call("three"),
            Err(RunStopReason::ModelCallBudgetExceeded)
        );

        let continued = AgentRunControl::from_snapshot_for_continuation(control.snapshot())
            .expect("budget exhaustion should be resumable");
        assert_eq!(continued.stop_reason(), None);
        assert_eq!(continued.partial_output(), "verified work");
        assert_eq!(continued.progress().model_calls, 0);
        assert_eq!(continued.progress().checkpoints, 1);
        assert_eq!(continued.begin_model_call("continued-one"), Ok(1));
        assert_eq!(continued.begin_model_call("continued-two"), Ok(2));
        assert_eq!(
            continued.begin_model_call("continued-three"),
            Err(RunStopReason::ModelCallBudgetExceeded)
        );
        assert_eq!(
            continued
                .take_pending_steers()
                .into_iter()
                .map(|steer| steer.queue_id)
                .collect::<Vec<_>>(),
            vec!["queue-a"]
        );
    }

    #[test]
    fn permission_resume_preserves_consumed_budget() {
        let control = AgentRunControl::with_budget(test_budget());
        assert_eq!(control.begin_model_call("planning"), Ok(1));
        control.finish_model_call();

        let resumed = AgentRunControl::from_snapshot(control.snapshot());
        assert_eq!(resumed.stop_reason(), None);
        assert_eq!(resumed.progress().model_calls, 1);
        assert_eq!(resumed.begin_model_call("after-permission"), Ok(2));
        assert_eq!(
            resumed.begin_model_call("over-budget"),
            Err(RunStopReason::ModelCallBudgetExceeded)
        );
    }

    #[test]
    fn user_cancelled_snapshot_cannot_continue() {
        let control = AgentRunControl::with_budget(test_budget());
        control.request_cancel();
        assert_eq!(
            AgentRunControl::from_snapshot_for_continuation(control.snapshot())
                .expect_err("user cancellation must remain terminal"),
            RunStopReason::UserCancelled
        );
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

    #[test]
    fn active_tool_call_uses_its_own_timeout_and_finishes_cleanly() {
        let mut budget = test_budget();
        budget.max_duration = Duration::from_secs(1);
        budget.no_progress_timeout = Duration::from_millis(20);
        budget.tool_call_timeout = Duration::from_millis(90);
        let control = AgentRunControl::with_budget(budget);

        control
            .begin_tool_call("main", "shell.run", "build")
            .expect("tool call should start");
        thread::sleep(Duration::from_millis(35));
        assert_eq!(control.stop_reason(), None);

        control.finish_tool_call();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(control.stop_reason(), Some(RunStopReason::NoProgress));
    }

    #[test]
    fn rejected_tool_calls_do_not_consume_the_call_counter() {
        let mut budget = test_budget();
        budget.initial_tool_calls = 10;
        budget.max_tool_calls = 10;
        budget.max_identical_actions = 1;
        let control = AgentRunControl::with_budget(budget);

        assert!(control.begin_tool_call("main", "file.read", "a").is_ok());
        control.finish_tool_call();
        assert_eq!(
            control.begin_tool_call("main", "file.read", "a"),
            Err(RunStopReason::RepeatedAction)
        );
        assert_eq!(control.progress().tool_calls, 1);
    }

    #[test]
    fn steering_is_deduplicated_and_survives_a_snapshot() {
        let control = AgentRunControl::with_budget(test_budget());
        assert_eq!(control.request_steer("queue-a"), Ok(true));
        assert_eq!(control.request_steer("queue-a"), Ok(false));
        assert!(control.has_pending_steer());

        let resumed = AgentRunControl::from_snapshot(control.snapshot());
        assert_eq!(
            resumed
                .take_pending_steers()
                .into_iter()
                .map(|steer| steer.queue_id)
                .collect::<Vec<_>>(),
            vec!["queue-a"]
        );
        assert!(!resumed.has_pending_steer());
    }

    #[test]
    fn partial_output_keeps_the_latest_bounded_unicode_tail() {
        let control = AgentRunControl::with_budget(test_budget());
        let output = format!(
            "{}{}",
            "old".repeat(PARTIAL_OUTPUT_MAX_CHARS),
            "latest verified result 你好"
        );

        control.record_partial_output(&output);

        let partial = control.partial_output();
        assert_eq!(partial.chars().count(), PARTIAL_OUTPUT_MAX_CHARS);
        assert!(partial.ends_with("latest verified result 你好"));
        assert_ne!(partial, output);
    }

    #[test]
    fn agent_turns_repairs_and_model_calls_are_accounted_independently() {
        let control = AgentRunControl::with_budget(test_budget());

        assert_eq!(control.begin_model_call("executor"), Ok(1));
        control.finish_model_call();
        assert_eq!(control.record_agent_turn("executor"), Ok(1));
        assert_eq!(control.begin_repair_attempt("protocol_repair"), Ok(1));

        let progress = control.progress();
        assert_eq!(progress.model_calls, 1);
        assert_eq!(progress.agent_turns, 1);
        assert_eq!(progress.repair_attempts, 1);
        assert_eq!(progress.tool_calls, 0);
    }

    #[test]
    fn stage_exhaustion_is_local_and_preserves_terminal_reserve() {
        let mut budget = test_budget();
        budget.initial_model_calls = 4;
        budget.max_model_calls = 4;
        budget.terminal_model_call_reserve = 1;
        let control = AgentRunControl::with_budget(budget);

        assert!(control
            .begin_stage_model_call("candidate_1", RunStageClass::Candidate)
            .is_ok());
        control.finish_model_call();
        assert!(control
            .begin_stage_model_call("candidate_2", RunStageClass::Candidate)
            .is_ok());
        control.finish_model_call();
        assert_eq!(
            control.begin_stage_model_call("candidate_3", RunStageClass::Candidate),
            Err(RunStopReason::StageBudgetExhausted)
        );
        assert_eq!(control.stop_reason(), None);
        assert!(control
            .begin_stage_model_call("synthesizer", RunStageClass::Synthesizer)
            .is_ok());
    }

    #[test]
    fn best_known_result_is_ranked_and_survives_resume() {
        let control = AgentRunControl::with_budget(test_budget());
        assert!(control.record_best_known_result(
            "candidate",
            "grounded candidate",
            ResultQuality::Grounded,
            2,
            false,
            false,
        ));
        assert!(!control.record_best_known_result(
            "draft",
            "longer but weaker draft",
            ResultQuality::Draft,
            0,
            false,
            false,
        ));
        assert!(control.record_best_known_result(
            "verification",
            "verified answer",
            ResultQuality::Verified,
            3,
            true,
            true,
        ));

        let resumed = AgentRunControl::from_snapshot(control.snapshot());
        let best = resumed
            .best_known_result()
            .expect("best result should persist");
        assert_eq!(best.content, "verified answer");
        assert_eq!(best.quality, ResultQuality::Verified);
        assert!(best.verified);
        assert!(best.deliverable);
    }
}
