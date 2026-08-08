use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkQueuePolicy {
    pub capacity: usize,
    pub max_attempts: u32,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl WorkQueuePolicy {
    pub fn validate(self) -> Result<Self, &'static str> {
        if self.capacity == 0 {
            return Err("work queue capacity must be positive");
        }
        if self.max_attempts == 0 {
            return Err("work queue max attempts must be positive");
        }
        if self.base_backoff_ms == 0 {
            return Err("work queue base backoff must be positive");
        }
        if self.max_backoff_ms < self.base_backoff_ms {
            return Err("work queue max backoff must cover the base backoff");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkEnqueueOutcome {
    Queued,
    Duplicate,
    CapacityExceeded,
}

#[derive(Debug)]
pub struct WorkEnqueueResult<T> {
    outcome: WorkEnqueueOutcome,
    rejected: Option<T>,
}

impl<T> WorkEnqueueResult<T> {
    pub fn outcome(&self) -> WorkEnqueueOutcome {
        self.outcome
    }

    pub fn into_rejected(self) -> Option<T> {
        self.rejected
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkRetryOutcome {
    Scheduled { delay_ms: u64, next_attempt: u32 },
    Exhausted { attempts: u32 },
}

#[derive(Debug)]
pub struct WorkRetryResult<T> {
    outcome: WorkRetryOutcome,
    exhausted: Option<T>,
}

impl<T> WorkRetryResult<T> {
    pub fn outcome(&self) -> WorkRetryOutcome {
        self.outcome
    }

    pub fn into_exhausted(self) -> Option<T> {
        self.exhausted
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkQueueMetrics {
    pub accepted: u64,
    pub duplicates: u64,
    pub capacity_rejections: u64,
    pub claimed: u64,
    pub retries: u64,
    pub completed: u64,
    pub exhausted: u64,
    pub abandoned: u64,
    pub pending: usize,
    pub in_flight: usize,
}

#[derive(Debug)]
struct PendingWork<T> {
    id: String,
    payload: T,
    attempts: u32,
    not_before_ms: u64,
}

#[derive(Debug)]
pub struct ClaimedWork<T> {
    id: String,
    payload: T,
    attempt: u32,
}

impl<T> ClaimedWork<T> {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn payload(&self) -> &T {
        &self.payload
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn into_payload(self) -> T {
        self.payload
    }
}

#[derive(Debug)]
pub struct LossAwareWorkQueue<T> {
    policy: WorkQueuePolicy,
    pending: VecDeque<PendingWork<T>>,
    known_ids: BTreeSet<String>,
    metrics: WorkQueueMetrics,
}

impl<T> LossAwareWorkQueue<T> {
    pub fn new(policy: WorkQueuePolicy) -> Result<Self, &'static str> {
        Ok(Self {
            policy: policy.validate()?,
            pending: VecDeque::new(),
            known_ids: BTreeSet::new(),
            metrics: WorkQueueMetrics::default(),
        })
    }

    pub fn enqueue(&mut self, id: String, payload: T, now_ms: u64) -> WorkEnqueueResult<T> {
        if self.known_ids.contains(&id) {
            self.metrics.duplicates = self.metrics.duplicates.saturating_add(1);
            return WorkEnqueueResult {
                outcome: WorkEnqueueOutcome::Duplicate,
                rejected: Some(payload),
            };
        }
        if self.known_ids.len() >= self.policy.capacity {
            self.metrics.capacity_rejections = self.metrics.capacity_rejections.saturating_add(1);
            return WorkEnqueueResult {
                outcome: WorkEnqueueOutcome::CapacityExceeded,
                rejected: Some(payload),
            };
        }

        self.known_ids.insert(id.clone());
        self.pending.push_back(PendingWork {
            id,
            payload,
            attempts: 0,
            not_before_ms: now_ms,
        });
        self.metrics.accepted = self.metrics.accepted.saturating_add(1);
        WorkEnqueueResult {
            outcome: WorkEnqueueOutcome::Queued,
            rejected: None,
        }
    }

    pub fn claim_ready(&mut self, now_ms: u64) -> Option<ClaimedWork<T>> {
        let index = self
            .pending
            .iter()
            .position(|job| job.not_before_ms <= now_ms)?;
        let mut job = self.pending.remove(index)?;
        job.attempts = job.attempts.saturating_add(1);
        self.metrics.claimed = self.metrics.claimed.saturating_add(1);
        Some(ClaimedWork {
            id: job.id,
            payload: job.payload,
            attempt: job.attempts,
        })
    }

    pub fn next_ready_delay_ms(&self, now_ms: u64) -> Option<u64> {
        self.pending
            .iter()
            .map(|job| job.not_before_ms.saturating_sub(now_ms))
            .min()
    }

    pub fn complete(&mut self, job: ClaimedWork<T>) -> T {
        self.known_ids.remove(job.id());
        self.metrics.completed = self.metrics.completed.saturating_add(1);
        job.into_payload()
    }

    pub fn retry(&mut self, job: ClaimedWork<T>, now_ms: u64) -> WorkRetryResult<T> {
        if job.attempt >= self.policy.max_attempts {
            self.known_ids.remove(job.id());
            self.metrics.exhausted = self.metrics.exhausted.saturating_add(1);
            return WorkRetryResult {
                outcome: WorkRetryOutcome::Exhausted {
                    attempts: job.attempt,
                },
                exhausted: Some(job.payload),
            };
        }

        let delay_ms = retry_delay_ms(self.policy, job.attempt);
        let next_attempt = job.attempt.saturating_add(1);
        self.pending.push_back(PendingWork {
            id: job.id,
            payload: job.payload,
            attempts: job.attempt,
            not_before_ms: now_ms.saturating_add(delay_ms),
        });
        self.metrics.retries = self.metrics.retries.saturating_add(1);
        WorkRetryResult {
            outcome: WorkRetryOutcome::Scheduled {
                delay_ms,
                next_attempt,
            },
            exhausted: None,
        }
    }

    pub fn abandon(&mut self, job: ClaimedWork<T>) -> T {
        self.known_ids.remove(job.id());
        self.metrics.abandoned = self.metrics.abandoned.saturating_add(1);
        job.into_payload()
    }

    pub fn cancel_pending(&mut self, id: &str) -> Option<T> {
        let index = self.pending.iter().position(|job| job.id == id)?;
        let job = self.pending.remove(index)?;
        self.known_ids.remove(id);
        self.metrics.abandoned = self.metrics.abandoned.saturating_add(1);
        Some(job.payload)
    }

    pub fn drain_pending(&mut self) -> Vec<T> {
        let mut payloads = Vec::with_capacity(self.pending.len());
        while let Some(job) = self.pending.pop_front() {
            self.known_ids.remove(&job.id);
            payloads.push(job.payload);
        }
        self.metrics.abandoned = self.metrics.abandoned.saturating_add(payloads.len() as u64);
        payloads
    }

    pub fn metrics(&self) -> WorkQueueMetrics {
        let mut metrics = self.metrics;
        metrics.pending = self.pending.len();
        metrics.in_flight = self.known_ids.len().saturating_sub(self.pending.len());
        metrics
    }
}

fn retry_delay_ms(policy: WorkQueuePolicy, completed_attempts: u32) -> u64 {
    let exponent = completed_attempts.saturating_sub(1).min(31);
    policy
        .base_backoff_ms
        .saturating_mul(1_u64 << exponent)
        .min(policy.max_backoff_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(capacity: usize, max_attempts: u32) -> LossAwareWorkQueue<&'static str> {
        LossAwareWorkQueue::new(WorkQueuePolicy {
            capacity,
            max_attempts,
            base_backoff_ms: 100,
            max_backoff_ms: 800,
        })
        .expect("policy should be valid")
    }

    #[test]
    fn duplicate_and_capacity_outcomes_are_explicit() {
        let mut queue = queue(1, 3);
        assert_eq!(
            queue.enqueue("run-1".to_string(), "first", 0).outcome(),
            WorkEnqueueOutcome::Queued
        );
        assert_eq!(
            queue.enqueue("run-1".to_string(), "duplicate", 0).outcome(),
            WorkEnqueueOutcome::Duplicate
        );
        assert_eq!(
            queue.enqueue("run-2".to_string(), "second", 0).outcome(),
            WorkEnqueueOutcome::CapacityExceeded
        );
        assert_eq!(queue.metrics().capacity_rejections, 1);
    }

    #[test]
    fn retry_is_bounded_and_uses_exponential_backoff() {
        let mut queue = queue(2, 3);
        assert_eq!(
            queue.enqueue("run-1".to_string(), "payload", 0).outcome(),
            WorkEnqueueOutcome::Queued
        );

        let first = queue.claim_ready(0).expect("first attempt should run");
        assert_eq!(first.attempt(), 1);
        assert_eq!(
            queue.retry(first, 0).outcome(),
            WorkRetryOutcome::Scheduled {
                delay_ms: 100,
                next_attempt: 2,
            }
        );
        assert!(queue.claim_ready(99).is_none());

        let second = queue.claim_ready(100).expect("second attempt should run");
        assert_eq!(
            queue.retry(second, 100).outcome(),
            WorkRetryOutcome::Scheduled {
                delay_ms: 200,
                next_attempt: 3,
            }
        );
        let third = queue.claim_ready(300).expect("third attempt should run");
        assert_eq!(
            queue.retry(third, 300).outcome(),
            WorkRetryOutcome::Exhausted { attempts: 3 }
        );
        assert_eq!(queue.metrics().exhausted, 1);
        assert_eq!(queue.metrics().pending, 0);
    }

    #[test]
    fn delayed_retry_does_not_starve_ready_work() {
        let mut queue = queue(2, 3);
        queue.enqueue("run-1".to_string(), "first", 0);
        let first = queue.claim_ready(0).expect("first should run");
        queue.retry(first, 0);
        queue.enqueue("run-2".to_string(), "second", 1);

        let ready = queue
            .claim_ready(1)
            .expect("ready work should bypass delay");
        assert_eq!(ready.id(), "run-2");
        assert_eq!(queue.complete(ready), "second");
    }

    #[test]
    fn completion_releases_identity_for_future_runs() {
        let mut queue = queue(1, 2);
        queue.enqueue("run-1".to_string(), "first", 0);
        let job = queue.claim_ready(0).expect("work should be claimable");
        assert_eq!(queue.complete(job), "first");
        assert_eq!(
            queue.enqueue("run-1".to_string(), "again", 1).outcome(),
            WorkEnqueueOutcome::Queued
        );
    }

    #[test]
    fn shutdown_drain_returns_every_pending_payload_and_releases_capacity() {
        let mut queue = queue(2, 2);
        queue.enqueue("run-1".to_string(), "first", 0);
        queue.enqueue("run-2".to_string(), "second", 0);

        assert_eq!(queue.drain_pending(), vec!["first", "second"]);
        assert_eq!(queue.metrics().pending, 0);
        assert_eq!(queue.metrics().in_flight, 0);
        assert_eq!(queue.metrics().abandoned, 2);
        assert_eq!(
            queue.enqueue("run-1".to_string(), "again", 1).outcome(),
            WorkEnqueueOutcome::Queued
        );
    }
}
