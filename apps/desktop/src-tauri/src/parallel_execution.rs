use agent_runtime::BoundedParallelExecutor;
use std::sync::OnceLock;

pub(crate) use agent_runtime::{
    AnytimeQuorumExecution, AnytimeQuorumPolicy, CancellableParallelJob,
    InterruptibleQuorumExecution, InterruptibleQuorumPolicy, ParallelJobCompletion,
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
    policy: InterruptibleQuorumPolicy,
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
        policy,
        is_success,
        should_interrupt,
    )
}

pub(crate) fn run_model_jobs_until_anytime_quorum_interruptible<T, F, I>(
    thread_label: &str,
    jobs: Vec<CancellableParallelJob<T>>,
    policy: AnytimeQuorumPolicy,
    is_success: F,
    should_interrupt: I,
) -> AnytimeQuorumExecution<T>
where
    T: Send + 'static,
    F: Fn(&T) -> bool,
    I: FnMut() -> bool,
{
    model_executor().run_until_anytime_quorum_interruptible(
        thread_label,
        jobs,
        policy,
        is_success,
        should_interrupt,
    )
}
