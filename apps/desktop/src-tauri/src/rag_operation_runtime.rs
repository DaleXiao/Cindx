use crate::desktop_event_sink::DesktopEventSink;
use crate::view_models::RagOperationProgress;
use agent_rag::RAG_INDEX_CANCELLED;
use agent_runtime::{AgentRunControl, RunBudget};
use model_provider::MODEL_REQUEST_CANCELLED;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_ACTIVE_RAG_OPERATIONS: usize = 8;
const MAX_RAG_OPERATION_ID_BYTES: usize = 160;
const RAG_PROVIDER_TIMEOUT: Duration = Duration::from_secs(180);
const RAG_OPERATION_MAX_DURATION: Duration = Duration::from_secs(365 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RagOperationPhase {
    Cancellable,
    CommitWindow,
    Cancelled,
    Finalized,
}

pub(crate) struct RagOperationControl {
    agent: Arc<AgentRunControl>,
    phase: Mutex<RagOperationPhase>,
}

impl RagOperationControl {
    fn new() -> Self {
        Self {
            agent: Arc::new(AgentRunControl::with_budget(rag_operation_budget())),
            phase: Mutex::new(RagOperationPhase::Cancellable),
        }
    }

    pub(crate) fn agent(&self) -> &Arc<AgentRunControl> {
        &self.agent
    }

    pub(crate) fn should_cancel(&self) -> bool {
        *self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            == RagOperationPhase::Cancelled
    }

    pub(crate) fn checkpoint(&self) -> Result<(), String> {
        if self.should_cancel() {
            Err(MODEL_REQUEST_CANCELLED.to_string())
        } else {
            Ok(())
        }
    }

    pub(crate) fn request_cancel(&self) -> bool {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *phase != RagOperationPhase::Cancellable {
            return false;
        }
        *phase = RagOperationPhase::Cancelled;
        self.agent.request_cancel();
        true
    }

    pub(crate) fn begin_commit_window(&self) -> Result<RagCommitWindow<'_>, String> {
        let mut phase = self
            .phase
            .lock()
            .map_err(|error| format!("RAG operation control lock poisoned: {error}"))?;
        match *phase {
            RagOperationPhase::Cancellable => {}
            RagOperationPhase::Finalized => {
                return Err("RAG operation is already finalized".to_string())
            }
            RagOperationPhase::Cancelled => return Err(MODEL_REQUEST_CANCELLED.to_string()),
            RagOperationPhase::CommitWindow => {
                return Err("RAG operation commit window is already active".to_string())
            }
        }
        *phase = RagOperationPhase::CommitWindow;
        drop(phase);
        Ok(RagCommitWindow {
            control: self,
            completed: false,
        })
    }
}

fn rag_operation_budget() -> RunBudget {
    let practical_limit = usize::MAX / 4;
    let mut budget = RunBudget::for_effort("auto");
    budget.max_duration = RAG_OPERATION_MAX_DURATION;
    budget.model_call_timeout = RAG_PROVIDER_TIMEOUT;
    budget.tool_call_timeout = RAG_PROVIDER_TIMEOUT;
    budget.initial_model_calls = practical_limit;
    budget.max_model_calls = practical_limit;
    budget.model_calls_per_extension = 1;
    budget.initial_tool_calls = practical_limit;
    budget.max_tool_calls = practical_limit;
    budget.tool_calls_per_extension = 1;
    budget.no_progress_timeout = RAG_OPERATION_MAX_DURATION;
    budget.max_identical_actions = practical_limit;
    budget.initial_agent_turns = practical_limit;
    budget.max_agent_turns = practical_limit;
    budget.agent_turns_per_extension = 1;
    budget.max_repair_attempts = practical_limit;
    budget.terminal_model_call_reserve = 0;
    budget.terminal_time_reserve = RAG_PROVIDER_TIMEOUT;
    budget.max_total_tokens = u64::MAX / 2;
    budget.max_physical_model_attempts = usize::MAX / 2;
    budget.terminal_token_reserve = 0;
    budget.terminal_physical_model_attempt_reserve = 0;
    budget
}

pub(crate) struct RagCommitWindow<'a> {
    control: &'a RagOperationControl,
    completed: bool,
}

impl RagCommitWindow<'_> {
    pub(crate) fn resume(mut self) {
        self.complete(RagOperationPhase::Cancellable);
    }

    pub(crate) fn finalize(mut self) {
        self.complete(RagOperationPhase::Finalized);
    }

    fn complete(&mut self, next_phase: RagOperationPhase) {
        *self
            .control
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next_phase;
        self.completed = true;
    }
}

impl Drop for RagCommitWindow<'_> {
    fn drop(&mut self) {
        if !self.completed {
            *self
                .control
                .phase
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = RagOperationPhase::Cancellable;
        }
    }
}

pub(crate) struct RagOperationProgressReporter<'a> {
    events: &'a dyn DesktopEventSink,
    operation_id: String,
    operation_kind: String,
    completed_steps: usize,
    total_steps: usize,
    terminal: bool,
}

impl<'a> RagOperationProgressReporter<'a> {
    fn new(
        events: &'a dyn DesktopEventSink,
        operation_id: &str,
        operation_kind: &str,
        total_steps: usize,
    ) -> Self {
        Self {
            events,
            operation_id: operation_id.to_string(),
            operation_kind: operation_kind.to_string(),
            completed_steps: 0,
            total_steps: total_steps.max(1),
            terminal: false,
        }
    }

    pub(crate) fn advance(
        &mut self,
        stage: impl Into<String>,
        completed_steps: usize,
        detail: impl Into<String>,
    ) {
        if self.terminal {
            return;
        }
        self.completed_steps = self
            .completed_steps
            .max(completed_steps.min(self.total_steps));
        self.publish_progress(stage, detail, "running");
    }

    fn finish(&mut self, status: &str, detail: impl Into<String>) {
        if self.terminal {
            return;
        }
        if status == "completed" {
            self.completed_steps = self.total_steps;
        }
        self.terminal = true;
        self.publish_progress(status, detail, status);
    }

    fn publish_progress(&self, stage: impl Into<String>, detail: impl Into<String>, status: &str) {
        self.events
            .emit_rag_operation_progress(RagOperationProgress {
                operation_id: self.operation_id.clone(),
                operation_kind: self.operation_kind.clone(),
                stage: stage.into(),
                completed_steps: self.completed_steps,
                total_steps: self.total_steps,
                detail: detail.into(),
                status: status.to_string(),
            });
    }
}

struct RagOperationLease<'a> {
    controls: &'a Mutex<BTreeMap<String, Arc<RagOperationControl>>>,
    operation_id: String,
    control: Arc<RagOperationControl>,
}

impl<'a> RagOperationLease<'a> {
    fn register(
        controls: &'a Mutex<BTreeMap<String, Arc<RagOperationControl>>>,
        operation_id: &str,
    ) -> Result<Self, String> {
        validate_rag_operation_id(operation_id)?;
        let control = Arc::new(RagOperationControl::new());
        let mut active = controls
            .lock()
            .map_err(|error| format!("RAG operation control lock poisoned: {error}"))?;
        if active.contains_key(operation_id) {
            return Err("RAG operation is already active".to_string());
        }
        if active.len() >= MAX_ACTIVE_RAG_OPERATIONS {
            return Err("too many RAG operations are active".to_string());
        }
        active.insert(operation_id.to_string(), Arc::clone(&control));
        drop(active);
        Ok(Self {
            controls,
            operation_id: operation_id.to_string(),
            control,
        })
    }

    fn control(&self) -> Arc<RagOperationControl> {
        Arc::clone(&self.control)
    }
}

impl Drop for RagOperationLease<'_> {
    fn drop(&mut self) {
        let mut active = self
            .controls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .get(&self.operation_id)
            .is_some_and(|current| Arc::ptr_eq(current, &self.control))
        {
            active.remove(&self.operation_id);
        }
    }
}

pub(crate) fn run_rag_operation<T>(
    controls: &Mutex<BTreeMap<String, Arc<RagOperationControl>>>,
    events: &dyn DesktopEventSink,
    operation_id: &str,
    operation_kind: &str,
    total_steps: usize,
    worker: impl FnOnce(
        &Arc<RagOperationControl>,
        &mut RagOperationProgressReporter<'_>,
    ) -> Result<T, String>,
    output_error: impl FnOnce(&T) -> Option<String>,
) -> Result<T, String> {
    let lease = RagOperationLease::register(controls, operation_id)?;
    let control = lease.control();
    let mut progress =
        RagOperationProgressReporter::new(events, operation_id, operation_kind, total_steps);
    progress.advance("preparing", 0, "Preparing RAG operation");

    let result = if control.should_cancel() {
        Err(MODEL_REQUEST_CANCELLED.to_string())
    } else {
        worker(&control, &mut progress)
    };
    match &result {
        Ok(output) => {
            if let Some(error) = output_error(output) {
                let status = if rag_operation_was_cancelled(&control, &error) {
                    "cancelled"
                } else {
                    "failed"
                };
                progress.finish(status, error);
            } else {
                progress.finish("completed", "RAG operation completed");
            }
        }
        Err(error) => {
            let status = if rag_operation_was_cancelled(&control, error) {
                "cancelled"
            } else {
                "failed"
            };
            progress.finish(status, error.clone());
        }
    }
    result
}

pub(crate) fn cancel_rag_operation_control(
    controls: &Mutex<BTreeMap<String, Arc<RagOperationControl>>>,
    operation_id: &str,
) -> Result<bool, String> {
    validate_rag_operation_id(operation_id)?;
    let control = controls
        .lock()
        .map_err(|error| format!("RAG operation control lock poisoned: {error}"))?
        .get(operation_id)
        .cloned();
    Ok(control.is_some_and(|control| control.request_cancel()))
}

pub(crate) fn rag_operation_checkpoint(control: &Arc<RagOperationControl>) -> Result<(), String> {
    control.checkpoint()
}

pub(crate) fn with_cancellable_mutex<T, R>(
    mutex: &Mutex<T>,
    control: &RagOperationControl,
    lock_label: &str,
    operation: impl FnOnce(&mut T) -> Result<R, String>,
) -> Result<R, String> {
    with_cancellable_mutex_mode(
        mutex,
        control,
        lock_label,
        MutexCommitMode::Resume,
        || {},
        operation,
    )
}

pub(crate) fn with_finalizing_cancellable_mutex<T, R>(
    mutex: &Mutex<T>,
    control: &RagOperationControl,
    lock_label: &str,
    operation: impl FnOnce(&mut T) -> Result<R, String>,
) -> Result<R, String> {
    with_cancellable_mutex_mode(
        mutex,
        control,
        lock_label,
        MutexCommitMode::Finalize,
        || {},
        operation,
    )
}

#[derive(Clone, Copy)]
enum MutexCommitMode {
    Resume,
    Finalize,
}

fn with_cancellable_mutex_mode<T, R>(
    mutex: &Mutex<T>,
    control: &RagOperationControl,
    lock_label: &str,
    mode: MutexCommitMode,
    mut on_wait: impl FnMut(),
    operation: impl FnOnce(&mut T) -> Result<R, String>,
) -> Result<R, String> {
    let mut guard = loop {
        control.checkpoint()?;
        match mutex.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                on_wait();
                std::thread::park_timeout(Duration::from_millis(20));
            }
            Err(std::sync::TryLockError::Poisoned(error)) => {
                return Err(format!("{lock_label} lock poisoned: {error}"));
            }
        }
    };
    let commit_window = control.begin_commit_window()?;
    let result = operation(&mut *guard);
    match mode {
        MutexCommitMode::Resume => commit_window.resume(),
        MutexCommitMode::Finalize if result.is_ok() => commit_window.finalize(),
        MutexCommitMode::Finalize => commit_window.resume(),
    }
    result
}

fn validate_rag_operation_id(operation_id: &str) -> Result<(), String> {
    if operation_id.is_empty()
        || operation_id.len() > MAX_RAG_OPERATION_ID_BYTES
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("invalid RAG operation id".to_string());
    }
    Ok(())
}

fn is_rag_cancellation_error(error: &str) -> bool {
    error == MODEL_REQUEST_CANCELLED || error == RAG_INDEX_CANCELLED
}

pub(crate) fn rag_operation_was_cancelled(control: &RagOperationControl, error: &str) -> bool {
    control.should_cancel() || is_rag_cancellation_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<RagOperationProgress>>,
    }

    impl DesktopEventSink for RecordingSink {
        fn emit_model_stream_delta(&self, _payload: crate::view_models::ModelStreamDelta) {}

        fn emit_rag_operation_progress(&self, payload: RagOperationProgress) {
            self.events
                .lock()
                .expect("recording sink lock should remain available")
                .push(payload);
        }

        fn emit_session_title_updated(&self, _session_id: String) {}
    }

    #[test]
    fn progress_is_monotonic_bounded_and_success_preserves_output() {
        let controls = Mutex::new(BTreeMap::new());
        let sink = RecordingSink::default();
        let output = run_rag_operation(
            &controls,
            &sink,
            "operation-1",
            "search",
            4,
            |_control, progress| {
                progress.advance("retrieving", 2, "Retrieving sources");
                progress.advance("stale", 1, "Late update");
                progress.advance("projecting", 99, "Projecting result");
                Ok("unchanged output".to_string())
            },
            |_| None,
        )
        .expect("operation should succeed");

        assert_eq!(output, "unchanged output");
        let events = sink
            .events
            .lock()
            .expect("recording sink lock should remain available");
        assert!(events
            .windows(2)
            .all(|pair| pair[0].completed_steps <= pair[1].completed_steps));
        assert!(events
            .iter()
            .all(|event| event.completed_steps <= event.total_steps));
        assert_eq!(
            events.last().map(|event| event.status.as_str()),
            Some("completed")
        );
        assert_eq!(events.last().map(|event| event.completed_steps), Some(4));
        assert!(controls
            .lock()
            .expect("control map lock should remain available")
            .is_empty());
    }

    #[test]
    fn cancelled_control_skips_worker_before_work_starts() {
        let controls = Mutex::new(BTreeMap::new());
        let lease = RagOperationLease::register(&controls, "operation-2")
            .expect("operation should register");
        let control = lease.control();
        assert!(control.request_cancel());
        let worker_called = AtomicBool::new(false);

        let result = if rag_operation_checkpoint(&control).is_ok() {
            worker_called.store(true, Ordering::SeqCst);
            Ok(())
        } else {
            Err(MODEL_REQUEST_CANCELLED.to_string())
        };

        assert_eq!(result, Err(MODEL_REQUEST_CANCELLED.to_string()));
        assert!(!worker_called.load(Ordering::SeqCst));
        drop(lease);
        assert!(controls
            .lock()
            .expect("control map lock should remain available")
            .is_empty());
    }

    #[test]
    fn cancellation_checkpoint_stops_later_work_and_cleans_registration() {
        let controls = Mutex::new(BTreeMap::new());
        let sink = RecordingSink::default();
        let later_work = AtomicBool::new(false);

        let result = run_rag_operation(
            &controls,
            &sink,
            "operation-3",
            "index",
            4,
            |control, progress| {
                progress.advance("indexing", 1, "Workspace scanned");
                control.request_cancel();
                rag_operation_checkpoint(control)?;
                later_work.store(true, Ordering::SeqCst);
                Ok(())
            },
            |_| None,
        );

        assert_eq!(result, Err(MODEL_REQUEST_CANCELLED.to_string()));
        assert!(!later_work.load(Ordering::SeqCst));
        assert_eq!(
            sink.events
                .lock()
                .expect("recording sink lock should remain available")
                .last()
                .map(|event| event.status.as_str()),
            Some("cancelled")
        );
        assert!(controls
            .lock()
            .expect("control map lock should remain available")
            .is_empty());
    }

    #[test]
    fn failure_cleans_registration_and_allows_same_id_to_run_again() {
        let controls = Mutex::new(BTreeMap::new());
        let sink = RecordingSink::default();
        let failed = run_rag_operation::<()>(
            &controls,
            &sink,
            "operation-4",
            "answer",
            5,
            |_control, _progress| Err("provider unavailable".to_string()),
            |_| None,
        );
        assert_eq!(failed, Err("provider unavailable".to_string()));

        let retried = run_rag_operation(
            &controls,
            &sink,
            "operation-4",
            "answer",
            5,
            |_control, _progress| Ok(7_u8),
            |_| None,
        );
        assert_eq!(retried, Ok(7));
        assert!(controls
            .lock()
            .expect("control map lock should remain available")
            .is_empty());
    }

    #[test]
    fn operation_budget_exceeds_auto_embedding_capacity_and_keeps_provider_window() {
        let budget = rag_operation_budget();
        assert_eq!(budget.model_call_timeout, RAG_PROVIDER_TIMEOUT);
        assert_eq!(budget.terminal_time_reserve, RAG_PROVIDER_TIMEOUT);
        assert_eq!(
            budget
                .stage_budget(agent_runtime::RunStageClass::Finalizer)
                .max_duration,
            RAG_PROVIDER_TIMEOUT
        );

        let control = RagOperationControl::new();
        for expected_call in 1..=96 {
            assert_eq!(
                control
                    .agent()
                    .begin_model_call("manual_rag_embedding")
                    .expect("manual RAG should not inherit the Auto 72-call cap"),
                expected_call
            );
            control.agent().finish_model_call();
        }
        assert_eq!(control.agent().model_call_timeout_seconds(), 180);
        control
            .agent()
            .begin_stage_model_call("rag_answer", agent_runtime::RunStageClass::Finalizer)
            .expect("embedding volume must not consume the answer window");
        assert!(
            control
                .agent()
                .stage_model_call_timeout_seconds(agent_runtime::RunStageClass::Finalizer)
                >= 179
        );
        control.agent().finish_model_call();
        assert!(!control.should_cancel());
    }

    #[test]
    fn commit_windows_linearize_resume_and_finalization_against_cancellation() {
        let resumable = RagOperationControl::new();
        let auto_index_commit = resumable
            .begin_commit_window()
            .expect("auto-index commit should begin");
        assert!(!resumable.request_cancel());
        assert!(!resumable.agent().should_stop());
        auto_index_commit.resume();
        assert!(resumable.request_cancel());
        assert!(resumable.agent().should_stop());
        assert!(!resumable.request_cancel());

        let finalized = RagOperationControl::new();
        let direct_index_commit = finalized
            .begin_commit_window()
            .expect("direct-index commit should begin");
        assert!(!finalized.request_cancel());
        direct_index_commit.finalize();
        assert!(!finalized.request_cancel());
        assert!(finalized.checkpoint().is_ok());
        assert!(finalized.begin_commit_window().is_err());
    }

    #[test]
    fn cancellation_while_waiting_for_mutex_cannot_write_after_acquire() {
        let values = Arc::new(Mutex::new(Vec::<u8>::new()));
        let held = values.lock().expect("fixture mutex should lock");
        let worker_values = Arc::clone(&values);
        let control = Arc::new(RagOperationControl::new());
        let worker_control = Arc::clone(&control);
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();

        let worker = std::thread::spawn(move || {
            let mut waiting_tx = Some(waiting_tx);
            with_cancellable_mutex_mode(
                &worker_values,
                &worker_control,
                "fixture",
                MutexCommitMode::Resume,
                || {
                    if let Some(waiting_tx) = waiting_tx.take() {
                        waiting_tx
                            .send(())
                            .expect("test should still observe mutex wait");
                    }
                },
                |values| {
                    values.push(1);
                    Ok(())
                },
            )
        });

        waiting_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should wait on the held mutex");
        assert!(control.request_cancel());
        drop(held);
        assert_eq!(
            worker.join().expect("worker should join"),
            Err(MODEL_REQUEST_CANCELLED.to_string())
        );
        assert!(values
            .lock()
            .expect("fixture mutex should relock")
            .is_empty());
    }

    #[test]
    fn provider_and_user_cancellation_share_terminal_classification() {
        let provider_cancel = RagOperationControl::new();
        assert!(rag_operation_was_cancelled(
            &provider_cancel,
            MODEL_REQUEST_CANCELLED
        ));
        assert!(rag_operation_was_cancelled(
            &provider_cancel,
            RAG_INDEX_CANCELLED
        ));
        assert!(!rag_operation_was_cancelled(
            &provider_cancel,
            "provider unavailable"
        ));

        let user_cancel = RagOperationControl::new();
        assert!(user_cancel.request_cancel());
        assert!(rag_operation_was_cancelled(
            &user_cancel,
            "provider unavailable"
        ));
    }
}
