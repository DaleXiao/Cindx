use crate::control::progress::RunTelemetryCountersSnapshot;
use crate::control::RunStopReason;
use crate::resource_ledger::RunResourceSnapshot;
use crate::result_frontier::BestKnownResult;
use crate::run_budget::{RunBudget, RunStageClass};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunStageUsageSnapshot {
    pub model_calls: usize,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSteer {
    pub queue_id: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunEpochLease {
    pub(super) epoch: u64,
}

impl RunEpochLease {
    pub fn epoch(self) -> u64 {
        self.epoch
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunEpochLeaseOutcome {
    Acquired(RunEpochLease),
    RestartAfterSteer,
    Stopped(RunStopReason),
    TerminalCommitted,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunPreparationCommit<T> {
    Committed { value: T, lease: RunEpochLease },
    RestartAfterSteer,
    Stopped(RunStopReason),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunPreparationCheckpoint<T> {
    Committed(T),
    RestartAfterSteer,
    Stopped(RunStopReason),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunStartCheckpoint<T> {
    Committed(T),
    Stopped(RunStopReason),
    TerminalCommitted,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunExecutionStepCommit<T> {
    Committed(T),
    RestartAfterSteer,
    Stopped(RunStopReason),
    TerminalCommitted,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunTerminalCommit<T> {
    Committed(T),
    RestartAfterSteer,
    Stopped(RunStopReason),
    AlreadyCommitted,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunSteerBatchCommit<T> {
    NoPending,
    Committed { value: T, steers: Vec<RunSteer> },
    Stopped(RunStopReason),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunSteerRequestCommit<T> {
    Committed { value: T, steer: RunSteer },
    Duplicate,
    CapacityReached,
    Stopped(RunStopReason),
    TerminalCommitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunToolCallStart {
    Started(usize),
    RestartAfterSteer,
    Stopped(RunStopReason),
    TerminalCommitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunPhase {
    Preparing,
    Executing,
    TerminalCommitted,
}

#[derive(Debug, Clone)]
pub struct RunControlSnapshot {
    pub(super) budget: RunBudget,
    pub(super) elapsed_active: Duration,
    pub(super) model_calls: usize,
    pub(super) tool_calls: usize,
    pub(super) agent_turns: usize,
    pub(super) repair_attempts: usize,
    pub(super) partial_output: String,
    pub(super) action_history: BTreeMap<String, (u64, usize)>,
    pub(super) recent_actions: BTreeMap<String, VecDeque<u64>>,
    pub(super) continuation_actions: BTreeMap<String, u64>,
    pub(super) observation_fingerprints: BTreeSet<u64>,
    pub(super) observation_count: usize,
    pub(super) checkpoint_fingerprints: BTreeSet<u64>,
    pub(super) checkpoint_count: usize,
    pub(super) goal_delta_fingerprints: BTreeSet<u64>,
    pub(super) goal_delta_count: usize,
    pub(super) model_extension_goal_delta: usize,
    pub(super) tool_extension_goal_delta: usize,
    pub(super) agent_turn_extension_goal_delta: usize,
    pub(super) model_call_limit: usize,
    pub(super) tool_call_limit: usize,
    pub(super) agent_turn_limit: usize,
    pub(super) budget_extensions: usize,
    pub(super) stop_reason: Option<RunStopReason>,
    pub(super) pending_steers: VecDeque<RunSteer>,
    pub(super) steer_epoch: u64,
    pub(super) applied_steer_epoch: u64,
    pub(super) phase: RunPhase,
    pub(super) stage_usage: BTreeMap<RunStageClass, RunStageUsageSnapshot>,
    pub(super) best_known_result: Option<BestKnownResult>,
    pub(super) result_frontier: Vec<BestKnownResult>,
    pub(super) resources: RunResourceSnapshot,
    pub(super) telemetry: RunTelemetryCountersSnapshot,
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
    pub resources: RunResourceSnapshot,
    /// Cumulative measurement counters for this control segment.
    pub telemetry: RunTelemetryCountersSnapshot,
}

#[derive(Debug, Clone)]
pub(super) struct RunStageUsage {
    pub(super) started_at: Instant,
    pub(super) model_calls: usize,
}

pub(super) fn snapshot_stage_usage(
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

pub(super) fn restore_stage_usage(
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
