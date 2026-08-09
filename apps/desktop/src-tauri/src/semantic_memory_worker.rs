use crate::app_state::AppState;
use crate::background_work_runtime::wait_for_foreground_agent_idle;
use crate::runtime_constants::BACKGROUND_WORK_IDLE_GRACE_MS;
#[path = "semantic_memory_queue.rs"]
mod queue;
#[path = "semantic_memory_scheduler.rs"]
mod scheduler;

pub(crate) use scheduler::schedule_semantic_memory_refresh;

use self::queue::{
    abandon_semantic_memory_job_with_fallback, complete_semantic_memory_job_with_fallback,
    record_semantic_memory_queue_event, retry_semantic_memory_job, run_semantic_memory_attempt,
    SemanticMemoryAttemptOutcome, SemanticMemoryQueue,
};
use crate::semantic_memory_runtime::{
    generate_semantic_memory, recover_semantic_memory_without_model,
};
use agent_core::Metadata;
use agent_runtime::AgentRunControl;
use model_provider::MODEL_REQUEST_CANCELLED;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

pub(crate) fn start_semantic_memory_worker(
    app: tauri::AppHandle,
    queue: &'static SemanticMemoryQueue,
) -> Result<(), String> {
    if !queue.begin_worker() {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("cindx-semantic-memory".to_string())
        .spawn(move || semantic_memory_worker_loop(app, queue))
        .map(|_| ())
        .map_err(|error| {
            queue.finish_worker();
            format!("semantic memory worker could not start: {error}")
        })
}

fn semantic_memory_worker_loop(app: tauri::AppHandle, queue: &'static SemanticMemoryQueue) {
    let mut pending_fallback_outcome = "application_shutdown";
    let mut pending_fallback_reason =
        "semantic memory extraction was pending during application shutdown".to_string();
    while !app.state::<AppState>().allow_exit.load(Ordering::Relaxed) {
        let job = match queue.next() {
            Ok(Some(job)) => job,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("{error}");
                pending_fallback_outcome = "worker_stopped";
                pending_fallback_reason = error;
                break;
            }
        };
        let state = app.state::<AppState>();
        let idle_control = Arc::new(AgentRunControl::new("auto"));
        match wait_for_foreground_agent_idle(
            &state,
            &idle_control,
            Duration::from_millis(BACKGROUND_WORK_IDLE_GRACE_MS),
        ) {
            Ok(true) => {}
            Ok(false) => {
                abandon_semantic_memory_job_with_fallback(
                    &app,
                    queue,
                    job,
                    "application_shutdown",
                    "semantic memory extraction was interrupted by application shutdown",
                );
                break;
            }
            Err(error) => {
                complete_semantic_memory_job_with_fallback(
                    &app,
                    queue,
                    job,
                    "idle_wait_failed",
                    &format!("semantic project memory idle wait failed: {error}"),
                );
                continue;
            }
        }

        let attempt = run_semantic_memory_attempt(|| {
            generate_semantic_memory(
                &state,
                &job.payload().workspace_root,
                &job.payload().config,
                &job.payload().run_context,
            )
        });
        match attempt {
            SemanticMemoryAttemptOutcome::Completed => {
                queue.complete(job);
            }
            SemanticMemoryAttemptOutcome::Failed(error)
                if error == MODEL_REQUEST_CANCELLED
                    && !state.allow_exit.load(Ordering::Relaxed) =>
            {
                retry_semantic_memory_job(&app, queue, job)
            }
            SemanticMemoryAttemptOutcome::Failed(error) => {
                complete_semantic_memory_job_with_fallback(
                    &app,
                    queue,
                    job,
                    "generation_failed",
                    &error,
                );
            }
            SemanticMemoryAttemptOutcome::Panicked(error) => {
                complete_semantic_memory_job_with_fallback(
                    &app,
                    queue,
                    job,
                    "generation_panicked",
                    &error,
                );
            }
        }
    }
    fallback_pending_semantic_memory_jobs(
        &app,
        queue,
        pending_fallback_outcome,
        &pending_fallback_reason,
    );
    queue.finish_worker();
}

pub(crate) fn fallback_pending_semantic_memory_jobs(
    app: &tauri::AppHandle,
    queue: &'static SemanticMemoryQueue,
    outcome: &str,
    reason: &str,
) {
    let jobs = match queue.drain_pending() {
        Ok(jobs) => jobs,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    for job in jobs {
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
}

#[cfg(test)]
mod tests {
    use super::queue::SemanticMemoryAttemptOutcome;
    use super::run_semantic_memory_attempt;

    #[test]
    fn semantic_memory_attempt_isolates_panics_without_losing_the_failure_reason() {
        let outcome = run_semantic_memory_attempt(|| panic!("provider decoder failed"));

        assert_eq!(
            outcome,
            SemanticMemoryAttemptOutcome::Panicked(
                "semantic memory extraction panicked: provider decoder failed".to_string()
            )
        );
    }

    #[test]
    fn semantic_memory_attempt_preserves_recoverable_errors() {
        assert_eq!(
            run_semantic_memory_attempt(|| Err("provider unavailable".to_string())),
            SemanticMemoryAttemptOutcome::Failed("provider unavailable".to_string())
        );
    }
}
