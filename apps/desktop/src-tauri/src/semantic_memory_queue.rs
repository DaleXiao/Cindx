use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::runtime_values::{current_time_millis, phase16_task_id};
use crate::semantic_memory_runtime::recover_semantic_memory_without_model;
use agent_application::{
    ClaimedWork, LossAwareWorkQueue, WorkEnqueueOutcome, WorkQueueMetrics, WorkQueuePolicy,
    WorkRetryOutcome,
};
use agent_core::{EventKind, Metadata};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;
use tauri::Manager;

const SEMANTIC_MEMORY_QUEUE_LIMIT: usize = 128;
const SEMANTIC_MEMORY_MAX_ATTEMPTS: u32 = 4;
const SEMANTIC_MEMORY_RETRY_BASE_MS: u64 = 500;
const SEMANTIC_MEMORY_RETRY_MAX_MS: u64 = 8_000;

pub(crate) struct SemanticMemoryJob {
    pub(crate) workspace_root: PathBuf,
    pub(crate) config: ProviderConfig,
    pub(crate) run_context: Metadata,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SemanticMemoryAttemptOutcome {
    Completed,
    Failed(String),
    Panicked(String),
}

pub(crate) enum SemanticMemoryEnqueueOutcome {
    Queued,
    Duplicate {
        run_context: Metadata,
        metrics: WorkQueueMetrics,
    },
    CapacityExceeded {
        job: Box<SemanticMemoryJob>,
        metrics: WorkQueueMetrics,
    },
}

pub(crate) struct SemanticMemoryEnqueueError {
    pub(crate) job: Box<SemanticMemoryJob>,
    pub(crate) reason: String,
}

pub(crate) struct SemanticMemoryQueue {
    state: Mutex<LossAwareWorkQueue<SemanticMemoryJob>>,
    wake: Condvar,
    worker_started: AtomicBool,
}

impl Default for SemanticMemoryQueue {
    fn default() -> Self {
        Self {
            state: Mutex::new(
                LossAwareWorkQueue::new(WorkQueuePolicy {
                    capacity: SEMANTIC_MEMORY_QUEUE_LIMIT,
                    max_attempts: SEMANTIC_MEMORY_MAX_ATTEMPTS,
                    base_backoff_ms: SEMANTIC_MEMORY_RETRY_BASE_MS,
                    max_backoff_ms: SEMANTIC_MEMORY_RETRY_MAX_MS,
                })
                .expect("semantic memory queue policy must be valid"),
            ),
            wake: Condvar::new(),
            worker_started: AtomicBool::new(false),
        }
    }
}

impl SemanticMemoryQueue {
    pub(crate) fn enqueue(
        &self,
        job_id: String,
        job: SemanticMemoryJob,
    ) -> Result<SemanticMemoryEnqueueOutcome, SemanticMemoryEnqueueError> {
        let run_context = job.run_context.clone();
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(error) => {
                return Err(SemanticMemoryEnqueueError {
                    job: Box::new(job),
                    reason: format!("semantic memory queue lock poisoned: {error}"),
                });
            }
        };
        let result = state.enqueue(job_id, job, current_time_millis());
        let metrics = state.metrics();
        match result.outcome() {
            WorkEnqueueOutcome::Queued => {
                self.wake.notify_one();
                Ok(SemanticMemoryEnqueueOutcome::Queued)
            }
            WorkEnqueueOutcome::Duplicate => Ok(SemanticMemoryEnqueueOutcome::Duplicate {
                run_context,
                metrics,
            }),
            WorkEnqueueOutcome::CapacityExceeded => {
                Ok(SemanticMemoryEnqueueOutcome::CapacityExceeded {
                    job: Box::new(
                        result
                            .into_rejected()
                            .expect("capacity rejection must return the job"),
                    ),
                    metrics,
                })
            }
        }
    }

    pub(crate) fn begin_worker(&self) -> bool {
        self.worker_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn finish_worker(&self) {
        self.worker_started.store(false, Ordering::Release);
    }

    pub(crate) fn next(&self) -> Result<Option<ClaimedWork<SemanticMemoryJob>>, String> {
        let state = self
            .state
            .lock()
            .map_err(|error| format!("semantic memory queue lock poisoned: {error}"))?;
        let wait_ms = state
            .next_ready_delay_ms(current_time_millis())
            .unwrap_or(1_000)
            .max(1);
        let (mut state, _) = self
            .wake
            .wait_timeout_while(state, Duration::from_millis(wait_ms), |state| {
                state
                    .next_ready_delay_ms(current_time_millis())
                    .is_none_or(|delay| delay > 0)
            })
            .map_err(|error| format!("semantic memory queue wait poisoned: {error}"))?;
        Ok(state.claim_ready(current_time_millis()))
    }

    pub(crate) fn complete(&self, job: ClaimedWork<SemanticMemoryJob>) -> SemanticMemoryJob {
        match self.state.lock() {
            Ok(mut state) => state.complete(job),
            Err(error) => {
                eprintln!("semantic memory queue lock poisoned during completion: {error}");
                job.into_payload()
            }
        }
    }

    pub(crate) fn abandon(&self, job: ClaimedWork<SemanticMemoryJob>) -> SemanticMemoryJob {
        match self.state.lock() {
            Ok(mut state) => state.abandon(job),
            Err(error) => {
                eprintln!("semantic memory queue lock poisoned during abandonment: {error}");
                job.into_payload()
            }
        }
    }

    pub(crate) fn drain_pending(&self) -> Result<Vec<SemanticMemoryJob>, String> {
        self.state
            .lock()
            .map(|mut state| state.drain_pending())
            .map_err(|error| {
                format!("semantic memory queue lock poisoned during shutdown drain: {error}")
            })
    }

    pub(crate) fn metrics(&self) -> Option<WorkQueueMetrics> {
        self.state.lock().ok().map(|state| state.metrics())
    }
}

static SEMANTIC_MEMORY_QUEUE: OnceLock<SemanticMemoryQueue> = OnceLock::new();

pub(crate) fn semantic_memory_queue() -> &'static SemanticMemoryQueue {
    SEMANTIC_MEMORY_QUEUE.get_or_init(SemanticMemoryQueue::default)
}

pub(crate) fn semantic_memory_job_id(run_context: &Metadata) -> Result<String, String> {
    let run_id = run_context
        .get("agent_run_id")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "semantic memory scheduling requires an agent run id".to_string())?;
    let project_id = run_context
        .get("project_id")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "semantic memory scheduling requires project and session scope".to_string())?;
    let session_id = run_context
        .get("session_id")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "semantic memory scheduling requires project and session scope".to_string())?;
    serde_json::to_string(&(project_id, session_id, run_id))
        .map_err(|error| format!("semantic memory queue identity unavailable: {error}"))
}

pub(crate) fn run_semantic_memory_attempt(
    operation: impl FnOnce() -> Result<(), String>,
) -> SemanticMemoryAttemptOutcome {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => SemanticMemoryAttemptOutcome::Completed,
        Ok(Err(error)) => SemanticMemoryAttemptOutcome::Failed(error),
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            SemanticMemoryAttemptOutcome::Panicked(format!(
                "semantic memory extraction panicked: {detail}"
            ))
        }
    }
}

pub(crate) fn retry_semantic_memory_job(
    app: &tauri::AppHandle,
    queue: &'static SemanticMemoryQueue,
    job: ClaimedWork<SemanticMemoryJob>,
) {
    let run_context = job.payload().run_context.clone();
    let result = match queue.state.lock() {
        Ok(mut state) => {
            let result = state.retry(job, current_time_millis());
            let metrics = state.metrics();
            (result, metrics)
        }
        Err(error) => {
            let job = job.into_payload();
            recover_semantic_memory_without_model(
                &app.state::<AppState>(),
                job.workspace_root,
                job.config,
                &job.run_context,
                &format!("semantic memory queue lock poisoned during retry: {error}"),
            );
            return;
        }
    };
    let (result, metrics) = result;
    match result.outcome() {
        WorkRetryOutcome::Scheduled {
            delay_ms,
            next_attempt,
        } => {
            record_semantic_memory_queue_event(
                app,
                &run_context,
                "Semantic memory refresh deferred",
                "foreground_preempted",
                metrics,
                [
                    ("retry_delay_ms".to_string(), delay_ms.to_string()),
                    (
                        "semantic_memory_attempt".to_string(),
                        next_attempt.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            );
            queue.wake.notify_one();
        }
        WorkRetryOutcome::Exhausted { attempts } => {
            let job = result
                .into_exhausted()
                .expect("exhausted retry must return the job");
            record_semantic_memory_queue_event(
                app,
                &job.run_context,
                "Semantic memory refresh exhausted",
                "retry_exhausted",
                metrics,
                [("semantic_memory_attempts".to_string(), attempts.to_string())]
                    .into_iter()
                    .collect(),
            );
            recover_semantic_memory_without_model(
                &app.state::<AppState>(),
                job.workspace_root,
                job.config,
                &job.run_context,
                "semantic memory extraction was repeatedly preempted",
            );
        }
    }
}

pub(crate) fn complete_semantic_memory_job_with_fallback(
    app: &tauri::AppHandle,
    queue: &'static SemanticMemoryQueue,
    job: ClaimedWork<SemanticMemoryJob>,
    outcome: &str,
    reason: &str,
) {
    let job = queue.complete(job);
    fallback_semantic_memory_job(app, queue, job, outcome, reason);
}

pub(crate) fn abandon_semantic_memory_job_with_fallback(
    app: &tauri::AppHandle,
    queue: &'static SemanticMemoryQueue,
    job: ClaimedWork<SemanticMemoryJob>,
    outcome: &str,
    reason: &str,
) {
    let job = queue.abandon(job);
    fallback_semantic_memory_job(app, queue, job, outcome, reason);
}

fn fallback_semantic_memory_job(
    app: &tauri::AppHandle,
    queue: &'static SemanticMemoryQueue,
    job: SemanticMemoryJob,
    outcome: &str,
    reason: &str,
) {
    record_semantic_memory_queue_event(
        app,
        &job.run_context,
        "Semantic memory refresh fell back",
        outcome,
        queue.metrics().unwrap_or_default(),
        Metadata::new(),
    );
    recover_semantic_memory_without_model(
        &app.state::<AppState>(),
        job.workspace_root,
        job.config,
        &job.run_context,
        reason,
    );
}

pub(crate) fn record_semantic_memory_queue_event(
    app: &tauri::AppHandle,
    run_context: &Metadata,
    summary: &str,
    outcome: &str,
    metrics: WorkQueueMetrics,
    mut metadata: Metadata,
) {
    metadata.extend([
        (
            "semantic_memory_queue_outcome".to_string(),
            outcome.to_string(),
        ),
        (
            "semantic_memory_queue_pending".to_string(),
            metrics.pending.to_string(),
        ),
        (
            "semantic_memory_queue_in_flight".to_string(),
            metrics.in_flight.to_string(),
        ),
        (
            "semantic_memory_queue_retries".to_string(),
            metrics.retries.to_string(),
        ),
        (
            "semantic_memory_queue_exhausted".to_string(),
            metrics.exhausted.to_string(),
        ),
        (
            "semantic_memory_queue_abandoned".to_string(),
            metrics.abandoned.to_string(),
        ),
    ]);
    let state = app.state::<AppState>();
    let result = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))
        .and_then(|mut store| {
            append_event(
                &mut store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                summary,
                metadata_with_context(metadata, run_context),
            )
            .map_err(|error| error.to_string())
        });
    if let Err(error) = result {
        eprintln!("semantic memory queue event unavailable: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::semantic_memory_job_id;
    use agent_core::Metadata;

    fn context(project: &str, session: &str, run: &str) -> Metadata {
        [
            ("project_id".to_string(), project.to_string()),
            ("session_id".to_string(), session.to_string()),
            ("agent_run_id".to_string(), run.to_string()),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn semantic_memory_queue_identity_is_scoped_to_project_session_and_run() {
        let first = semantic_memory_job_id(&context("project-a", "session-a", "run-a"));
        let other_project = semantic_memory_job_id(&context("project-b", "session-a", "run-a"));
        let other_session = semantic_memory_job_id(&context("project-a", "session-b", "run-a"));
        assert!(first.is_ok());
        assert_ne!(first, other_project);
        assert_ne!(first, other_session);
    }

    #[test]
    fn semantic_memory_queue_identity_rejects_incomplete_or_empty_scope() {
        assert!(semantic_memory_job_id(&Metadata::new()).is_err());
        assert!(semantic_memory_job_id(&context("", "session-a", "run-a")).is_err());
    }
}
