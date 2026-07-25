use agent_runtime::BoundedParallelExecutor;
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) use agent_runtime::{
    CancellableParallelJob, InterruptibleQuorumExecution, ParallelJobCompletion,
    ParallelJobSupervisor,
};

const MAX_GLOBAL_MODEL_WORKERS: usize = 12;

fn model_executor() -> &'static BoundedParallelExecutor {
    static EXECUTOR: OnceLock<BoundedParallelExecutor> = OnceLock::new();
    EXECUTOR.get_or_init(|| BoundedParallelExecutor::new(MAX_GLOBAL_MODEL_WORKERS))
}

pub(crate) fn model_job_supervisor<T: Send + 'static>() -> ParallelJobSupervisor<T> {
    model_executor().supervisor()
}

pub(crate) fn run_model_jobs_until_quorum_interruptible<T, F, I>(
    thread_label: &str,
    jobs: Vec<CancellableParallelJob<T>>,
    required_successes: usize,
    grace_period: Duration,
    poll_interval: Duration,
    is_success: F,
    should_interrupt: I,
) -> InterruptibleQuorumExecution<T>
where
    T: Send + 'static,
    F: Fn(&T) -> bool,
    I: FnMut() -> bool,
{
    model_executor().run_until_quorum_interruptible(
        thread_label,
        jobs,
        required_successes,
        grace_period,
        poll_interval,
        is_success,
        should_interrupt,
    )
}
