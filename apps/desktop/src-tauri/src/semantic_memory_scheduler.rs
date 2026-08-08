use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use super::queue::{
    record_semantic_memory_queue_event, semantic_memory_job_id, semantic_memory_queue,
    SemanticMemoryEnqueueOutcome, SemanticMemoryJob,
};
use crate::semantic_memory_runtime::recover_semantic_memory_without_model;
use super::{fallback_pending_semantic_memory_jobs, start_semantic_memory_worker};
use agent_core::Metadata;
use std::path::PathBuf;
use tauri::Manager;

pub(crate) fn schedule_semantic_memory_refresh(
    app: tauri::AppHandle,
    workspace_root: PathBuf,
    config: ProviderConfig,
    run_context: Metadata,
) {
    let job_id = match semantic_memory_job_id(&run_context) {
        Ok(job_id) => job_id,
        Err(error) => {
            recover_semantic_memory_without_model(
                &app.state::<AppState>(),
                workspace_root,
                config,
                &run_context,
                &error,
            );
            return;
        }
    };
    let queue = semantic_memory_queue();
    let enqueue = queue.enqueue(
        job_id,
        SemanticMemoryJob {
            workspace_root,
            config,
            run_context,
        },
    );
    match enqueue {
        Ok(SemanticMemoryEnqueueOutcome::Queued) => {}
        Ok(SemanticMemoryEnqueueOutcome::Duplicate {
            run_context,
            metrics,
        }) => {
            record_semantic_memory_queue_event(
                &app,
                &run_context,
                "Semantic memory refresh coalesced",
                "duplicate",
                metrics,
                Metadata::new(),
            );
            return;
        }
        Ok(SemanticMemoryEnqueueOutcome::CapacityExceeded { job, metrics }) => {
            let job = *job;
            record_semantic_memory_queue_event(
                &app,
                &job.run_context,
                "Semantic memory refresh fell back",
                "capacity_exceeded",
                metrics,
                Metadata::new(),
            );
            recover_semantic_memory_without_model(
                &app.state::<AppState>(),
                job.workspace_root,
                job.config,
                &job.run_context,
                "semantic memory queue capacity exceeded",
            );
            return;
        }
        Err(error) => {
            let job = *error.job;
            recover_semantic_memory_without_model(
                &app.state::<AppState>(),
                job.workspace_root,
                job.config,
                &job.run_context,
                &error.reason,
            );
            return;
        }
    }

    if let Err(error) = start_semantic_memory_worker(app.clone(), queue) {
        fallback_pending_semantic_memory_jobs(&app, queue, "worker_start_failed", &error);
    }
}
