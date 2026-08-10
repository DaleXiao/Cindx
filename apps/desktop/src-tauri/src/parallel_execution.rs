use agent_runtime::BoundedParallelExecutor;
use std::sync::OnceLock;

pub(crate) use agent_runtime::{
    AnytimeQuorumExecution, AnytimeQuorumPolicy, CancellableParallelJob,
    InterruptibleQuorumExecution, InterruptibleQuorumPolicy, ParallelJob, ParallelJobSupervisor,
    ParallelTaskError,
};

const MAX_GLOBAL_MODEL_WORKERS: usize = 12;
pub(crate) const MAX_GLOBAL_TOOL_WORKERS: usize = 4;

fn model_executor() -> &'static BoundedParallelExecutor {
    static EXECUTOR: OnceLock<BoundedParallelExecutor> = OnceLock::new();
    EXECUTOR.get_or_init(|| BoundedParallelExecutor::new(MAX_GLOBAL_MODEL_WORKERS))
}

fn tool_executor() -> &'static BoundedParallelExecutor {
    static EXECUTOR: OnceLock<BoundedParallelExecutor> = OnceLock::new();
    EXECUTOR.get_or_init(|| BoundedParallelExecutor::new(MAX_GLOBAL_TOOL_WORKERS))
}

#[cfg(test)]
pub(crate) static TOOL_EXECUTOR_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn model_job_supervisor<T: Send + 'static>() -> ParallelJobSupervisor<T> {
    model_executor().supervisor()
}

pub(crate) fn run_tool_jobs_ordered<T: Send + 'static>(
    jobs: Vec<ParallelJob<T>>,
) -> Vec<Result<T, ParallelTaskError>> {
    tool_executor().run_ordered("agent-tool", jobs)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn tool_executor_preserves_input_order() {
        let _test_guard = TOOL_EXECUTOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let release = Arc::new(Barrier::new(3));
        let jobs = [40_u64, 5, 20]
            .into_iter()
            .enumerate()
            .map(|(index, delay_ms)| {
                let release = Arc::clone(&release);
                Box::new(move || {
                    release.wait();
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    index
                }) as ParallelJob<usize>
            })
            .collect();

        let results = run_tool_jobs_ordered(jobs)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("tool workers should finish");

        assert_eq!(results, vec![0, 1, 2]);
    }

    #[test]
    fn tool_executor_caps_global_parallelism() {
        let _test_guard = TOOL_EXECUTOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let first_wave = Arc::new(Barrier::new(MAX_GLOBAL_TOOL_WORKERS));
        let jobs = (0..MAX_GLOBAL_TOOL_WORKERS * 2)
            .map(|index| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                let first_wave = Arc::clone(&first_wave);
                Box::new(move || {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    if index < MAX_GLOBAL_TOOL_WORKERS {
                        first_wave.wait();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    active.fetch_sub(1, Ordering::SeqCst);
                    index
                }) as ParallelJob<usize>
            })
            .collect();

        let results = run_tool_jobs_ordered(jobs)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("tool workers should finish");

        assert_eq!(
            results,
            (0..MAX_GLOBAL_TOOL_WORKERS * 2).collect::<Vec<_>>()
        );
        assert_eq!(peak.load(Ordering::SeqCst), MAX_GLOBAL_TOOL_WORKERS);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}
