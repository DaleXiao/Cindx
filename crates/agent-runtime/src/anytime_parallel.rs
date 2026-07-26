use crate::parallel::{
    BoundedParallelExecutor, CancellableParallelJob, ParallelJobCompletion, ParallelTaskError,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnytimeQuorumPolicy {
    pub minimum_successes: usize,
    pub preferred_successes: usize,
    pub improvement_window: Duration,
    pub poll_interval: Duration,
}

impl AnytimeQuorumPolicy {
    pub fn new(
        minimum_successes: usize,
        preferred_successes: usize,
        improvement_window: Duration,
        poll_interval: Duration,
    ) -> Self {
        Self {
            minimum_successes,
            preferred_successes,
            improvement_window,
            poll_interval,
        }
    }
}

#[derive(Debug)]
pub struct AnytimeQuorumExecution<T> {
    pub results: Vec<Result<T, ParallelTaskError>>,
    pub minimum_reached: bool,
    pub preferred_reached: bool,
    pub successful: usize,
    pub cancelled_stragglers: usize,
    pub interrupted: bool,
}

impl BoundedParallelExecutor {
    pub fn run_until_anytime_quorum_interruptible<T, F, I>(
        &self,
        thread_label: &str,
        jobs: Vec<CancellableParallelJob<T>>,
        policy: AnytimeQuorumPolicy,
        is_success: F,
        mut should_interrupt: I,
    ) -> AnytimeQuorumExecution<T>
    where
        T: Send + 'static,
        F: Fn(&T) -> bool,
        I: FnMut() -> bool,
    {
        let total = jobs.len();
        if total == 0 {
            return AnytimeQuorumExecution {
                results: Vec::new(),
                minimum_reached: false,
                preferred_reached: false,
                successful: 0,
                cancelled_stragglers: 0,
                interrupted: false,
            };
        }

        let minimum_successes = policy.minimum_successes.clamp(1, total);
        let preferred_successes = policy.preferred_successes.clamp(minimum_successes, total);
        let poll_interval = policy.poll_interval.max(Duration::from_millis(1));
        let mut supervisor = self.supervisor();
        let mut slots = (0..total).map(|_| None).collect::<Vec<_>>();
        for (index, job) in jobs.into_iter().enumerate() {
            if let Err(error) = supervisor.submit(index, thread_label, job) {
                slots[index] = Some(Err(error));
            }
        }

        let spawned = supervisor.pending();
        let mut received = 0usize;
        let mut successful = 0usize;
        let mut minimum_at = None;
        let mut cancelled_stragglers = 0usize;
        let mut interrupted = false;

        while received < spawned {
            if should_interrupt() {
                interrupted = true;
                cancelled_stragglers = supervisor.cancel_all();
                collect_completed_results(
                    &mut supervisor,
                    &mut slots,
                    &mut received,
                    &mut successful,
                    &is_success,
                );
                break;
            }
            if successful >= preferred_successes {
                collect_ready_results(
                    &mut supervisor,
                    &mut slots,
                    &mut received,
                    &mut successful,
                    &is_success,
                );
                cancelled_stragglers = supervisor.cancel_all();
                collect_completed_results(
                    &mut supervisor,
                    &mut slots,
                    &mut received,
                    &mut successful,
                    &is_success,
                );
                break;
            }

            let wait = minimum_at
                .map(|minimum_at| {
                    policy
                        .improvement_window
                        .saturating_sub(Instant::now().duration_since(minimum_at))
                        .min(poll_interval)
                })
                .unwrap_or(poll_interval);
            if minimum_at.is_some() && wait.is_zero() {
                cancelled_stragglers = supervisor.cancel_all();
                collect_completed_results(
                    &mut supervisor,
                    &mut slots,
                    &mut received,
                    &mut successful,
                    &is_success,
                );
                break;
            }

            let Some(ParallelJobCompletion { job_id, result }) = supervisor.recv_timeout(wait)
            else {
                continue;
            };
            if result.as_ref().is_ok_and(&is_success) {
                successful = successful.saturating_add(1);
            }
            slots[job_id] = Some(result);
            received = received.saturating_add(1);
            if successful >= minimum_successes && minimum_at.is_none() {
                minimum_at = Some(Instant::now());
            }
        }

        drop(supervisor);
        AnytimeQuorumExecution {
            results: slots
                .into_iter()
                .map(|result| result.unwrap_or(Err(ParallelTaskError::Cancelled)))
                .collect(),
            minimum_reached: successful >= minimum_successes,
            preferred_reached: successful >= preferred_successes,
            successful,
            cancelled_stragglers,
            interrupted,
        }
    }
}

fn collect_ready_results<T, F>(
    supervisor: &mut crate::parallel::ParallelJobSupervisor<T>,
    slots: &mut [Option<Result<T, ParallelTaskError>>],
    received: &mut usize,
    successful: &mut usize,
    is_success: &F,
) where
    T: Send + 'static,
    F: Fn(&T) -> bool,
{
    while let Some(ParallelJobCompletion { job_id, result }) = supervisor.try_recv() {
        if result.as_ref().is_ok_and(is_success) {
            *successful = successful.saturating_add(1);
        }
        slots[job_id] = Some(result);
        *received = received.saturating_add(1);
    }
}

fn collect_completed_results<T, F>(
    supervisor: &mut crate::parallel::ParallelJobSupervisor<T>,
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn anytime_quorum_waits_briefly_for_preferred_quality_then_cancels_tail() {
        let executor = BoundedParallelExecutor::new(3);
        let jobs = vec![
            Box::new(|_| 10usize) as CancellableParallelJob<usize>,
            Box::new(|_| {
                thread::sleep(Duration::from_millis(8));
                20usize
            }) as CancellableParallelJob<usize>,
            Box::new(|cancelled: Arc<AtomicBool>| {
                while !cancelled.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                0usize
            }) as CancellableParallelJob<usize>,
        ];

        let output = executor.run_until_anytime_quorum_interruptible(
            "anytime-preferred",
            jobs,
            AnytimeQuorumPolicy::new(1, 2, Duration::from_millis(30), Duration::from_millis(2)),
            |value| *value > 0,
            || false,
        );

        assert!(output.minimum_reached);
        assert!(output.preferred_reached);
        assert_eq!(output.successful, 2);
        assert_eq!(output.cancelled_stragglers, 1);
        assert_eq!(output.results[0], Ok(10));
        assert_eq!(output.results[1], Ok(20));
        assert_eq!(output.results[2], Err(ParallelTaskError::Cancelled));
    }

    #[test]
    fn anytime_quorum_returns_minimum_result_when_preferred_quality_is_too_slow() {
        let executor = BoundedParallelExecutor::new(2);
        let jobs = vec![
            Box::new(|_| 10usize) as CancellableParallelJob<usize>,
            Box::new(|cancelled: Arc<AtomicBool>| {
                while !cancelled.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                20usize
            }) as CancellableParallelJob<usize>,
        ];
        let started = Instant::now();

        let output = executor.run_until_anytime_quorum_interruptible(
            "anytime-minimum",
            jobs,
            AnytimeQuorumPolicy::new(1, 2, Duration::from_millis(10), Duration::from_millis(2)),
            |_| true,
            || false,
        );

        assert!(output.minimum_reached);
        assert!(!output.preferred_reached);
        assert_eq!(output.successful, 1);
        assert_eq!(output.cancelled_stragglers, 1);
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
