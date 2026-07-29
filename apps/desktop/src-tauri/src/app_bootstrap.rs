use super::*;
use crate::conductor_health_runtime::ConductorHealthLedger;

pub fn run() {
    install_startup_panic_log();
    if let Err(error) = migrate_legacy_app_data() {
        append_startup_log(&format!("legacy data migration failed: {error}"));
    }
    append_startup_log(&format!(
        "starting Cindx {} on {} with data root {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        app_data_root().display()
    ));
    let data_root = app_data_root();
    let event_redaction_pending = !event_redaction_complete(&data_root);
    let (mut store, persistent_store) = match open_app_store() {
        Ok(store) => (store, true),
        Err(error) => {
            append_startup_log(&format!(
                "persistent state unavailable; using in-memory state: {error}"
            ));
            (
                SqliteStore::in_memory().unwrap_or_else(|memory_error| {
                    panic!(
                        "failed to open persistent state ({error}) and in-memory state ({memory_error})"
                    )
                }),
                false,
            )
        }
    };
    if event_redaction_pending && persistent_store {
        match redact_persisted_events(&mut store) {
            Ok(_) => {
                if let Err(error) = mark_event_redaction_complete(&data_root) {
                    append_startup_log(&format!(
                        "failed to mark event redaction complete: {error}"
                    ));
                }
            }
            Err(error) => eprintln!("failed to redact persisted Cindx history: {error}"),
        }
    }
    if let Err(error) = reconcile_interrupted_agent_runs(&mut store) {
        append_startup_log(&format!("interrupted run recovery failed: {error}"));
    }
    let provider_config = load_provider_config();
    let mcp_catalog = McpCatalogService::load(mcp_config_path(), mcp_catalog_cache_path());
    let mut workspace_config = load_workspace_config();
    let sidecar_config = load_sidecar_config();
    let web_search_config = load_web_search_config();
    let project_session_config = load_project_session_config(&workspace_config.root);
    let (schedule_config, schedule_last_error) = match schedule::load(&schedule_config_path()) {
        Ok(config) => (config, None),
        Err(error) => {
            append_startup_log(&error);
            (ScheduleConfig::default(), Some(error))
        }
    };
    if let Some(project) = project_session_config.active_project() {
        if let Ok(root) = validate_workspace_root(&project.root) {
            workspace_config.root = root;
        }
    }
    for path in [
        context_checkpoint_path_for(&workspace_config.root),
        agent_trace_export_path_for(&workspace_config.root),
    ] {
        if let Err(error) = redact_existing_text_artifact(&path) {
            eprintln!("failed to redact {}: {error}", path.display());
        }
    }
    apply_sidecar_env(&sidecar_config);
    if std::env::var("CINDX_STARTUP_PROBE")
        .map(|value| config_bool(&value))
        .unwrap_or(false)
    {
        append_startup_log("startup probe completed");
        return;
    }

    let app = tauri::Builder::default()
        .manage(AppState {
            store: Mutex::new(store),
            provider_config: Mutex::new(provider_config),
            workspace_config: Mutex::new(workspace_config),
            sidecar_config: Mutex::new(sidecar_config),
            web_search_config: Mutex::new(web_search_config),
            project_session_config: Mutex::new(project_session_config),
            schedule_config: Mutex::new(schedule_config),
            schedule_last_error: Mutex::new(schedule_last_error),
            mcp_catalog: Mutex::new(mcp_catalog),
            suspended_agent_runs: Mutex::new(BTreeMap::new()),
            session_output_cache: Mutex::new(BTreeMap::new()),
            agent_run_controls: Mutex::new(BTreeMap::new()),
            prompt_evaluation_controls: Mutex::new(BTreeMap::new()),
            queue_dispatching_sessions: Mutex::new(BTreeSet::new()),
            session_title_refinement_sessions: Mutex::new(BTreeSet::new()),
            workspace_knowledge_cache: Mutex::new(BTreeMap::new()),
            tool_registry_cache: Mutex::new(ToolRegistryCache::default()),
            conductor_health: Mutex::new(ConductorHealthLedger::default()),
            tool_registry_generation: AtomicU64::new(0),
            allow_exit: AtomicBool::new(false),
            quit_prompt_active: AtomicBool::new(false),
        })
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = install_macos_sidebar_material(&window) {
                    append_startup_log(&error);
                }
            }
            schedule_main_window_reveal_fallback(app.handle().clone());
            start_schedule_runner(app.handle().clone());
            start_prompt_evolution_worker(app.handle().clone());
            start_tool_event_metadata_compaction();
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::Resized(_)
                    | tauri::WindowEvent::ScaleFactorChanged { .. }
                    | tauri::WindowEvent::Focused(true)
            ) {
                schedule_macos_traffic_light_position_repair(window.app_handle(), window.label());
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !confirm_application_exit(window.app_handle()) {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            reveal_main_window,
            set_sidebar_material_width,
            get_runtime_status,
            get_sidecar_state,
            save_sidecar_config,
            get_web_search_config,
            save_web_search_config,
            get_mcp_state,
            save_mcp_servers,
            upsert_mcp_server,
            update_mcp_server_policy,
            remove_mcp_server,
            refresh_mcp_server,
            get_skill_state,
            refresh_skills,
            save_skill_preference,
            install_skill_package,
            install_skill_url,
            get_project_session_state,
            get_schedule_state,
            upsert_schedule,
            set_schedule_enabled,
            delete_schedule,
            run_schedule_now,
            cancel_schedule_run,
            create_project,
            create_session,
            rename_project,
            delete_project,
            rename_session,
            set_session_effort,
            generate_session_title,
            stage_agent_attachments,
            remove_agent_attachment,
            fork_session,
            archive_session,
            restore_session,
            delete_session,
            select_project,
            select_session,
            acknowledge_session_activity,
            confirm_delete_action,
            pick_workspace_folder,
            save_workspace_root,
            get_phase3_state,
            get_permission_review_state,
            request_mock_permission,
            resolve_permission,
            get_phase4_state,
            get_personalization_config,
            save_personalization_config,
            save_provider_config,
            set_prompt_evolution_enabled,
            list_provider_models,
            validate_image_endpoint,
            negotiate_voice_session,
            send_model_prompt,
            get_agent_state,
            get_agent_state_revision,
            get_agent_state_delta,
            get_agent_history_page,
            get_agent_trace_state,
            get_agent_session_outputs,
            export_agent_trace_jsonl,
            run_agent_task,
            queue_agent_message,
            edit_queued_agent_message,
            delete_queued_agent_message,
            steer_queued_agent_message,
            run_next_queued_agent_message,
            cancel_agent_task,
            retry_agent_task,
            resolve_agent_permission,
            get_phase5_state,
            run_tool,
            resolve_tool_permission,
            get_phase6_state,
            run_orchestration,
            get_phase7_state,
            ensure_workspace_knowledge,
            index_workspace_rag,
            search_rag,
            answer_with_rag,
            get_phase8_state,
            get_context_state,
            compact_context,
            read_artifact_image,
            read_artifact_preview,
            open_artifact,
            reveal_artifact,
            open_external_url,
            run_browser_tool,
            resolve_browser_permission
        ])
        .build(tauri::generate_context!())
        .expect("error while building Cindx desktop app");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if !confirm_application_exit(app_handle) {
                api.prevent_exit();
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuitConfirmation {
    pub(crate) confirmed: bool,
    pub(crate) suppress_future: bool,
}
