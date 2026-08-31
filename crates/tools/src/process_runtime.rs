use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use agent_core::{Metadata, SandboxMode, TaskId, ToolInvocation};

use crate::process_capture::{ProcessCapture, ProcessSnapshot, ProcessStopCause};
use crate::process_supervisor::{spawn_supervised_process, ProcessInputRequest, SupervisedProcess};
use crate::ToolExecutionControl;

const MAX_ACTIVE_PROCESSES: usize = 4;
const MAX_ACTIVE_PROCESSES_PER_OWNER: usize = 2;
const MAX_TERMINAL_RECORDS: usize = 16;
const TERMINAL_TTL: Duration = Duration::from_secs(120);
const PENDING_TTL: Duration = Duration::from_secs(30);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);
const MAX_PROCESS_INPUT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessBudgets {
    pub(crate) timeout_seconds: u64,
    pub(crate) cpu_seconds: u64,
    pub(crate) output_limit_bytes: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedProcessSnapshot {
    pub(crate) process_id: String,
    pub(crate) generation: u64,
    pub(crate) start_call_id: String,
    pub(crate) cwd: String,
    pub(crate) budgets: ProcessBudgets,
    pub(crate) snapshot: ProcessSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessManagerError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

impl ProcessManagerError {
    fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessOwner {
    task_id: String,
    project_id: String,
    session_id: String,
    agent_run_id: String,
    collaboration_id: String,
    steer_epoch: String,
    contract_epoch: String,
}

impl ProcessOwner {
    fn from_invocation(invocation: &ToolInvocation) -> Self {
        let metadata = &invocation.metadata;
        Self {
            task_id: invocation.task_id.0.clone(),
            project_id: metadata.get("project_id").cloned().unwrap_or_default(),
            session_id: metadata.get("session_id").cloned().unwrap_or_default(),
            agent_run_id: metadata.get("agent_run_id").cloned().unwrap_or_default(),
            collaboration_id: metadata
                .get("collaboration_id")
                .cloned()
                .unwrap_or_default(),
            steer_epoch: metadata.get("steer_epoch").cloned().unwrap_or_default(),
            contract_epoch: metadata
                .get("prompt_contract_epoch")
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn matches_run(&self, task_id: &TaskId, run_context: &Metadata) -> bool {
        self.task_id == task_id.0
            && metadata_matches(run_context, "project_id", &self.project_id)
            && metadata_matches(run_context, "session_id", &self.session_id)
            && metadata_matches(run_context, "agent_run_id", &self.agent_run_id)
    }
}

fn metadata_matches(metadata: &Metadata, key: &str, expected: &str) -> bool {
    !expected.is_empty() && metadata.get(key).is_some_and(|actual| actual == expected)
}

struct PendingLaunch {
    command: String,
    cwd: PathBuf,
    control: ToolExecutionControl,
    sandbox: SandboxMode,
    workspace_root: PathBuf,
}

struct ProcessEntry {
    process_id: String,
    generation: u64,
    owner: ProcessOwner,
    start_call_id: String,
    cwd: String,
    budgets: ProcessBudgets,
    created_at: Instant,
    capture: Arc<ProcessCapture>,
    launch: Mutex<Option<PendingLaunch>>,
    stop_sender: Mutex<Option<mpsc::Sender<ProcessStopCause>>>,
    input_sender: Arc<Mutex<Option<mpsc::SyncSender<ProcessInputRequest>>>>,
    monitor: Mutex<Option<JoinHandle<()>>>,
    input_bytes: AtomicU64,
    #[cfg(test)]
    activation_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct ProcessManagerInner {
    entries: Mutex<BTreeMap<String, Arc<ProcessEntry>>>,
    generation: AtomicU64,
    #[cfg(test)]
    activation_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

pub struct ProcessManager {
    inner: Arc<ProcessManagerInner>,
}

impl ProcessManager {
    pub fn new() -> Self {
        let inner = Arc::new(ProcessManagerInner {
            entries: Mutex::new(BTreeMap::new()),
            generation: AtomicU64::new(0),
            #[cfg(test)]
            activation_hook: Mutex::new(None),
        });
        spawn_reaper(Arc::downgrade(&inner));
        Self { inner }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reserve(
        &self,
        invocation: &ToolInvocation,
        command: String,
        cwd: PathBuf,
        cwd_label: String,
        budgets: ProcessBudgets,
        control: ToolExecutionControl,
        sandbox: SandboxMode,
        workspace_root: PathBuf,
    ) -> Result<ManagedProcessSnapshot, ProcessManagerError> {
        self.prune();
        let owner = ProcessOwner::from_invocation(invocation);
        let mut entries = self.inner.entries.lock().map_err(|_| lock_error())?;
        let active = entries
            .values()
            .filter(|entry| !entry.capture.is_terminal())
            .count();
        if active >= MAX_ACTIVE_PROCESSES {
            return Err(ProcessManagerError::new(
                "process_concurrency_limit",
                format!("at most {MAX_ACTIVE_PROCESSES} process sessions may be active"),
                true,
            ));
        }
        let owner_active = entries
            .values()
            .filter(|entry| entry.owner == owner && !entry.capture.is_terminal())
            .count();
        if owner_active >= MAX_ACTIVE_PROCESSES_PER_OWNER {
            return Err(ProcessManagerError::new(
                "process_owner_concurrency_limit",
                format!(
                    "at most {MAX_ACTIVE_PROCESSES_PER_OWNER} process sessions may be active for one run"
                ),
                true,
            ));
        }
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let process_id = opaque_process_id()?;
        let capture = Arc::new(ProcessCapture::pending(budgets.output_limit_bytes));
        let entry = Arc::new(ProcessEntry {
            process_id: process_id.clone(),
            generation,
            owner,
            start_call_id: invocation.id.0.clone(),
            cwd: cwd_label,
            budgets,
            created_at: Instant::now(),
            capture,
            launch: Mutex::new(Some(PendingLaunch {
                command,
                cwd,
                control,
                sandbox,
                workspace_root,
            })),
            stop_sender: Mutex::new(None),
            input_sender: Arc::new(Mutex::new(None)),
            monitor: Mutex::new(None),
            input_bytes: AtomicU64::new(0),
            #[cfg(test)]
            activation_hook: self
                .inner
                .activation_hook
                .lock()
                .ok()
                .and_then(|hook| hook.clone()),
        });
        let snapshot = entry_snapshot(&entry, 0, 0, 0, Duration::ZERO)?;
        entries.insert(process_id, entry);
        Ok(snapshot)
    }

    pub(crate) fn poll(
        &self,
        invocation: &ToolInvocation,
        process_id: &str,
        stdout_offset: u64,
        stderr_offset: u64,
        max_bytes: usize,
        wait: Duration,
    ) -> Result<ManagedProcessSnapshot, ProcessManagerError> {
        self.prune();
        let entry = self.entry_for(invocation, process_id)?;
        activate_entry(&entry);
        entry_snapshot(&entry, stdout_offset, stderr_offset, max_bytes, wait)
    }

    pub(crate) fn input(
        &self,
        invocation: &ToolInvocation,
        process_id: &str,
        bytes: Vec<u8>,
        close: bool,
    ) -> Result<usize, ProcessManagerError> {
        self.prune();
        let entry = self.entry_for(invocation, process_id)?;
        activate_entry(&entry);
        if entry.capture.is_terminal() {
            return Err(ProcessManagerError::new(
                "process_not_running",
                "the process is already terminal",
                false,
            ));
        }
        let requested = bytes.len() as u64;
        entry
            .input_bytes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(requested)
                    .filter(|next| *next <= MAX_PROCESS_INPUT_BYTES)
            })
            .map_err(|_| {
                ProcessManagerError::new(
                    "process_input_limit",
                    format!("process input is capped at {MAX_PROCESS_INPUT_BYTES} bytes"),
                    false,
                )
            })?;
        let (acknowledgement, receiver) = mpsc::channel();
        let request = ProcessInputRequest {
            bytes,
            close,
            acknowledgement,
        };
        let send_result = entry
            .input_sender
            .lock()
            .map_err(|_| lock_error())?
            .as_ref()
            .ok_or_else(|| {
                ProcessManagerError::new("process_stdin_closed", "process stdin is closed", false)
            })?
            .try_send(request);
        if let Err(error) = send_result {
            entry.input_bytes.fetch_sub(requested, Ordering::SeqCst);
            let (code, retryable) = match error {
                mpsc::TrySendError::Full(_) => ("process_input_backpressure", true),
                mpsc::TrySendError::Disconnected(_) => ("process_stdin_closed", false),
            };
            return Err(ProcessManagerError::new(
                code,
                "process input queue is unavailable",
                retryable,
            ));
        }
        match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(written)) => Ok(written),
            Ok(Err(error)) => Err(ProcessManagerError::new(
                "process_input_failed",
                error,
                false,
            )),
            Err(_) => Err(ProcessManagerError::new(
                "process_input_unknown",
                "process input acknowledgement timed out; do not repeat the same input",
                false,
            )),
        }
    }

    pub(crate) fn terminate(
        &self,
        invocation: &ToolInvocation,
        process_id: &str,
    ) -> Result<ManagedProcessSnapshot, ProcessManagerError> {
        self.prune();
        let entry = self.entry_for(invocation, process_id)?;
        let mut launch = entry.launch.lock().map_err(|_| lock_error())?;
        if entry.capture.is_pending() {
            launch.take();
            finish_pending(&entry, ProcessStopCause::Terminated);
        } else {
            drop(launch);
            if !entry.capture.is_terminal() {
                send_stop(&entry, ProcessStopCause::Terminated);
            }
        }
        entry_snapshot(&entry, 0, 0, 64 * 1024, Duration::from_secs(2))
    }

    pub fn shutdown_all(&self) {
        let entries = self
            .inner
            .entries
            .lock()
            .map(|entries| entries.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        shutdown_entries(&entries, ProcessStopCause::AppExit);
    }

    pub fn shutdown_run(&self, task_id: &TaskId, run_context: &Metadata) {
        let entries = self
            .inner
            .entries
            .lock()
            .map(|entries| {
                entries
                    .values()
                    .filter(|entry| entry.owner.matches_run(task_id, run_context))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        shutdown_entries(&entries, ProcessStopCause::RunEnded);
    }

    pub fn shutdown_sessions(&self, session_ids: &[String]) {
        let session_ids = session_ids.iter().map(String::as_str).collect::<Vec<_>>();
        let entries = self
            .inner
            .entries
            .lock()
            .map(|entries| {
                entries
                    .values()
                    .filter(|entry| session_ids.contains(&entry.owner.session_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        shutdown_entries(&entries, ProcessStopCause::SessionRemoved);
    }

    #[cfg(test)]
    pub(crate) fn set_activation_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.inner.activation_hook.lock() {
            *slot = Some(hook);
        }
    }

    fn entry_for(
        &self,
        invocation: &ToolInvocation,
        process_id: &str,
    ) -> Result<Arc<ProcessEntry>, ProcessManagerError> {
        let owner = ProcessOwner::from_invocation(invocation);
        self.inner
            .entries
            .lock()
            .map_err(|_| lock_error())?
            .get(process_id)
            .filter(|entry| entry.owner == owner)
            .cloned()
            .ok_or_else(|| {
                ProcessManagerError::new(
                    "process_not_found",
                    "process handle is missing, expired, or belongs to another run",
                    false,
                )
            })
    }

    fn prune(&self) {
        prune_inner(&self.inner);
    }
}

fn shutdown_entries(entries: &[Arc<ProcessEntry>], cause: ProcessStopCause) {
    for entry in entries {
        let finished_pending = entry.launch.lock().is_ok_and(|mut launch| {
            if entry.capture.is_pending() {
                launch.take();
                finish_pending(entry, cause);
                true
            } else {
                false
            }
        });
        if !finished_pending && !entry.capture.is_terminal() {
            send_stop(entry, cause);
        }
    }
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    while Instant::now() < deadline && entries.iter().any(|entry| !entry.capture.is_terminal()) {
        thread::sleep(Duration::from_millis(20));
    }
    for entry in entries {
        join_monitor(entry);
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

fn activate_entry(entry: &Arc<ProcessEntry>) {
    let Ok(mut launch_slot) = entry.launch.lock() else {
        return;
    };
    if !entry.capture.is_pending() {
        return;
    }
    let Some(launch) = launch_slot.take() else {
        return;
    };
    #[cfg(test)]
    if let Some(hook) = entry.activation_hook.as_ref() {
        hook();
    }
    if launch.control.should_cancel() {
        finish_pending(entry, ProcessStopCause::Cancelled);
        return;
    }
    match spawn_supervised_process(
        &launch.command,
        &launch.cwd,
        entry.budgets,
        launch.control,
        Arc::clone(&entry.capture),
        Arc::clone(&entry.input_sender),
        launch.sandbox,
        &launch.workspace_root,
    ) {
        Ok(SupervisedProcess {
            stop_sender,
            monitor,
        }) => {
            if let Ok(mut slot) = entry.stop_sender.lock() {
                *slot = Some(stop_sender);
            }
            if let Ok(mut slot) = entry.monitor.lock() {
                *slot = Some(monitor);
            }
        }
        Err(error) => {
            entry.capture.append_stderr(error.as_bytes());
            entry.capture.mark_stream_closed(true);
            entry.capture.mark_stream_closed(false);
            entry
                .capture
                .finish(ProcessStopCause::LaunchFailed, None, None);
        }
    }
}

fn finish_pending(entry: &ProcessEntry, cause: ProcessStopCause) {
    entry.capture.mark_stream_closed(true);
    entry.capture.mark_stream_closed(false);
    entry.capture.finish(cause, None, None);
}

fn entry_snapshot(
    entry: &ProcessEntry,
    stdout_offset: u64,
    stderr_offset: u64,
    max_bytes: usize,
    wait: Duration,
) -> Result<ManagedProcessSnapshot, ProcessManagerError> {
    let snapshot = entry
        .capture
        .wait_and_snapshot(stdout_offset, stderr_offset, max_bytes, wait)
        .map_err(|message| ProcessManagerError::new("process_poll_invalid", message, false))?;
    Ok(ManagedProcessSnapshot {
        process_id: entry.process_id.clone(),
        generation: entry.generation,
        start_call_id: entry.start_call_id.clone(),
        cwd: entry.cwd.clone(),
        budgets: entry.budgets,
        snapshot,
    })
}

fn send_stop(entry: &ProcessEntry, cause: ProcessStopCause) {
    if let Ok(sender) = entry.stop_sender.lock() {
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(cause);
        }
    }
}

fn join_monitor(entry: &ProcessEntry) {
    if let Ok(mut monitor) = entry.monitor.lock() {
        if let Some(monitor) = monitor.take() {
            let _ = monitor.join();
        }
    }
}

fn prune_inner(inner: &ProcessManagerInner) {
    let removed = {
        let Ok(mut entries) = inner.entries.lock() else {
            return;
        };
        for entry in entries.values() {
            if entry.capture.is_pending() {
                let Ok(mut launch) = entry.launch.lock() else {
                    continue;
                };
                if !entry.capture.is_pending() {
                    continue;
                }
                let cancelled = launch
                    .as_ref()
                    .is_none_or(|launch| launch.control.should_cancel());
                if cancelled || entry.created_at.elapsed() >= PENDING_TTL {
                    launch.take();
                    finish_pending(
                        entry,
                        if cancelled {
                            ProcessStopCause::Cancelled
                        } else {
                            ProcessStopCause::Timeout
                        },
                    );
                }
            }
        }
        let mut terminal = entries
            .values()
            .filter(|entry| entry.capture.is_terminal())
            .map(|entry| (entry.generation, entry.process_id.clone()))
            .collect::<Vec<_>>();
        terminal.sort_by_key(|(generation, _)| *generation);
        let excess = terminal.len().saturating_sub(MAX_TERMINAL_RECORDS);
        let mut expired = terminal
            .iter()
            .filter(|(_, id)| {
                entries
                    .get(id)
                    .and_then(|entry| entry.capture.finished_age())
                    .is_some_and(|age| age >= TERMINAL_TTL)
            })
            .map(|(_, id)| id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        expired.extend(terminal.iter().take(excess).map(|(_, id)| id.clone()));
        expired
            .into_iter()
            .filter_map(|id| entries.remove(&id))
            .collect::<Vec<_>>()
    };
    for entry in removed {
        join_monitor(&entry);
    }
}

fn spawn_reaper(inner: Weak<ProcessManagerInner>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(200));
        let Some(inner) = inner.upgrade() else {
            return;
        };
        prune_inner(&inner);
    });
}

fn opaque_process_id() -> Result<String, ProcessManagerError> {
    let mut bytes = [0u8; 18];
    getrandom::fill(&mut bytes).map_err(|error| {
        ProcessManagerError::new(
            "process_identity_unavailable",
            format!("failed to create an opaque process handle: {error}"),
            true,
        )
    })?;
    Ok(format!(
        "proc_{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn lock_error() -> ProcessManagerError {
    ProcessManagerError::new(
        "process_manager_unavailable",
        "process manager lock poisoned",
        true,
    )
}
