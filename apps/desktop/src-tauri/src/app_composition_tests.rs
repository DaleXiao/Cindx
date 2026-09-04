use super::*;
use std::sync::atomic::Ordering;

/// The composition root's contract: the managed state serves exactly the values
/// that were loaded, and nothing is pre-populated.
#[test]
fn compose_app_state_serves_the_loaded_values_and_starts_empty() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let store = SqliteStore::in_memory().expect("store should open");
    let composition = DesktopComposition {
        provider_config: ProviderConfig {
            model: "composition-model".to_string(),
            ..Default::default()
        },
        mcp_catalog: McpCatalogService::load(
            workspace.path().join("mcp.json"),
            workspace.path().join("mcp-cache.json"),
        ),
        workspace_config: WorkspaceConfig {
            root: workspace.path().join("work"),
        },
        sidecar_config: SidecarConfig {
            browser_path: "/tmp/composition-browser".to_string(),
            ..Default::default()
        },
        web_search_config: WebSearchConfig::default(),
        project_session_config: {
            let mut config = load_project_session_config(workspace.path());
            config.active_project_id = "project-1".to_string();
            config
        },
        schedule_config: ScheduleConfig::default(),
        schedule_last_error: Some("schedule file was corrupt".to_string()),
        recovered_memory_refreshes: Vec::new(),
    };

    let state = compose_app_state(composition, store);

    assert_eq!(
        state.provider_config.lock().expect("poisoned").model,
        "composition-model"
    );
    assert_eq!(
        state.workspace_config.lock().expect("poisoned").root,
        workspace.path().join("work")
    );
    assert_eq!(
        state.sidecar_config.lock().expect("poisoned").browser_path,
        "/tmp/composition-browser"
    );
    assert_eq!(
        state
            .project_session_config
            .lock()
            .expect("poisoned")
            .active_project_id,
        "project-1"
    );
    assert_eq!(
        state
            .schedule_last_error
            .lock()
            .expect("poisoned")
            .as_deref(),
        Some("schedule file was corrupt"),
        "a schedule file that will not parse degrades to the default but keeps its reason"
    );
    assert!(state
        .schedule_config
        .lock()
        .expect("poisoned")
        .schedules
        .is_empty());

    // A composed state cannot inherit another run's leftovers.
    assert!(state
        .rag_operation_controls
        .lock()
        .expect("poisoned")
        .is_empty());
    assert!(state
        .workspace_knowledge_cache
        .lock()
        .expect("poisoned")
        .is_empty());
    assert_eq!(
        state.tool_registry_generation.load(Ordering::SeqCst),
        0,
        "the registry generation starts before any invalidation"
    );
    assert!(!state.allow_exit.load(Ordering::SeqCst));
    assert!(!state.quit_prompt_active.load(Ordering::SeqCst));
}

/// Recovery usually finds nothing; resuming an empty set must be a no-op rather
/// than an error, because it runs on every startup.
#[test]
fn resuming_no_recovered_lifecycle_operations_is_a_no_op() {
    resume_recovered_memory_refreshes(Vec::new(), &ProviderConfig::default());
}
