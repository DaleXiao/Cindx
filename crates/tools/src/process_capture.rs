use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessStopCause {
    Exit,
    Signal,
    Timeout,
    Cancelled,
    Terminated,
    OutputLimit,
    CpuLimit,
    RunEnded,
    SessionRemoved,
    AppExit,
    LaunchFailed,
}

impl ProcessStopCause {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Signal => "signal",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Terminated => "terminated",
            Self::OutputLimit => "output_limit",
            Self::CpuLimit => "cpu_limit",
            Self::RunEnded => "run_ended",
            Self::SessionRemoved => "session_removed",
            Self::AppExit => "app_exit",
            Self::LaunchFailed => "launch_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Pending,
    Running,
    Terminal(ProcessStopCause),
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessStreamPage {
    pub(crate) offset: u64,
    pub(crate) next_offset: u64,
    pub(crate) captured_bytes: u64,
    pub(crate) produced_bytes: u64,
    pub(crate) text: String,
    pub(crate) more_available: bool,
    pub(crate) complete: bool,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessSnapshot {
    pub(crate) state: &'static str,
    pub(crate) terminal: bool,
    pub(crate) termination: Option<&'static str>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) signal: Option<i32>,
    pub(crate) elapsed_ms: u64,
    pub(crate) stdout: ProcessStreamPage,
    pub(crate) stderr: ProcessStreamPage,
}

struct CapturedState {
    lifecycle: Lifecycle,
    started_at: Instant,
    finished_at: Option<Instant>,
    exit_code: Option<i32>,
    signal: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_produced: u64,
    stderr_produced: u64,
    stdout_closed: bool,
    stderr_closed: bool,
}

pub(crate) struct ProcessCapture {
    state: Mutex<CapturedState>,
    changed: Condvar,
    output_limit_bytes: u64,
}

impl ProcessCapture {
    pub(crate) fn pending(output_limit_bytes: u64) -> Self {
        Self {
            state: Mutex::new(CapturedState {
                lifecycle: Lifecycle::Pending,
                started_at: Instant::now(),
                finished_at: None,
                exit_code: None,
                signal: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
                stdout_produced: 0,
                stderr_produced: 0,
                stdout_closed: false,
                stderr_closed: false,
            }),
            changed: Condvar::new(),
            output_limit_bytes,
        }
    }

    pub(crate) fn mark_running(&self) {
        if let Ok(mut state) = self.state.lock() {
            if matches!(state.lifecycle, Lifecycle::Pending) {
                state.lifecycle = Lifecycle::Running;
                state.started_at = Instant::now();
                self.changed.notify_all();
            }
        }
    }

    pub(crate) fn append_stdout(&self, bytes: &[u8]) -> bool {
        self.append(bytes, true)
    }

    pub(crate) fn append_stderr(&self, bytes: &[u8]) -> bool {
        self.append(bytes, false)
    }

    fn append(&self, bytes: &[u8], stdout: bool) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return true;
        };
        let aggregate_before = state.stdout_produced.saturating_add(state.stderr_produced);
        let remaining = self.output_limit_bytes.saturating_sub(aggregate_before) as usize;
        let retained = remaining.min(bytes.len());
        if stdout {
            state.stdout_produced = state.stdout_produced.saturating_add(bytes.len() as u64);
            state.stdout.extend_from_slice(&bytes[..retained]);
        } else {
            state.stderr_produced = state.stderr_produced.saturating_add(bytes.len() as u64);
            state.stderr.extend_from_slice(&bytes[..retained]);
        }
        self.changed.notify_all();
        retained < bytes.len()
    }

    pub(crate) fn mark_stream_closed(&self, stdout: bool) {
        if let Ok(mut state) = self.state.lock() {
            if stdout {
                state.stdout_closed = true;
            } else {
                state.stderr_closed = true;
            }
            self.changed.notify_all();
        }
    }

    pub(crate) fn finish(
        &self,
        cause: ProcessStopCause,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) {
        if let Ok(mut state) = self.state.lock() {
            if matches!(state.lifecycle, Lifecycle::Terminal(_)) {
                return;
            }
            state.lifecycle = Lifecycle::Terminal(cause);
            state.finished_at = Some(Instant::now());
            state.exit_code = exit_code;
            state.signal = signal;
            self.changed.notify_all();
        }
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.state
            .lock()
            .map(|state| matches!(state.lifecycle, Lifecycle::Terminal(_)))
            .unwrap_or(true)
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.state
            .lock()
            .map(|state| matches!(state.lifecycle, Lifecycle::Pending))
            .unwrap_or(false)
    }

    pub(crate) fn finished_age(&self) -> Option<Duration> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.finished_at.map(|finished| finished.elapsed()))
    }

    pub(crate) fn wait_and_snapshot(
        &self,
        stdout_offset: u64,
        stderr_offset: u64,
        max_bytes: usize,
        wait: Duration,
    ) -> Result<ProcessSnapshot, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "process capture lock poisoned".to_string())?;
        let baseline = (
            state.stdout.len(),
            state.stderr.len(),
            matches!(state.lifecycle, Lifecycle::Terminal(_)),
        );
        if !wait.is_zero()
            && !baseline.2
            && stdout_offset >= state.stdout.len() as u64
            && stderr_offset >= state.stderr.len() as u64
        {
            let (next, _) = self
                .changed
                .wait_timeout(state, wait)
                .map_err(|_| "process capture lock poisoned".to_string())?;
            state = next;
        }
        snapshot_locked(&state, stdout_offset, stderr_offset, max_bytes)
    }
}

fn snapshot_locked(
    state: &CapturedState,
    stdout_offset: u64,
    stderr_offset: u64,
    max_bytes: usize,
) -> Result<ProcessSnapshot, String> {
    if stdout_offset > state.stdout.len() as u64 || stderr_offset > state.stderr.len() as u64 {
        return Err("process stream offset exceeds captured output".to_string());
    }
    let stdout_budget = max_bytes.min(state.stdout.len().saturating_sub(stdout_offset as usize));
    let stderr_budget = max_bytes.saturating_sub(stdout_budget);
    let stdout = stream_page(
        &state.stdout,
        state.stdout_produced,
        state.stdout_closed,
        stdout_offset,
        stdout_budget,
    );
    let stderr = stream_page(
        &state.stderr,
        state.stderr_produced,
        state.stderr_closed,
        stderr_offset,
        stderr_budget,
    );
    let (state_label, terminal, termination) = match state.lifecycle {
        Lifecycle::Pending => ("pending", false, None),
        Lifecycle::Running => ("running", false, None),
        Lifecycle::Terminal(cause) => ("terminal", true, Some(cause.label())),
    };
    let ended_at = state.finished_at.unwrap_or_else(Instant::now);
    Ok(ProcessSnapshot {
        state: state_label,
        terminal,
        termination,
        exit_code: state.exit_code,
        signal: state.signal,
        elapsed_ms: ended_at
            .saturating_duration_since(state.started_at)
            .as_millis()
            .min(u64::MAX as u128) as u64,
        stdout,
        stderr,
    })
}

fn stream_page(
    bytes: &[u8],
    produced_bytes: u64,
    closed: bool,
    offset: u64,
    max_bytes: usize,
) -> ProcessStreamPage {
    let start = offset as usize;
    let end = start.saturating_add(max_bytes).min(bytes.len());
    let next_offset = end as u64;
    ProcessStreamPage {
        offset,
        next_offset,
        captured_bytes: bytes.len() as u64,
        produced_bytes,
        text: String::from_utf8_lossy(&bytes[start..end]).into_owned(),
        more_available: end < bytes.len(),
        complete: closed && end == bytes.len(),
        truncated: produced_bytes > bytes.len() as u64,
    }
}
