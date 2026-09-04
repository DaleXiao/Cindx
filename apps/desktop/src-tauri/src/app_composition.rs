//! The desktop composition root: what the application is made of.
//!
//! `app_bootstrap::run` keeps the decision about *whether* the app may start at
//! all — the panic log, the TLS provider, the persistent store and its
//! fatal-startup dialog, event redaction, and the startup probe. This module owns
//! everything after that decision: loading the persisted configuration in the
//! order its dependencies require, building the one managed [`AppState`] from it,
//! resuming the project lifecycle operations that recovery found pending, and
//! starting the background workers.
//!
//! Keeping every `AppState` field in one constructor is the point. They used to be
//! assembled inline inside `.manage(...)` in `run()`, and the load order existed
//! only as the sequence of statements in a 336-line function, so both were easy to
//! extend wrongly and impossible to test.

use super::*;
use crate::project_lifecycle_runtime::RecoveredMemoryRefresh;
use crate::session_output_cache_store::SessionOutputCache;
use crate::suspended_run_runtime::SuspendedRunStore;

/// Every persisted input the managed state is composed from, plus the project
/// lifecycle operations that recovery found pending.
pub(crate) struct DesktopComposition {
    pub(crate) provider_config: ProviderConfig,
    pub(crate) mcp_catalog: McpCatalogService,
    pub(crate) workspace_config: WorkspaceConfig,
    pub(crate) sidecar_config: SidecarConfig,
    pub(crate) web_search_config: WebSearchConfig,
    pub(crate) project_session_config: ProjectSessionConfig,
    pub(crate) schedule_config: ScheduleConfig,
    pub(crate) schedule_last_error: Option<String>,
    pub(crate) recovered_memory_refreshes: Vec<RecoveredMemoryRefresh>,
}

/// Loads the composition. The order is not incidental:
///
/// 1. The sidecar environment is applied before anything can spawn a sidecar.
/// 2. The project and session configuration is loaded from the *configured*
///    workspace root, and only afterwards is the authoritative project root
///    applied to the workspace configuration. Reversing those two would load every
///    session against the wrong root.
/// 3. Project lifecycle recovery needs both the store and that session
///    configuration, and it is fatal: a half-recovered lifecycle must not serve.
///    Its message is returned to the caller, which owns the fatal-startup dialog.
/// 4. Interrupted-run reconciliation and the schedule load are not fatal. A
///    schedule file that will not parse degrades to the default with its error
///    retained, because the panel has to be able to say why.
/// 5. Existing text artifacts are redacted once the authoritative root is known.
pub(crate) fn load_desktop_composition(
    store: &mut SqliteStore,
) -> Result<DesktopComposition, String> {
    let provider_config = load_provider_config();
    let mcp_catalog = McpCatalogService::load(mcp_config_path(), mcp_catalog_cache_path());
    let mut workspace_config = load_workspace_config();
    let sidecar_config = load_sidecar_config();
    apply_sidecar_env(&sidecar_config);
    let web_search_config = load_web_search_config();
    let project_session_config = load_project_session_config(&workspace_config.root);
    let recovered_memory_refreshes =
        recover_project_lifecycle_operations(store, &project_session_config).map_err(|error| {
            format!("project lifecycle recovery failed; startup aborted: {error}")
        })?;
    if let Err(error) = reconcile_interrupted_agent_runs(store) {
        append_startup_log(&format!("interrupted run recovery failed: {error}"));
    }
    let (schedule_config, schedule_last_error) = match schedule::load(&schedule_config_path()) {
        Ok(config) => (config, None),
        Err(error) => {
            append_startup_log(&error);
            (ScheduleConfig::default(), Some(error))
        }
    };
    apply_authoritative_project_root(&mut workspace_config, &project_session_config);
    for path in [
        context_checkpoint_path_for(&workspace_config.root),
        agent_trace_export_path_for(&workspace_config.root),
    ] {
        if let Err(error) = redact_existing_text_artifact(&path) {
            eprintln!("failed to redact {}: {error}", path.display());
        }
    }
    Ok(DesktopComposition {
        provider_config,
        mcp_catalog,
        workspace_config,
        sidecar_config,
        web_search_config,
        project_session_config,
        schedule_config,
        schedule_last_error,
        recovered_memory_refreshes,
    })
}

/// Builds the one managed state. Every `AppState` field is initialized here and
/// nowhere else, so a new field cannot be defaulted ad hoc at the builder, and the
/// caches, registries, and exit flags all provably start empty.
pub(crate) fn compose_app_state(composition: DesktopComposition, store: SqliteStore) -> AppState {
    AppState {
        store: Mutex::new(store),
        attachment_upload_batches: Mutex::new(AttachmentUploadBatches::default()),
        manual_tool_execution_gate: Mutex::new(()),
        provider_config: Mutex::new(composition.provider_config),
        provider_config_update: Mutex::new(()),
        workspace_config: Mutex::new(composition.workspace_config),
        sidecar_config: Mutex::new(composition.sidecar_config),
        web_search_config: Mutex::new(composition.web_search_config),
        project_session_config: Mutex::new(composition.project_session_config),
        session_lifecycle_gate: Mutex::new(()),
        schedule_config: Mutex::new(composition.schedule_config),
        schedule_last_error: Mutex::new(composition.schedule_last_error),
        mcp_catalog: Mutex::new(composition.mcp_catalog),
        suspended_agent_runs: SuspendedRunStore::default(),
        session_output_cache: SessionOutputCache::default(),
        agent_run_controls: agent_harness::RunRegistry::new("agent run control"),
        prompt_evaluation_controls: agent_harness::RunRegistry::new("prompt evaluation control"),
        rag_operation_controls: Mutex::new(BTreeMap::new()),
        queue_dispatching_sessions: agent_harness::ExclusiveKeyRegistry::new("queue dispatch"),
        session_title_refinement_sessions: agent_harness::ExclusiveKeyRegistry::new(
            "session title refinement",
        ),
        workspace_knowledge_cache: Mutex::new(BTreeMap::new()),
        process_manager: Arc::new(ProcessManager::new()),
        tool_registry_cache: Mutex::new(ToolRegistryCache::default()),
        tool_registry_generation: AtomicU64::new(0),
        allow_exit: AtomicBool::new(false),
        quit_prompt_active: AtomicBool::new(false),
    }
}

/// Resumes the project lifecycle operations that recovery found pending. This runs
/// at setup, after the state is managed, because a refresh publishes into the
/// shared coordinator and then completes a durable journal; a journal that still
/// will not complete is logged and left pending rather than dropped.
pub(crate) fn resume_recovered_memory_refreshes(
    refreshes: Vec<RecoveredMemoryRefresh>,
    provider_config: &ProviderConfig,
) {
    for refresh in refreshes {
        schedule_project_memory_vector_refresh(
            refresh.project_root,
            provider_config.clone(),
            refresh.ledger,
        );
        if let Err(error) = complete_project_lifecycle_journal(&refresh.journal) {
            append_startup_log(&format!(
                "recovered project lifecycle journal remains pending: {error}"
            ));
        }
    }
}

/// The background workers that run for the whole session. One owner so the set is
/// discoverable in one place: the schedule runner, tool-event metadata compaction,
/// and the `.cindx` aggregate-quota and TTL sweep.
pub(crate) fn start_background_workers(app: &tauri::AppHandle) {
    start_schedule_runner(app.clone());
    start_tool_event_metadata_compaction();
    start_cindx_retention_sweep(app.clone());
}

#[cfg(test)]
#[path = "app_composition_tests.rs"]
mod tests;
