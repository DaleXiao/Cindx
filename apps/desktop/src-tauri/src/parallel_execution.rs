use agent_runtime::{BoundedParallelExecutor, ParallelTaskError};
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) use agent_runtime::{CancellableParallelJob, ParallelJob, QuorumExecution};

const MAX_GLOBAL_MODEL_WORKERS: usize = 12;

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

pub(crate) fn run_model_jobs_until_quorum<T, F>(
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
    model_executor().run_until_quorum(
        thread_label,
        jobs,
        required_successes,
        grace_period,
        is_success,
    )
}
