use crate::app_state::AppState;
use crate::background_work_runtime::wait_for_foreground_agent_idle;
use crate::configuration_models::ProviderConfig;
use crate::runtime_constants::BACKGROUND_WORK_IDLE_GRACE_MS;
use crate::semantic_memory_runtime::{
    generate_semantic_memory, recover_semantic_memory_without_model,
};
use agent_core::Metadata;
use agent_runtime::AgentRunControl;
use model_provider::MODEL_REQUEST_CANCELLED;
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
use tauri::Manager;

const SEMANTIC_MEMORY_QUEUE_LIMIT: usize = 128;

struct SemanticMemoryJob {
    run_id: String,
    workspace_root: PathBuf,
    config: ProviderConfig,
    run_context: Metadata,
}

#[derive(Default)]
struct SemanticMemoryQueueState {
    pending: VecDeque<SemanticMemoryJob>,
    known_run_ids: BTreeSet<String>,
}

#[derive(Default)]
struct SemanticMemoryQueue {
    state: Mutex<SemanticMemoryQueueState>,
    wake: Condvar,
    worker_started: AtomicBool,
}

static SEMANTIC_MEMORY_QUEUE: OnceLock<SemanticMemoryQueue> = OnceLock::new();

pub(crate) fn schedule_semantic_memory_refresh(
    app: tauri::AppHandle,
    workspace_root: PathBuf,
    config: ProviderConfig,
    run_context: Metadata,
) {
    let Some(run_id) = run_context.get("agent_run_id").cloned() else {
        return;
    };
    if !run_context.contains_key("project_id") || !run_context.contains_key("session_id") {
        return;
    }
    let queue = SEMANTIC_MEMORY_QUEUE.get_or_init(SemanticMemoryQueue::default);
    let enqueued = queue
        .state
        .lock()
        .map(|mut state| {
            if state.known_run_ids.contains(&run_id)
                || state.pending.len() >= SEMANTIC_MEMORY_QUEUE_LIMIT
            {
                return false;
            }
            state.known_run_ids.insert(run_id.clone());
            state.pending.push_back(SemanticMemoryJob {
                run_id,
                workspace_root,
                config,
                run_context,
            });
            true
        })
        .unwrap_or(false);
    if !enqueued {
        return;
    }
    queue.wake.notify_one();
    start_semantic_memory_worker(app, queue);
}

fn start_semantic_memory_worker(app: tauri::AppHandle, queue: &'static SemanticMemoryQueue) {
    if queue
        .worker_started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    if let Err(error) = std::thread::Builder::new()
        .name("cindx-semantic-memory".to_string())
        .spawn(move || semantic_memory_worker_loop(app, queue))
    {
        queue.worker_started.store(false, Ordering::Release);
        eprintln!("semantic memory worker could not start: {error}");
    }
}

fn semantic_memory_worker_loop(app: tauri::AppHandle, queue: &'static SemanticMemoryQueue) {
    while !app.state::<AppState>().allow_exit.load(Ordering::Relaxed) {
        let Some(job) = next_semantic_memory_job(queue) else {
            continue;
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
                finish_semantic_memory_job(queue, &job.run_id);
                break;
            }
            Err(error) => {
                eprintln!("semantic project memory idle wait failed: {error}");
                finish_semantic_memory_job(queue, &job.run_id);
                continue;
            }
        }
        match generate_semantic_memory(&state, &job.workspace_root, &job.config, &job.run_context) {
            Ok(()) => finish_semantic_memory_job(queue, &job.run_id),
            Err(error)
                if error == MODEL_REQUEST_CANCELLED
                    && !state.allow_exit.load(Ordering::Relaxed) =>
            {
                retry_semantic_memory_job(queue, job)
            }
            Err(error) => {
                recover_semantic_memory_without_model(
                    &state,
                    job.workspace_root,
                    job.config,
                    &job.run_context,
                    &error,
                );
                finish_semantic_memory_job(queue, &job.run_id);
            }
        }
    }
    queue.worker_started.store(false, Ordering::Release);
}

fn next_semantic_memory_job(queue: &'static SemanticMemoryQueue) -> Option<SemanticMemoryJob> {
    let state = queue.state.lock().ok()?;
    let (mut state, _) = queue
        .wake
        .wait_timeout_while(state, Duration::from_secs(1), |state| {
            state.pending.is_empty()
        })
        .ok()?;
    state.pending.pop_front()
}

fn retry_semantic_memory_job(queue: &'static SemanticMemoryQueue, job: SemanticMemoryJob) {
    if let Ok(mut state) = queue.state.lock() {
        state.pending.push_front(job);
    }
}

fn finish_semantic_memory_job(queue: &'static SemanticMemoryQueue, run_id: &str) {
    if let Ok(mut state) = queue.state.lock() {
        state.known_run_ids.remove(run_id);
    }
}
