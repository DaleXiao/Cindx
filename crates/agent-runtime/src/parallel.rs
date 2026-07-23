use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub type ParallelJob<T> = Box<dyn FnOnce() -> T + Send + 'static>;
pub type CancellableParallelJob<T> = Box<dyn FnOnce(Arc<AtomicBool>) -> T + Send + 'static>;

#[derive(Debug)]
pub struct QuorumExecution<T> {
    pub results: Vec<Result<T, ParallelTaskError>>,
    pub quorum_reached: bool,
    pub successful: usize,
    pub cancelled_stragglers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParallelTaskError {
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
pub struct BoundedParallelExecutor {
    gate: Arc<WorkerGate>,
}

impl BoundedParallelExecutor {
    pub fn new(limit: usize) -> Self {
        Self {
            gate: Arc::new(WorkerGate::new(limit)),
        }
    }

    pub fn run_ordered<T: Send + 'static>(
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

    pub fn run_until_quorum<T, F>(
        &self,
        thread_label: &str,
        jobs: Vec<CancellableParallelJob<T>>,
        required_successes: usize,
        grace_period: Duration,
        is_success: F,
    ) -> QuorumExecution<T>
    where
        T: Send + 'static,
        F: Fn(&T) -> bool,
    {
        static THREAD_SEQUENCE: AtomicU64 = AtomicU64::new(1);

        let total = jobs.len();
        if total == 0 {
            return QuorumExecution {
                results: Vec::new(),
                quorum_reached: false,
                successful: 0,
                cancelled_stragglers: 0,
            };
        }
        let required_successes = required_successes.clamp(1, total);
        let cancellation = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let mut handles = Vec::with_capacity(total);
        let mut slots = (0..total).map(|_| None).collect::<Vec<_>>();

        for (index, job) in jobs.into_iter().enumerate() {
            let permit = self.gate.acquire();
            let worker_cancellation = Arc::clone(&cancellation);
            let sender = sender.clone();
            let sequence = THREAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = format!("cindx-{thread_label}-{sequence}");
            match thread::Builder::new().name(name).spawn(move || {
                let _permit = permit;
                let result =
                    catch_unwind(AssertUnwindSafe(|| job(Arc::clone(&worker_cancellation))))
                        .map_err(|_| ParallelTaskError::Panic);
                let _ = sender.send((index, result));
            }) {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    slots[index] = Some(Err(ParallelTaskError::Spawn(error.to_string())));
                }
            }
        }
        drop(sender);

        let mut received = 0usize;
        let mut successful = 0usize;
        let spawned = handles.len();
        let mut quorum_at = None;
        let mut cancelled_stragglers = 0usize;

        while received < spawned {
            let next = if let Some(quorum_at) = quorum_at {
                let remaining =
                    grace_period.saturating_sub(Instant::now().duration_since(quorum_at));
                if remaining.is_zero() {
                    None
                } else {
                    receiver.recv_timeout(remaining).ok()
                }
            } else {
                receiver.recv().ok()
            };
            let Some((index, result)) = next else {
                cancelled_stragglers = spawned.saturating_sub(received);
                cancellation.store(true, Ordering::SeqCst);
                break;
            };
            if result.as_ref().is_ok_and(&is_success) {
                successful = successful.saturating_add(1);
            }
            slots[index] = Some(result);
            received = received.saturating_add(1);
            if successful >= required_successes && quorum_at.is_none() && received < spawned {
                quorum_at = Some(Instant::now());
                if grace_period.is_zero() {
                    cancelled_stragglers = spawned.saturating_sub(received);
                    cancellation.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }

        while received < spawned {
            let Ok((index, result)) = receiver.recv() else {
                break;
            };
            if result.as_ref().is_ok_and(&is_success) {
                successful = successful.saturating_add(1);
            }
            slots[index] = Some(result);
            received = received.saturating_add(1);
        }
        for handle in handles {
            let _ = handle.join();
        }

        QuorumExecution {
            results: slots
                .into_iter()
                .map(|result| result.unwrap_or(Err(ParallelTaskError::Panic)))
                .collect(),
            quorum_reached: successful >= required_successes,
            successful,
            cancelled_stragglers,
        }
    }
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

    #[test]
    fn quorum_executor_cancels_only_remaining_stragglers_and_preserves_order() {
        let executor = BoundedParallelExecutor::new(3);
        let jobs = vec![
            Box::new(|_| 10usize) as CancellableParallelJob<usize>,
            Box::new(|_| 20usize) as CancellableParallelJob<usize>,
            Box::new(|cancelled: Arc<AtomicBool>| {
                while !cancelled.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                0usize
            }) as CancellableParallelJob<usize>,
        ];

        let output =
            executor.run_until_quorum("quorum-test", jobs, 2, Duration::ZERO, |value| *value > 0);

        assert!(output.quorum_reached);
        assert_eq!(output.successful, 2);
        assert_eq!(output.cancelled_stragglers, 1);
        assert_eq!(output.results[0], Ok(10));
        assert_eq!(output.results[1], Ok(20));
        assert_eq!(output.results[2], Ok(0));
    }
}
