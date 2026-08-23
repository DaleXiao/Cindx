use super::*;

pub(crate) const PARTIAL_OUTPUT_MAX_CHARS: usize = 24_000;

/// Cumulative measurement counters for one control segment. They never gate
/// execution. Suspension snapshots restore them with the other progress
/// counters; a continuation starts a fresh segment at zero. Start instants
/// pair with finishes in stack order, which keeps cumulative wait/execution
/// sums order-invariant under concurrent calls.
#[derive(Debug, Default)]
pub(super) struct RunTelemetryCounters {
    active_model_call_starts: Vec<Instant>,
    active_tool_call_starts: Vec<Instant>,
    model_wait_ms: u64,
    tool_execution_ms: u64,
    context_compactions: u64,
    rolling_summaries: u64,
    retrieval_ms: u64,
    retrieval_channels: u64,
    retrieval_channel_hits: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunTelemetryCountersSnapshot {
    /// Cumulative provider wait across finished model calls.
    pub model_wait_ms: u64,
    /// Cumulative execution time across finished tool calls.
    pub tool_execution_ms: u64,
    pub context_compactions: u64,
    pub rolling_summaries: u64,
    pub retrieval_ms: u64,
    pub retrieval_channels: u64,
    pub retrieval_channel_hits: u64,
}

impl RunTelemetryCounters {
    pub(super) fn restore(snapshot: RunTelemetryCountersSnapshot) -> Self {
        Self {
            active_model_call_starts: Vec::new(),
            active_tool_call_starts: Vec::new(),
            model_wait_ms: snapshot.model_wait_ms,
            tool_execution_ms: snapshot.tool_execution_ms,
            context_compactions: snapshot.context_compactions,
            rolling_summaries: snapshot.rolling_summaries,
            retrieval_ms: snapshot.retrieval_ms,
            retrieval_channels: snapshot.retrieval_channels,
            retrieval_channel_hits: snapshot.retrieval_channel_hits,
        }
    }

    pub(super) fn snapshot(&self) -> RunTelemetryCountersSnapshot {
        RunTelemetryCountersSnapshot {
            model_wait_ms: self.model_wait_ms,
            tool_execution_ms: self.tool_execution_ms,
            context_compactions: self.context_compactions,
            rolling_summaries: self.rolling_summaries,
            retrieval_ms: self.retrieval_ms,
            retrieval_channels: self.retrieval_channels,
            retrieval_channel_hits: self.retrieval_channel_hits,
        }
    }

    pub(super) fn absorb(&mut self, snapshot: RunTelemetryCountersSnapshot) {
        self.model_wait_ms = self.model_wait_ms.saturating_add(snapshot.model_wait_ms);
        self.tool_execution_ms = self
            .tool_execution_ms
            .saturating_add(snapshot.tool_execution_ms);
        self.context_compactions = self
            .context_compactions
            .saturating_add(snapshot.context_compactions);
        self.rolling_summaries = self
            .rolling_summaries
            .saturating_add(snapshot.rolling_summaries);
        self.retrieval_ms = self.retrieval_ms.saturating_add(snapshot.retrieval_ms);
        self.retrieval_channels = self
            .retrieval_channels
            .saturating_add(snapshot.retrieval_channels);
        self.retrieval_channel_hits = self
            .retrieval_channel_hits
            .saturating_add(snapshot.retrieval_channel_hits);
    }

    pub(super) fn begin_model_call(&mut self) {
        self.active_model_call_starts.push(Instant::now());
    }

    pub(super) fn finish_model_call(&mut self) {
        if let Some(started) = self.active_model_call_starts.pop() {
            self.model_wait_ms = self
                .model_wait_ms
                .saturating_add(started.elapsed().as_millis() as u64);
        }
    }

    pub(super) fn begin_tool_call(&mut self) {
        self.active_tool_call_starts.push(Instant::now());
    }

    pub(super) fn begin_tool_call_batch(&mut self, call_count: usize) {
        let started = Instant::now();
        self.active_tool_call_starts
            .extend(std::iter::repeat_n(started, call_count));
    }

    pub(super) fn finish_tool_call(&mut self) {
        if let Some(started) = self.active_tool_call_starts.pop() {
            self.tool_execution_ms = self
                .tool_execution_ms
                .saturating_add(started.elapsed().as_millis() as u64);
        }
    }

    pub(super) fn record_context_compaction(&mut self) {
        self.context_compactions = self.context_compactions.saturating_add(1);
    }

    pub(super) fn record_rolling_summary(&mut self) {
        self.rolling_summaries = self.rolling_summaries.saturating_add(1);
    }

    pub(super) fn record_retrieval(&mut self, duration_ms: u64, channels: u64, hits: u64) {
        self.retrieval_ms = self.retrieval_ms.saturating_add(duration_ms);
        self.retrieval_channels = self.retrieval_channels.saturating_add(channels);
        self.retrieval_channel_hits = self.retrieval_channel_hits.saturating_add(hits);
    }
}

impl AgentRunControl {
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
            resources: state.resources.snapshot(),
            telemetry: state.telemetry.snapshot(),
        }
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

    pub fn mark_progress_at(&self, expected_epoch: u64, stage: &str, detail: &str) -> bool {
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.stop_reason.is_some()
            || self.cancellation_requested()
            || !self.objective_epoch_matches_locked(&state, expected_epoch)
        {
            return false;
        }
        state.stage = stage.to_string();
        state.detail = detail.to_string();
        state.last_progress_at = Instant::now();
        true
    }

    /// Counts one session-history compaction materialized for this run.
    /// Measurement only; never gates execution.
    pub fn record_context_compaction(&self) {
        let mut state = self.state.lock().expect("run control state poisoned");
        state.telemetry.record_context_compaction();
    }

    /// Counts one rolling summary injected into the run context.
    /// Measurement only; never gates execution.
    pub fn record_rolling_summary(&self) {
        let mut state = self.state.lock().expect("run control state poisoned");
        state.telemetry.record_rolling_summary();
    }

    /// Accumulates one completed workspace retrieval pass.
    /// Measurement only; never gates execution.
    pub fn record_retrieval(&self, duration_ms: u64, channels: u64, channel_hits: u64) {
        let mut state = self.state.lock().expect("run control state poisoned");
        state
            .telemetry
            .record_retrieval(duration_ms, channels, channel_hits);
    }

    pub fn record_partial_output(&self, output: &str) {
        let epoch = self.steer_epoch();
        let _ = self.record_partial_output_at(epoch, output);
    }

    pub fn record_partial_output_at(&self, expected_epoch: u64, output: &str) -> bool {
        let output = output.trim();
        if output.is_empty() {
            return false;
        }
        let partial_output = output
            .char_indices()
            .rev()
            .nth(PARTIAL_OUTPUT_MAX_CHARS.saturating_sub(1))
            .map(|(start, _)| output[start..].to_string())
            .unwrap_or_else(|| output.to_string());
        let mut state = self.state.lock().expect("run control state poisoned");
        if state.phase == RunPhase::TerminalCommitted
            || self.steer_epoch.load(Ordering::SeqCst) != expected_epoch
            || state.applied_steer_epoch != expected_epoch
            || !state.pending_steers.is_empty()
        {
            return false;
        }
        state.partial_output = partial_output;
        state.last_progress_at = Instant::now();
        true
    }
}
