use super::queue::{
    record_semantic_memory_queue_event, semantic_memory_job_id, semantic_memory_queue,
    SemanticMemoryEnqueueOutcome, SemanticMemoryJob,
};
use super::{fallback_pending_semantic_memory_jobs, start_semantic_memory_worker};
use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use crate::semantic_memory_runtime::{
    recover_semantic_memory_without_model, refresh_deterministic_memory_projection,
};
use agent_core::Metadata;
use std::ffi::OsStr;
use std::path::PathBuf;
use tauri::Manager;

pub(crate) fn schedule_semantic_memory_refresh(
    app: tauri::AppHandle,
    workspace_root: PathBuf,
    config: ProviderConfig,
    run_context: Metadata,
) {
    if evaluation_background_memory_disabled(
        std::env::var_os("CINDX_AGENT_REALWORLD_DISABLE_BACKGROUND_MEMORY").as_deref(),
        &run_context,
    ) {
        let local_projection_config = ProviderConfig::default();
        if let Err(error) = refresh_deterministic_memory_projection(
            &app.state::<AppState>(),
            &workspace_root,
            &local_projection_config,
            &run_context,
        ) {
            eprintln!("evaluation memory projection unavailable: {error}");
        }
        return;
    }
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

fn evaluation_background_memory_disabled(value: Option<&OsStr>, run_context: &Metadata) -> bool {
    value.is_some_and(|value| matches!(value.to_str(), Some("1" | "true" | "yes")))
        && run_context
            .get("session_id")
            .is_some_and(|session_id| session_id.starts_with("session-realworld-"))
}

#[cfg(test)]
mod tests {
    use super::evaluation_background_memory_disabled;
    use agent_core::Metadata;
    use std::ffi::OsStr;

    #[test]
    fn provider_backed_memory_is_disabled_only_for_realworld_evaluation_sessions() {
        let evaluation_context: Metadata = [(
            "session_id".to_string(),
            "session-realworld-campaign-case".to_string(),
        )]
        .into_iter()
        .collect();
        let product_context: Metadata = [("session_id".to_string(), "sess-product".to_string())]
            .into_iter()
            .collect();

        assert!(!evaluation_background_memory_disabled(
            None,
            &evaluation_context
        ));
        assert!(!evaluation_background_memory_disabled(
            Some(OsStr::new("0")),
            &evaluation_context
        ));
        assert!(evaluation_background_memory_disabled(
            Some(OsStr::new("1")),
            &evaluation_context
        ));
        assert!(evaluation_background_memory_disabled(
            Some(OsStr::new("true")),
            &evaluation_context
        ));
        assert!(evaluation_background_memory_disabled(
            Some(OsStr::new("yes")),
            &evaluation_context
        ));
        assert!(!evaluation_background_memory_disabled(
            Some(OsStr::new("1")),
            &product_context
        ));
    }
}
