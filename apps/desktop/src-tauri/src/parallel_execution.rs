use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

const MAX_GLOBAL_MODEL_WORKERS: usize = 12;

pub(crate) type ParallelJob<T> = Box<dyn FnOnce() -> T + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParallelTaskError {
    Spawn(String),
    Panic,
}

impl fmt::Display for ParallelTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "worker could not start: {error}"),
            Self::Panic => formatter.write_str("worker panicked"),
        }
    }
}

#[derive(Debug)]
struct WorkerGateState {
    active: usize,
}

#[derive(Debug)]
struct WorkerGate {
    limit: usize,
    state: Mutex<WorkerGateState>,
    available: Condvar,
}

impl WorkerGate {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            state: Mutex::new(WorkerGateState { active: 0 }),
            available: Condvar::new(),
        }
    }

    fn acquire(self: &Arc<Self>) -> WorkerPermit {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.active >= self.limit {
            state = self
                .available
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        state.active += 1;
        WorkerPermit {
            gate: Arc::clone(self),
        }
    }
}

#[derive(Debug)]
struct WorkerPermit {
    gate: Arc<WorkerGate>,
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active = state.active.saturating_sub(1);
        self.gate.available.notify_one();
    }
}

#[derive(Debug, Clone)]
struct BoundedParallelExecutor {
    gate: Arc<WorkerGate>,
}

impl BoundedParallelExecutor {
    fn new(limit: usize) -> Self {
        Self {
            gate: Arc::new(WorkerGate::new(limit)),
        }
    }

    fn run_ordered<T: Send + 'static>(
        &self,
        thread_label: &str,
        jobs: Vec<ParallelJob<T>>,
    ) -> Vec<Result<T, ParallelTaskError>> {
        enum Worker<T> {
            Running(JoinHandle<T>),
            Failed(String),
        }

        static THREAD_SEQUENCE: AtomicU64 = AtomicU64::new(1);

        let workers = jobs
            .into_iter()
            .map(|job| {
                let permit = self.gate.acquire();
                let sequence = THREAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let name = format!("cindx-{thread_label}-{sequence}");
                match thread::Builder::new().name(name).spawn(move || {
                    let _permit = permit;
                    job()
                }) {
                    Ok(handle) => Worker::Running(handle),
                    Err(error) => Worker::Failed(error.to_string()),
                }
            })
            .collect::<Vec<_>>();

        workers
            .into_iter()
            .map(|worker| match worker {
                Worker::Running(handle) => handle.join().map_err(|_| ParallelTaskError::Panic),
                Worker::Failed(error) => Err(ParallelTaskError::Spawn(error)),
            })
            .collect()
    }
}

fn model_executor() -> &'static BoundedParallelExecutor {
    static EXECUTOR: OnceLock<BoundedParallelExecutor> = OnceLock::new();
    EXECUTOR.get_or_init(|| BoundedParallelExecutor::new(MAX_GLOBAL_MODEL_WORKERS))
}

pub(crate) fn run_model_jobs_ordered<T: Send + 'static>(
    thread_label: &str,
    jobs: Vec<ParallelJob<T>>,
) -> Vec<Result<T, ParallelTaskError>> {
    model_executor().run_ordered(thread_label, jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn bounded_executor_preserves_order_and_caps_parallelism() {
        let executor = BoundedParallelExecutor::new(3);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let jobs = (0usize..12)
            .map(|index| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                Box::new(move || {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(5));
                    active.fetch_sub(1, Ordering::SeqCst);
                    index
                }) as ParallelJob<usize>
            })
            .collect();

        let output = executor
            .run_ordered("test", jobs)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("workers should finish");

        assert_eq!(output, (0usize..12).collect::<Vec<_>>());
        assert!(peak.load(Ordering::SeqCst) <= 3);
    }

    #[test]
    fn worker_panic_releases_capacity_for_following_jobs() {
        let executor = BoundedParallelExecutor::new(1);
        let jobs = vec![
            Box::new(|| -> usize { panic!("expected test panic") }) as ParallelJob<usize>,
            Box::new(|| 2usize) as ParallelJob<usize>,
        ];

        let output = executor.run_ordered("panic-test", jobs);

        assert_eq!(output[0], Err(ParallelTaskError::Panic));
        assert_eq!(output[1], Ok(2));
    }
}
