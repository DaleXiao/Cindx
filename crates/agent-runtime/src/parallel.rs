use std::collections::BTreeMap;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
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

#[derive(Debug)]
pub struct InterruptibleQuorumExecution<T> {
    pub execution: QuorumExecution<T>,
    pub interrupted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptibleQuorumPolicy {
    pub required_successes: usize,
    pub grace_period: Duration,
    pub poll_interval: Duration,
}

impl InterruptibleQuorumPolicy {
    pub fn new(required_successes: usize, grace_period: Duration, poll_interval: Duration) -> Self {
        Self {
            required_successes,
            grace_period,
            poll_interval,
        }
    }
}

#[derive(Debug)]
pub struct ParallelJobCompletion<T> {
    pub job_id: usize,
    pub result: Result<T, ParallelTaskError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParallelTaskError {
    Spawn(String),
    Panic,
    Cancelled,
}

impl fmt::Display for ParallelTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "worker could not start: {error}"),
            Self::Panic => formatter.write_str("worker panicked"),
            Self::Cancelled => formatter.write_str("worker cancelled after quorum"),
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

pub struct ParallelJobSupervisor<T> {
    gate: Arc<WorkerGate>,
    sender: mpsc::Sender<ParallelJobCompletion<T>>,
    receiver: mpsc::Receiver<ParallelJobCompletion<T>>,
    jobs: BTreeMap<usize, ParallelJobControl>,
}

const JOB_RUNNING: u8 = 0;
const JOB_COMPLETED: u8 = 1;
const JOB_CANCELLED: u8 = 2;

#[derive(Debug)]
struct ParallelJobControl {
    cancellation: Arc<AtomicBool>,
    lifecycle: Arc<AtomicU8>,
}

impl<T> fmt::Debug for ParallelJobSupervisor<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParallelJobSupervisor")
            .field("pending", &self.jobs.len())
            .finish()
    }
}

impl<T: Send + 'static> ParallelJobSupervisor<T> {
    pub fn submit(
        &mut self,
        job_id: usize,
        thread_label: &str,
        job: CancellableParallelJob<T>,
    ) -> Result<(), ParallelTaskError> {
        if self.jobs.contains_key(&job_id) {
            return Err(ParallelTaskError::Spawn(format!(
                "duplicate parallel job id {job_id}"
            )));
        }

        static THREAD_SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let lifecycle = Arc::new(AtomicU8::new(JOB_RUNNING));
        let worker_lifecycle = Arc::clone(&lifecycle);
        let sender = self.sender.clone();
        let gate = Arc::clone(&self.gate);
        let sequence = THREAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!("cindx-{thread_label}-{sequence}");
        thread::Builder::new()
            .name(name)
            .spawn(move || {
                let permit = gate.acquire();
                let result = if worker_cancellation.load(Ordering::Acquire) {
                    Err(ParallelTaskError::Cancelled)
                } else {
                    catch_unwind(AssertUnwindSafe(|| job(Arc::clone(&worker_cancellation))))
                        .map_err(|_| ParallelTaskError::Panic)
                };
                let result = if worker_lifecycle
                    .compare_exchange(
                        JOB_RUNNING,
                        JOB_COMPLETED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    result
                } else {
                    Err(ParallelTaskError::Cancelled)
                };
                drop(permit);
                let _ = sender.send(ParallelJobCompletion { job_id, result });
            })
            .map_err(|error| ParallelTaskError::Spawn(error.to_string()))?;
        self.jobs.insert(
            job_id,
            ParallelJobControl {
                cancellation,
                lifecycle,
            },
        );
        Ok(())
    }

    pub fn recv(&mut self) -> Option<ParallelJobCompletion<T>> {
        self.receiver.recv().ok().inspect(|completion| {
            self.jobs.remove(&completion.job_id);
        })
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<ParallelJobCompletion<T>> {
        self.receiver
            .recv_timeout(timeout)
            .ok()
            .inspect(|completion| {
                self.jobs.remove(&completion.job_id);
            })
    }

    pub fn try_recv(&mut self) -> Option<ParallelJobCompletion<T>> {
        self.receiver.try_recv().ok().inspect(|completion| {
            self.jobs.remove(&completion.job_id);
        })
    }

    pub fn cancel(&self, job_id: usize) -> bool {
        self.jobs.get(&job_id).is_some_and(cancel_job)
    }

    pub fn cancel_all(&self) -> usize {
        self.jobs
            .values()
            .filter(|control| cancel_job(control))
            .count()
    }

    /// Collects jobs that atomically completed before cancellation won.
    ///
    /// A completion can race with a quorum decision after the receiver's last
    /// poll. The lifecycle flag makes that boundary explicit: completed work is
    /// retained, while jobs for which cancellation won return `Cancelled`.
    pub(crate) fn collect_completed(&mut self) -> Vec<ParallelJobCompletion<T>> {
        let mut completions = Vec::new();
        while self
            .jobs
            .values()
            .any(|control| control.lifecycle.load(Ordering::Acquire) == JOB_COMPLETED)
        {
            let Some(completion) = self.recv() else {
                break;
            };
            completions.push(completion);
        }
        completions
    }

    pub fn pending(&self) -> usize {
        self.jobs.len()
    }
}

fn cancel_job(control: &ParallelJobControl) -> bool {
    if control
        .lifecycle
        .compare_exchange(
            JOB_RUNNING,
            JOB_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return false;
    }
    control.cancellation.store(true, Ordering::Release);
    true
}

impl<T> Drop for ParallelJobSupervisor<T> {
    fn drop(&mut self) {
        for control in self.jobs.values() {
            let _ = cancel_job(control);
        }
    }
}

impl BoundedParallelExecutor {
    pub fn new(limit: usize) -> Self {
        Self {
            gate: Arc::new(WorkerGate::new(limit)),
        }
    }

    pub fn supervisor<T: Send + 'static>(&self) -> ParallelJobSupervisor<T> {
        let (sender, receiver) = mpsc::channel();
        ParallelJobSupervisor {
            gate: Arc::clone(&self.gate),
            sender,
            receiver,
            jobs: BTreeMap::new(),
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
        let mut supervisor = self.supervisor();
        let mut slots = (0..total).map(|_| None).collect::<Vec<_>>();

        for (index, job) in jobs.into_iter().enumerate() {
            if let Err(error) = supervisor.submit(index, thread_label, job) {
                slots[index] = Some(Err(error));
            }
        }

        let mut received = 0usize;
        let mut successful = 0usize;
        let spawned = supervisor.pending();
        let mut quorum_at = None;
        let mut cancelled_stragglers = 0usize;

        while received < spawned {
            let next = if let Some(quorum_at) = quorum_at {
                let remaining =
                    grace_period.saturating_sub(Instant::now().duration_since(quorum_at));
                if remaining.is_zero() {
                    None
                } else {
                    supervisor.recv_timeout(remaining)
                }
            } else {
                supervisor.recv()
            };
            let Some(ParallelJobCompletion { job_id, result }) = next else {
                cancelled_stragglers = supervisor.cancel_all();
                collect_completed_quorum_results(
                    &mut supervisor,
                    &mut slots,
                    &mut received,
                    &mut successful,
                    &is_success,
                );
                break;
            };
            if result.as_ref().is_ok_and(&is_success) {
                successful = successful.saturating_add(1);
            }
            slots[job_id] = Some(result);
            received = received.saturating_add(1);
            if successful >= required_successes && quorum_at.is_none() && received < spawned {
                quorum_at = Some(Instant::now());
                if grace_period.is_zero() {
                    cancelled_stragglers = supervisor.cancel_all();
                    collect_completed_quorum_results(
                        &mut supervisor,
                        &mut slots,
                        &mut received,
                        &mut successful,
                        &is_success,
                    );
                    break;
                }
            }
        }

        drop(supervisor);

        QuorumExecution {
            results: slots
                .into_iter()
                .map(|result| result.unwrap_or(Err(ParallelTaskError::Cancelled)))
                .collect(),
            quorum_reached: successful >= required_successes,
            successful,
            cancelled_stragglers,
        }
    }

    pub fn run_until_quorum_interruptible<T, F, I>(
        &self,
        thread_label: &str,
        jobs: Vec<CancellableParallelJob<T>>,
        policy: InterruptibleQuorumPolicy,
        is_success: F,
        mut should_interrupt: I,
    ) -> InterruptibleQuorumExecution<T>
    where
        T: Send + 'static,
        F: Fn(&T) -> bool,
        I: FnMut() -> bool,
    {
        let total = jobs.len();
        if total == 0 {
            return InterruptibleQuorumExecution {
                execution: QuorumExecution {
                    results: Vec::new(),
                    quorum_reached: false,
                    successful: 0,
                    cancelled_stragglers: 0,
                },
                interrupted: false,
            };
        }
        let required_successes = policy.required_successes.clamp(1, total);
        let grace_period = policy.grace_period;
        let mut supervisor = self.supervisor();
        let mut slots = (0..total).map(|_| None).collect::<Vec<_>>();

        for (index, job) in jobs.into_iter().enumerate() {
            if let Err(error) = supervisor.submit(index, thread_label, job) {
                slots[index] = Some(Err(error));
            }
        }

        let poll_interval = policy.poll_interval.max(Duration::from_millis(1));
        let mut received = 0usize;
        let mut successful = 0usize;
        let spawned = supervisor.pending();
        let mut quorum_at = None;
        let mut cancelled_stragglers = 0usize;
        let mut interrupted = false;

        while received < spawned {
            if should_interrupt() {
                interrupted = true;
                cancelled_stragglers = supervisor.cancel_all();
                collect_completed_quorum_results(
                    &mut supervisor,
                    &mut slots,
                    &mut received,
                    &mut successful,
                    &is_success,
                );
                break;
            }
            let wait = if let Some(quorum_at) = quorum_at {
                let remaining =
                    grace_period.saturating_sub(Instant::now().duration_since(quorum_at));
                if remaining.is_zero() {
                    cancelled_stragglers = supervisor.cancel_all();
                    collect_completed_quorum_results(
                        &mut supervisor,
                        &mut slots,
                        &mut received,
                        &mut successful,
                        &is_success,
                    );
                    break;
                }
                remaining.min(poll_interval)
            } else {
                poll_interval
            };
            let Some(ParallelJobCompletion { job_id, result }) = supervisor.recv_timeout(wait)
            else {
                continue;
            };
            if result.as_ref().is_ok_and(&is_success) {
                successful = successful.saturating_add(1);
            }
            slots[job_id] = Some(result);
            received = received.saturating_add(1);
            if successful >= required_successes && quorum_at.is_none() && received < spawned {
                quorum_at = Some(Instant::now());
                if grace_period.is_zero() {
                    cancelled_stragglers = supervisor.cancel_all();
                    collect_completed_quorum_results(
                        &mut supervisor,
                        &mut slots,
                        &mut received,
                        &mut successful,
                        &is_success,
                    );
                    break;
                }
            }
        }

        drop(supervisor);

        InterruptibleQuorumExecution {
            execution: QuorumExecution {
                results: slots
                    .into_iter()
                    .map(|result| result.unwrap_or(Err(ParallelTaskError::Cancelled)))
                    .collect(),
                quorum_reached: successful >= required_successes,
                successful,
                cancelled_stragglers,
            },
            interrupted,
        }
    }
}

fn collect_completed_quorum_results<T, F>(
    supervisor: &mut ParallelJobSupervisor<T>,
    slots: &mut [Option<Result<T, ParallelTaskError>>],
    received: &mut usize,
    successful: &mut usize,
    is_success: &F,
) where
    T: Send + 'static,
    F: Fn(&T) -> bool,
{
    for ParallelJobCompletion { job_id, result } in supervisor.collect_completed() {
        if result.as_ref().is_ok_and(is_success) {
            *successful = successful.saturating_add(1);
        }
        slots[job_id] = Some(result);
        *received = received.saturating_add(1);
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
        assert_eq!(output.results[2], Err(ParallelTaskError::Cancelled));
    }

    #[test]
    fn quorum_executor_returns_without_joining_a_non_cooperative_straggler() {
        let executor = BoundedParallelExecutor::new(2);
        let jobs = vec![
            Box::new(|_| 10usize) as CancellableParallelJob<usize>,
            Box::new(|_| {
                thread::sleep(Duration::from_millis(500));
                20usize
            }) as CancellableParallelJob<usize>,
        ];

        let started = Instant::now();
        let output =
            executor.run_until_quorum("early-return-test", jobs, 1, Duration::ZERO, |_| true);

        assert!(output.quorum_reached);
        assert_eq!(output.successful, 1);
        assert_eq!(output.cancelled_stragglers, 1);
        assert_eq!(output.results[0], Ok(10));
        assert_eq!(output.results[1], Err(ParallelTaskError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "quorum return waited for the detached straggler"
        );
    }

    #[test]
    fn interruptible_quorum_cancels_speculation_without_waiting_for_stragglers() {
        let executor = BoundedParallelExecutor::new(2);
        let jobs = vec![
            Box::new(|cancelled: Arc<AtomicBool>| {
                while !cancelled.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                10usize
            }) as CancellableParallelJob<usize>,
            Box::new(|_| {
                thread::sleep(Duration::from_millis(500));
                20usize
            }) as CancellableParallelJob<usize>,
        ];
        let started = Instant::now();

        let output = executor.run_until_quorum_interruptible(
            "interrupt-test",
            jobs,
            InterruptibleQuorumPolicy::new(2, Duration::ZERO, Duration::from_millis(5)),
            |_| true,
            || started.elapsed() >= Duration::from_millis(20),
        );

        assert!(output.interrupted);
        assert_eq!(output.execution.cancelled_stragglers, 2);
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn supervisor_accepts_followup_work_before_an_older_straggler_finishes() {
        let executor = BoundedParallelExecutor::new(3);
        let mut supervisor = executor.supervisor();
        supervisor
            .submit(
                0,
                "dynamic-test",
                Box::new(|_| {
                    thread::sleep(Duration::from_millis(500));
                    "slow"
                }),
            )
            .unwrap();
        supervisor
            .submit(1, "dynamic-test", Box::new(|_| "anchor"))
            .unwrap();

        let first = supervisor
            .recv_timeout(Duration::from_millis(250))
            .expect("the direct anchor should complete first");
        assert_eq!(first.job_id, 1);
        assert_eq!(first.result, Ok("anchor"));

        supervisor
            .submit(2, "dynamic-test", Box::new(|_| "verification"))
            .unwrap();
        let second = supervisor
            .recv_timeout(Duration::from_millis(250))
            .expect("followup verification should not wait for the straggler");
        assert_eq!(second.job_id, 2);
        assert_eq!(second.result, Ok("verification"));
        assert!(supervisor.cancel(0));
    }
}
